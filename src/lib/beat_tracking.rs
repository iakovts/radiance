// Code in this file is based on and ported from the madmom project:
// @inproceedings{madmom,
//    Title = {{madmom: a new Python Audio and Music Signal Processing Library}},
//    Author = {B{\"o}ck, Sebastian and Korzeniowski, Filip and Schl{\"u}ter, Jan and Krebs, Florian and Widmer, Gerhard},
//    Booktitle = {Proceedings of the 24th ACM International Conference on
//    Multimedia},
//    Month = {10},
//    Year = {2016},
//    Pages = {1174--1178},
//    Address = {Amsterdam, The Netherlands},
//    Doi = {10.1145/2964284.2973795}
// }
//
// Copyright (c) 2022 Eric Van Albert
// Copyright (c) 2012-2014 Department of Computational Perception,
// Johannes Kepler University, Linz, Austria and Austrian Research Institute for
// Artificial Intelligence (OFAI), Vienna, Austria.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this
//    list of conditions and the following disclaimer.
// 2. Redistributions in binary form must reproduce the above copyright notice,
//    this list of conditions and the following disclaimer in the documentation
//    and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
// WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
// (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
// LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
// ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::sync::Arc;
use rustfft::{FftPlanner, num_complex::{Complex, ComplexFloat}, Fft};
use nalgebra::{SVector, SMatrix};

const SAMPLE_RATE: usize = 44100;
// Hop size of 441 with sample rate of 44100 Hz gives an output frame rate of 100 Hz
const HOP_SIZE: usize = 441;
const FRAME_SIZE: usize = 2048;
const SPECTROGRAM_SIZE: usize = FRAME_SIZE / 2;

// MIDI notes corresponding to the low and high filters
// Span a range from 30 Hz to 17000 Hz
const FILTER_MIN_NOTE: i32 = 23; // 30.87 Hz
const FILTER_MAX_NOTE: i32 = 132; // 16744 Hz
// Filtering parameters:
const N_FILTERS: usize = 81;

const LOG_OFFSET: f32 = 1.;

struct FramedSignalProcessor {
    buffer: [i16; FRAME_SIZE * 2], // Circular buffer using double-write strategy to ensure contiguous readout
    write_pointer: usize,
    hop_counter: i32,
}

impl FramedSignalProcessor {
    pub fn new() -> Self {
        Self {
            buffer: [0_i16; FRAME_SIZE * 2],
            write_pointer: 0,
            hop_counter: (HOP_SIZE as i32) - ((FRAME_SIZE / 2) as i32),
        }
    }

    pub fn process(&mut self, samples: &[i16]) -> Vec<SVector<i16, FRAME_SIZE>> {
        let mut result = Vec::<SVector<i16, FRAME_SIZE>>::new();
        for sample in samples {
            self.buffer[self.write_pointer] = *sample;
            self.buffer[self.write_pointer + FRAME_SIZE] = *sample;
            self.write_pointer += 1;
            assert!(self.write_pointer <= FRAME_SIZE);
            if self.write_pointer == FRAME_SIZE {
                self.write_pointer = 0;
            }
            self.hop_counter += 1;
            assert!(self.hop_counter <= HOP_SIZE as i32);
            if self.hop_counter == HOP_SIZE as i32 {
                self.hop_counter = 0;
                let mut chunk = [0_i16; FRAME_SIZE];
                chunk.copy_from_slice(&self.buffer[self.write_pointer..self.write_pointer + FRAME_SIZE]);
                result.push(SVector::from(chunk));
            }
        }
        result
    }
}

struct ShortTimeFourierTransformProcessor {
    window: SVector<f32, FRAME_SIZE>,
    fft: Arc<dyn Fft<f32>>,
}

fn hann(n: usize, m: usize) -> f32 {
    0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / (m as f32 - 1.)).cos()
}

