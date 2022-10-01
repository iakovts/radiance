use crate::beat_tracking::{BeatTracker, SAMPLE_RATE};
use cpal;
use cpal::traits::DeviceTrait;
use std::sync::mpsc;
use std::time;

const MAX_TIME: f32 = 64.;
// Anticipate beats by this many seconds
const LATENCY_COMPENSATION: f32 = 0.07;

/// A Mir (Music information retrieval) object
/// handles listening to the music via the system audio
/// and generates a global timebase from the beats,
/// along with other relevant real-time inputs based on the music
/// such as the low, mid, high, and overall audio level.
/// It can be polled from the main thread at every frame,
/// at which point it will compute and return the current time in beats,
/// and the four audio levels.
/// It has a buffer of about a second or so,
/// but should be polled frequently to avoid dropped updates.
pub struct Mir {
    _stream: cpal::Stream,
    receiver: mpsc::Receiver<Update>,
    last_update: Update,
}

/// Updates sent over a queue
/// from the audio thread to the main thread
/// containing new data from which to produce MusicInfos
/// when polled
#[derive(Clone, Debug)]
struct Update {
    // For computing t in beats
    // We will do a linear calculation
    // of the form t = tempo * (x - wall_ref) + t_ref
    // x (the wallclock time measured at the time of poll(); an Instant)
    // and wall_ref (a reference Instant in wallclock time)
    // yield (x - wall_ref) (a duration in seconds.)
    // m is a tempo in beats per second.
    wall_ref: time::Instant, // reference wall clock time
    t_ref: f32, // reference t measured in beats
    tempo: f32, // beats per second

    // For computing the audio levels
    low: f32,
    mid: f32,
    high: f32,
    level: f32,
}

impl Update {
    fn t(&self, wall: time::Instant) -> f32 {
        let elapsed = (wall - self.wall_ref).as_secs_f32();
        let t = self.tempo * elapsed + self.t_ref;
        t.rem_euclid(MAX_TIME)
    }
}

/// The structure returned from the Mir object when it is polled,
/// containing real-time information about the audio.
#[derive(Clone, Debug)]
pub struct MusicInfo {
    pub t: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
    pub level: f32,
    // TODO: send full spectrogram
}