impl ShortTimeFourierTransformProcessor {
    pub fn new() -> Self {
        // Generate a hann window that also normalizes i16 audio data to the range -1 to 1
        let mut window = SVector::from([0_f32; FRAME_SIZE]);
        for i in 0..FRAME_SIZE {
            window[i] = hann(i, FRAME_SIZE) * (1_f32 / (i16::MAX as f32));
        }

        // Plan the FFT
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME_SIZE);

        Self {
            window,
            fft,
        }
    }

    pub fn process(&mut self, frame: &SVector<i16, FRAME_SIZE>) -> SVector<f32, SPECTROGRAM_SIZE> {
        let mut buffer = [Complex {re: 0_f32, im: 0_f32}; FRAME_SIZE];
        for i in 0..FRAME_SIZE {
            buffer[i].re = (frame[i] as f32) * self.window[i];
        }
        self.fft.process(&mut buffer);

        // Slight deviation from madmom: the ShortTimeFourierTransformProcessor
        // returns complex values in madmom. Here, it returns a spectrogram (FFT magnitude)

        // Convert FFT to spectrogram by taking magnitude of each element
        let mut result = [0_f32; SPECTROGRAM_SIZE];
        for i in 0..SPECTROGRAM_SIZE {
            result[i] = buffer[i].abs();
        }
        SVector::from(result)
    }
}

fn triangle_filter(start: i32, center: i32, stop: i32) -> SVector<f32, SPECTROGRAM_SIZE> {
    assert!(start < center);
    assert!(center < stop);
    assert!(stop <= SPECTROGRAM_SIZE as i32);

    let mut result = [0_f32; SPECTROGRAM_SIZE];
    let mut sum = 0_f32;
    for i in start + 1..=center {
        let x = (i as f32 - start as f32) / (center as f32 - start as f32);
        if i >= 0 && i < SPECTROGRAM_SIZE as i32 {
            result[i as usize] = x;
        }
        sum += x;
    }
    for i in center + 1..stop {
        let x = (i as f32 - stop as f32) / (center as f32 - stop as f32);
        if i >= 0 && i < SPECTROGRAM_SIZE as i32 {
            result[i as usize] = x;
        }
        sum += x;
    }

    // Normalize
    for i in start + 1..stop {
        if i >= 0 && i < SPECTROGRAM_SIZE as i32 {
            result[i as usize] /= sum;
        }
    }

    SVector::from(result)
}

// MIDI note to frequency in Hz
fn note2freq(note: i32) -> f32 {
    2_f32.powf((note as f32 - 69.) / 12.) * 440.
}

// Returns the frequency corresponding to each entry in the spectrogram
fn spectrogram_frequencies() -> SVector<f32, SPECTROGRAM_SIZE> {
    let mut result = [0_f32; SPECTROGRAM_SIZE];
    for i in 0..SPECTROGRAM_SIZE {
        result[i] = i as f32 * SAMPLE_RATE as f32 / (SPECTROGRAM_SIZE * 2) as f32
    }
    SVector::from(result)
}

// Returns the index of the closest entry in the spectrogram to the given frequency in Hz
fn freq2bin(spectrogram_frequencies: SVector<f32, SPECTROGRAM_SIZE>, freq: f32) -> usize {
    let mut index = SPECTROGRAM_SIZE - 1;
    for i in 1..SPECTROGRAM_SIZE {
        if freq < spectrogram_frequencies[i] {
            let left = spectrogram_frequencies[i - 1];
            let right = spectrogram_frequencies[i];
            index = if (freq - left).abs() < (freq - right).abs() {
                i - 1
            } else {
                i
            };
            break;
        }
    }
    index
}

// Returns a filter bank according to the given constants
pub fn gen_filterbank() -> Box<SMatrix<f32, N_FILTERS, SPECTROGRAM_SIZE>> {
    let freqs = spectrogram_frequencies();

    let filterbank = [[0_f32; N_FILTERS]; SPECTROGRAM_SIZE];
    let mut filterbank: Box<SMatrix<f32, N_FILTERS, SPECTROGRAM_SIZE>> = Box::new(SMatrix::from(filterbank));

    // Generate a set of triangle filters
    let mut filter_index = 0_usize;
    let mut previous_center = -1_i32;
    for note in (FILTER_MIN_NOTE + 1)..=(FILTER_MAX_NOTE - 1) {
        let center = freq2bin(freqs, note2freq(note)) as i32;
        // Skip duplicate filters
        if center == previous_center {
            continue;
        }
        // Expand filter to include at least one spectrogram entry
        let mut start = freq2bin(freqs, note2freq(note - 1)) as i32;
        let mut stop = freq2bin(freqs, note2freq(note + 1)) as i32;
        if stop - start < 2 {
            start = center - 1;
            stop = center + 1;
        }
        filterbank.set_row(filter_index, &triangle_filter(start, center, stop).transpose());
        filter_index += 1;
        previous_center = center;
    }

    // Check that N_FILTERS constant was set appropriately
    assert_eq!(filter_index, N_FILTERS);

    filterbank
}


struct FilteredSpectrogramProcessor {
    filterbank: Box<SMatrix<f32, N_FILTERS, SPECTROGRAM_SIZE>>,
}

impl FilteredSpectrogramProcessor {
    pub fn new() -> Self {
        Self {
            filterbank: gen_filterbank(),
        }
    }

    pub fn process(&mut self, spectrogram: &SVector<f32, SPECTROGRAM_SIZE>) -> SVector<f32, N_FILTERS> {
        let filter_output = *self.filterbank * spectrogram;

        // Slight deviation from madmom: the output of the FilteredSpectrogramProcessor
        // is sent into a LogarithmicSpectrogramProcessor.
        // Instead, we just take the log here and skip defining a LogarithmicSpectrogramProcessor.
        // (It is strange they call it a *spectrogram* processor,
        // as the output of this step is not really a spectrogram)
        filter_output.map(|x| (x + LOG_OFFSET).log10())
    }
}

struct SpectrogramDifferenceProcessor {
    prev: Option<SVector<f32, N_FILTERS>>,
}

impl SpectrogramDifferenceProcessor {
    pub fn new() -> Self {
        Self {
            prev: None,
        }
    }

    pub fn reset(&mut self) {
        self.prev = None;
    }

    pub fn process(&mut self, filtered_data: &SVector<f32, N_FILTERS>) -> SVector<f32, {N_FILTERS * 2}> {
        let prev = match &self.prev {
            None => filtered_data,
            Some(prev) => prev,
        };

        let diff = (filtered_data - prev).map(|x| 0_f32.max(x));

        self.prev = Some(filtered_data.clone());

        let mut result = [0_f32; N_FILTERS * 2];
        result[0..N_FILTERS].copy_from_slice(filtered_data.as_slice());
        result[N_FILTERS..N_FILTERS * 2].copy_from_slice(diff.as_slice());
        SVector::from(result)
    }
}

fn sigmoid(x: f32) -> f32 {
    0.5_f32 * (1_f32 + (0.5_f32 * x).tanh())
}

struct FeedForwardLayer<const OUTPUT_SIZE: usize, const INPUT_SIZE: usize> {
    weights: Box<SMatrix<f32, OUTPUT_SIZE, INPUT_SIZE>>,
    bias: Box<SVector<f32, OUTPUT_SIZE>>,
}

impl<const OUTPUT_SIZE: usize, const INPUT_SIZE: usize> FeedForwardLayer<OUTPUT_SIZE, INPUT_SIZE> {
    pub fn new(weights: Box<SMatrix<f32, OUTPUT_SIZE, INPUT_SIZE>>, bias: Box<SVector<f32, OUTPUT_SIZE>>) -> Self {
        Self {
            weights,
            bias,
        }
    }