impl Mir {
    pub fn new() -> Self {
        // Make a new beat tracker
        let mut bt = BeatTracker::new();

        // Make a communication channel to communicate with the audio thread
        const MESSAGE_BUFFER_SIZE: usize = 16;
        let (sender, receiver) = mpsc::sync_channel(MESSAGE_BUFFER_SIZE); 

        // Set up system audio
        use cpal::traits::HostTrait;
        let host = cpal::default_host();
        let device = host.default_output_device().expect("no output device available");

        const MIN_USEFUL_BUFFER_SIZE: cpal::FrameCount = 256; // Lower actually would be useful, but CPAL lies about the min size, so this ought to be safe
        const SAMPLE_RATE_CPAL: cpal::SampleRate = cpal::SampleRate(SAMPLE_RATE as u32);
        let config_range = device.supported_input_configs()
            .expect("error while querying configs")
            .filter(|config| 
                (config.sample_format() == cpal::SampleFormat::I16
                || config.sample_format() == cpal::SampleFormat::U16)
                && SAMPLE_RATE_CPAL >= config.min_sample_rate()
                && SAMPLE_RATE_CPAL <= config.max_sample_rate()
                && match *config.buffer_size() {
                    cpal::SupportedBufferSize::Range {max, ..} => MIN_USEFUL_BUFFER_SIZE <= max,
                    cpal::SupportedBufferSize::Unknown => true,
                } && (config.channels() == 1
                || config.channels() == 2
                )
            ).min_by_key(|config| match *config.buffer_size() {
                cpal::SupportedBufferSize::Range {min, ..} => MIN_USEFUL_BUFFER_SIZE.max(min),
                cpal::SupportedBufferSize::Unknown => 8192, // Large but not unreasonable
            })
            .expect("no supported config")
            .with_sample_rate(SAMPLE_RATE_CPAL);

        println!("MIR: Found supported audio config: {:?}", config_range);
        let mut config = config_range.config();

        if let cpal::SupportedBufferSize::Range {min, ..} = *config_range.buffer_size() {
            config.buffer_size = cpal::BufferSize::Fixed(MIN_USEFUL_BUFFER_SIZE.max(min));
        }

        println!("MIR: Choosing audio config: {:?}", config);

        // This tempo will be quickly overridden as the audio thread
        // starts tapping out the real beat
        const DEFAULT_BPM: f32 = 120.;
        let mut update = Update {
            wall_ref: time::Instant::now(),
            t_ref: 0.,
            tempo: DEFAULT_BPM / 60.,
            low: 0.,
            mid: 0.,
            high: 0.,
            level: 0.,
        };

        let mut process_audio_i16_mono = move |data: &[i16]| {
            // Reduce all of the returned results into just the most recent
            // Typically; only 0 or 1 results are returned per audio frame,
            // but we do this reduction just to be safe,
            // in case the audio frames returned are really large
            let recent_result = bt.process(data).into_iter().reduce(|(_, _, beat_acc), (spectrogram, activation, beat)| (spectrogram, activation, beat_acc && beat));
            let (spectrogram, _activation, beat) = match recent_result {
                Some(result) => result,
                None => {return;},
            };

            // Compute the update
            // If we detected a beat, recompute the linear parameters for t
            if beat {
                // In computing the new line, we want to preserve continuity;
                // i.e. we want to pivot our line about the current point (wall clock time, current t in beats)
                // So, we set wall_ref to right now, and t_ref to t(wall_ref)
                let wall_ref = time::Instant::now();
                let t_ref = update.t(wall_ref);

                // Now we just have one remaining parameter to set: the slope (aka tempo)
                // We set the slope of the line so that it intersects the point
                // (expected wall clock time of next beat, current integer beat + 1)

                // Inter-arrival time of the last two beats, in seconds
                let last_beat_wall_period = (wall_ref - update.wall_ref).as_secs_f32();
                // Next beat
                let next_beat = t_ref.round() + 1.0;
                // Amount of ground we need to cover, in number of beats
                let beats_to_cover = next_beat - t_ref;
                // Typically, beats_to_cover should be close to 1.0 if we're doing a good job.

                let tempo = beats_to_cover / last_beat_wall_period;
                // The beat tracker is good about not tapping beats too fast,
                // so the denominator should never produce tempos that are too high.

                update.wall_ref = wall_ref;
                update.t_ref = t_ref;
                update.tempo = tempo;
            }

            // For now, simply set lows, mids, and highs to random spectrogram buckets
            update.low = spectrogram[2];
            update.mid = spectrogram[100];
            update.high = spectrogram[800];
            update.level = update.mid;

            // Send an update back to the main thread
            if let Err(err) = sender.try_send(update.clone()) {
                match err {
                    mpsc::TrySendError::Full(_) => {
                        println!("MIR: buffer full; dropping update (polling too slow?)");
                    },
                    mpsc::TrySendError::Disconnected(_) => {
                        println!("MIR: main thread disconnected; dropping update");
                    },
                }
            };
        };

        let process_error = move |err| {
            println!("MIR: audio stream error: {:?}", err)
        };

        let stream = match (config_range.sample_format(), config.channels) {
                (cpal::SampleFormat::I16, 1) => device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        process_audio_i16_mono(data);
                    },
                    process_error,
                ),
                (cpal::SampleFormat::I16, 2) => device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let data: Vec<i16> = data.chunks(2).map(|pair| ((pair[0] as i32 + pair[1] as i32) / 2) as i16).collect();
                        process_audio_i16_mono(&data);
                    },
                    process_error,
                ),
                (cpal::SampleFormat::U16, 1) => device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        let data: Vec<i16> = data.iter().map(|&x| ((x as i32) - 32768) as i16).collect();
                        process_audio_i16_mono(&data);
                    },
                    process_error,
                ),
                (cpal::SampleFormat::U16, 2) => device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        let data: Vec<i16> = data.chunks(2).map(|pair| ((pair[0] as i32 + pair[1] as i32) / 2 - 32768) as i16).collect();
                        process_audio_i16_mono(&data);
                    },
                    process_error,
                ),
                _ => panic!("unexpected sample format or channel count")
        }.expect("failed to open input stream");

        Self {
            _stream: stream,
            receiver,
            last_update: Update {
                wall_ref: time::Instant::now(),
                t_ref: 0.,
                tempo: 0., // This will hold t at 0 until the audio thread starts up
                low: 0.,
                mid: 0.,
                high: 0.,
                level: 0.,
            },
        }
    }

    pub fn poll(&mut self) -> MusicInfo {
        // Drain the receiver,
        // applying the most recent update from the audio thread
        match self.receiver.try_iter().last() {
            Some(update) => {self.last_update = update;},
            None => {},
        }

        // Compute t
        let t = self.last_update.t(time::Instant::now() + time::Duration::from_secs_f32(LATENCY_COMPENSATION));

        // Simply take the most recent values for the audio levels
        let low = self.last_update.low;
        let mid = self.last_update.mid;
        let high = self.last_update.high;
        let level = self.last_update.level;

        MusicInfo {
            t,
            low,
            mid,
            high,
            level,
        }
    }
}