    pub fn process(&self, data: SVector<f32, INPUT_SIZE>) -> SVector<f32, OUTPUT_SIZE> {
        (*self.weights * data + *self.bias).map(sigmoid)
    }
}

struct LSTMLayer {
}

impl LSTMLayer {
    pub fn new() -> Self {
        Self {}
    }
}

struct NeuralNetwork {
}

impl NeuralNetwork {
    pub fn new() -> Self {
        Self {}
    }
}

// Put everything together
struct BeatTracker {
    framed_processor: FramedSignalProcessor,
    stft_processor: ShortTimeFourierTransformProcessor,
    filter_processor: FilteredSpectrogramProcessor,
    difference_processor: SpectrogramDifferenceProcessor,
}

impl BeatTracker {
    pub fn new() -> Self {
        Self {
            framed_processor: FramedSignalProcessor::new(),
            stft_processor: ShortTimeFourierTransformProcessor::new(),
            filter_processor: FilteredSpectrogramProcessor::new(),
            difference_processor: SpectrogramDifferenceProcessor::new(),
        }
    }

    pub fn process(&mut self, samples: &[i16]) -> Vec<SVector<f32, {N_FILTERS * 2}>> {
        println!("Processing {:?} samples", samples.len());
        let frames = self.framed_processor.process(samples);
        println!("Yielded {:?} frames", frames.len());
        frames.iter().map(|frame| {
            let spectrogram = self.stft_processor.process(frame);
            let filtered = self.filter_processor.process(&spectrogram);
            let diff = self.difference_processor.process(&filtered);
            // TODO: NN
            diff
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framed_signal_processor() {
        let audio_signal: Vec<i16> = (0_i16..2048).collect();
        let mut framed_signal_processor = FramedSignalProcessor::new();
        let frames = framed_signal_processor.process(&audio_signal[0..512]);
        assert_eq!(frames.len(), 0); // No frames should be returned yet
        let frames = framed_signal_processor.process(&audio_signal[512..1024]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0][0], 0);
        assert_eq!(frames[0][1023], 0);
        assert_eq!(frames[0][1024], 0);
        assert_eq!(frames[0][1025], 1);
        assert_eq!(frames[0][2047], 1023);
        let frames = framed_signal_processor.process(&audio_signal[1024..2048]);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0][584], 1);
        assert_eq!(frames[0][1024], 441);
        assert_eq!(frames[0][2047], 1464);
        assert_eq!(frames[1][143], 1);
        assert_eq!(frames[1][1024], 882);
        assert_eq!(frames[1][2047], 1905);
    }

    #[test]
    fn test_hann() {
        assert_eq!(hann(0, 7), 0.);
        assert_eq!(hann(1, 7), 0.25);
        assert_eq!(hann(2, 7), 0.75);
        assert_eq!(hann(3, 7), 1.);
        assert_eq!(hann(4, 7), 0.74999994);
        assert_eq!(hann(5, 7), 0.24999982);
        assert_eq!(hann(6, 7), 0.);
    }

    #[test]
    fn test_stft_processor() {
        let mut audio_frame = [0_i16; 2048];
        audio_frame.copy_from_slice(&(0_i16..2048).collect::<Vec<_>>()[..]);
        let audio_frame = SVector::from(audio_frame);
        let mut stft_processor = ShortTimeFourierTransformProcessor::new();
        let result = stft_processor.process(&audio_frame);

        assert_eq!(result[0], 31.96973);
        assert_eq!(result[1], 17.724249);
        assert_eq!(result[2], 1.7021317);
        assert_eq!(result[1023], 0.);
    }

    #[test]
    fn test_triangle_filter() {
        let filt = triangle_filter(5, 7, 15);
        assert_eq!(filt[5], 0.);
        assert_eq!(filt[6], 0.1);
        assert_eq!(filt[7], 0.2);
        assert_eq!(filt[8], 0.175);
        assert_eq!(filt[9], 0.15);
        assert_eq!(filt[10], 0.125);
        assert_eq!(filt[11], 0.1);
        assert_eq!(filt[12], 0.075);
        assert_eq!(filt[13], 0.05);
        assert_eq!(filt[14], 0.025);
        assert_eq!(filt[15], 0.);
    }

    #[test]
    fn test_note2freq() {
        assert_eq!(note2freq(69), 440.);
        assert_eq!(note2freq(57), 220.);
        assert_eq!(note2freq(60), 261.62555);
    }

    #[test]
    fn test_spectrogram_frequencies() {
        let freqs = spectrogram_frequencies();
        assert_eq!(freqs[0], 0.);
        assert_eq!(freqs[1], 21.533203);
        assert_eq!(freqs[1022], 22006.934);
        assert_eq!(freqs[1023], 22028.467);
    }

    #[test]
    fn test_freq2bin() {
        let freqs = spectrogram_frequencies();
        assert_eq!(freq2bin(freqs, 0.), 0);
        assert_eq!(freq2bin(freqs, 24000.), 1023);
        assert_eq!(freq2bin(freqs, 440.), 20);
    }

    #[test]
    fn test_gen_filterbank() {
        let filterbank = gen_filterbank();

        // Check triangle filter peaks
        assert_eq!(filterbank[(0,2)], 1.);
        assert_eq!(filterbank[(1,3)], 1.);
        assert_eq!(filterbank[(2,4)], 1.);
        assert_eq!(filterbank[(78,654)], 0.02631579);
        assert_eq!(filterbank[(79,693)], 0.025);
        assert_eq!(filterbank[(80,734)], 0.023529412);
    }

    #[test]
    fn test_spectrogram_difference_processor() {
        let mut data = SVector::from([0_f32; N_FILTERS]);
        let mut proc = SpectrogramDifferenceProcessor::new();

        data[0] = 1.;
        let r1 = proc.process(&data);
        data[0] = 2.;
        let r2 = proc.process(&data);
        data[0] = 1.;
        let r3 = proc.process(&data);

        // First half matches input
        assert_eq!(r1[0], 1.);
        assert_eq!(r2[0], 2.);
        assert_eq!(r3[0], 1.);

        // Second half is clamped differences
        assert_eq!(r1[N_FILTERS], 0.);
        assert_eq!(r2[N_FILTERS], 1.);
        assert_eq!(r3[N_FILTERS], 0.);
    }

    #[test]
    fn test_sigmoid() {
        assert_eq!(sigmoid(0.), 0.5);
        assert_eq!(sigmoid(2.), 0.8807971);
        assert_eq!(sigmoid(-4.), 0.017986208);
    }

    #[test]
    fn test_feed_forward_layer() {
        let weights = Box::new(SMatrix::from([[1_f32, 0.], [1., 1.], [1., 0.]]));
        let bias = Box::new(SVector::from([0_f32, 5.]));
        let layer = FeedForwardLayer::new(weights, bias);
        let out = layer.process(SVector::from([0.5_f32, 0.6, 0.7]));

        assert_eq!(out[0], sigmoid(1.8));
        assert_eq!(out[1], sigmoid(5.6));
    }

    #[ignore]
    #[test]
    fn test_music() {
        use std::fs::File;
        use std::path::Path;

        // Read music from audio file
        let mut inp_file = File::open(Path::new("src/lib/test/frontier.wav")).unwrap();
        let (header, data) = wav::read(&mut inp_file).unwrap();
        assert_eq!(header.audio_format, wav::WAV_FORMAT_PCM);
        assert_eq!(header.channel_count, 1);
        assert_eq!(header.sampling_rate, 44100);
        assert_eq!(header.bits_per_sample, 16);
        let data = data.try_into_sixteen().unwrap();

        println!("WAV file has {:?} samples", data.len());

        // Instantiate a BeatTracker
        let mut bt = BeatTracker::new();
        let result = bt.process(&data);
        println!("{:?}", result[128]);
        panic!();
    }
}
