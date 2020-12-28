use crate::types::{Texture, BlankTexture, NoiseTexture, WorkerPool, WorkHandle, WorkResult, FetchContent, Resolution, Timebase};
use std::rc::Rc;
use shaderc;
use std::fmt;
use wgpu;
use std::num::NonZeroU32;
use bytemuck;

/// The EffectNodePaintState contains chain-specific data.
/// It is constructed by calling new_paint_state() and mutated by paint().
/// The application should construct and hold on to one paint state per render chain.
#[derive(Debug)]
pub struct EffectNodePaintState {
    input_textures: Vec<Rc<Texture>>,
    output_texture: Rc<Texture>,
}

/// The EffectNode struct contains context-specific, chain-agnostic data.
/// It is constructed by calling new()
#[derive(Debug)]
pub struct EffectNode<UpdateContext: WorkerPool + FetchContent + Timebase> {
    pending: EffectNodePendingChanges,
    state: EffectNodeState<UpdateContext>,
    name: Option<String>,
    intensity: f32,
}

enum EffectNodeState<UpdateContext: WorkerPool + FetchContent + Timebase> {
    Uninitialized,
    // Note: The work handle below is really not optional.
    // The Option<> is only there to allow "taking" it as soon as compilation is done.
    Compiling {shader_compilation_work_handle: Option<<UpdateContext as WorkerPool>::Handle<Result<Vec<u8>, String>>>},
    Ready(ReadyState),
    Error(String),
}

// Extra state associated with an EffectNode when it is Ready
struct ReadyState {
    render_pipeline: wgpu::RenderPipeline,
    update_bind_group: wgpu::BindGroup,
    paint_bind_group_layout: wgpu::BindGroupLayout,
    update_uniform_buffer: wgpu::Buffer,
    paint_uniform_buffer: wgpu::Buffer,
    n_inputs: u32,
}

impl<UpdateContext: WorkerPool + FetchContent + Timebase> fmt::Debug for EffectNodeState<UpdateContext> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EffectNodeState::Uninitialized => write!(f, "Uninitialized"),
            EffectNodeState::Compiling {shader_compilation_work_handle: _} => write!(f, "Compiling"),
            EffectNodeState::Ready(_) => write!(f, "Ready"),
            EffectNodeState::Error(e) => write!(f, "Error({})", e),
        }
    }
}

/// Holds the "pending" copy of this EffectNode's state.
/// This is how you tell the node what it should be doing.
#[derive(Debug)]
pub struct EffectNodePendingChanges {
    pub name: Option<String>,
    pub intensity: f32,
}

/// The uniform buffer associated with the effect (chain-agnostic)
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
#[allow(non_snake_case)]
struct UpdateUniforms {
    iAudio: [f32; 4],
    iStep: f32,
    iTime: f32,
    iFrequency: f32,
    iIntensity: f32,
    iIntensityIntegral: f32,
}

/// The uniform buffer associated with the effect (chain-specific)
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
#[allow(non_snake_case)]
struct PaintUniforms {
    iResolution: [f32; 2],
    iFPS: f32,
}

const EFFECT_HEADER: &str = include_str!("effect_header.glsl");

impl<UpdateContext: WorkerPool + FetchContent + Timebase> EffectNode<UpdateContext> {
    pub fn new() -> EffectNode<UpdateContext> {
        let pending = EffectNodePendingChanges {
            name: None,
            intensity: 0.,
        };

        EffectNode {
            state: EffectNodeState::Uninitialized,
            name: pending.name.clone(),
            intensity: pending.intensity,
            pending,
        }
    }

    // Called when the name changes. Sets the state to Compiling and kicks off shaderc in a worker.
    fn start_compiling_shader(&mut self, context: &UpdateContext) {
        let shader_content_closure = context.fetch_content_closure(&self.name.as_ref().unwrap());
        let shader_name = self.name.as_ref().unwrap().to_owned();

        let shader_compilation_work_handle = context.spawn(move || {
            let effect_src = shader_content_closure().map_err(|e| e.to_string())?;
            let frag_src = format!("{}{}\n", EFFECT_HEADER, effect_src);
            let mut compiler = shaderc::Compiler::new().unwrap();
            let compilation_result = compiler.compile_into_spirv(&frag_src, shaderc::ShaderKind::Fragment, &shader_name, "main", None);
            match compilation_result {
                Ok(artifact) => Ok(artifact.as_binary_u8().to_vec()),
                Err(e) => Err(e.to_string()),
            }
        });
        self.state = EffectNodeState::Compiling {shader_compilation_work_handle: Some(shader_compilation_work_handle)};
    }

    // Called when the shader compilation is finished. Sets up the render pipeline that will be used in paint calls, and sets the state to Ready.
    fn setup_render_pipeline(&mut self, device: &wgpu::Device, frag_binary: &[u8]) {
        let vs_module = device.create_shader_module(wgpu::include_spirv!(concat!(env!("OUT_DIR"), "/effect_vertex.spv")));
        let fs_module = device.create_shader_module(wgpu::util::make_spirv(frag_binary));

        let n_inputs = 1_u32; // XXX read from file

        // The effect will have two bind groups, one which will be bound in update() (most uniforms & sampler)
        // and one which will be bound in paint() (a few uniforms & textures)

        // The "update" bind group:
        // 0: UpdateUniforms
        // 1: iSampler

        // The "paint" bind group:
        // 0: PaintUniforms
        // 1: iInputsTex[]
        // 2: iNoiseTex
        // 3: iChannelTex[]

        let update_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, // UpdateUniforms
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: wgpu::BindingType::UniformBuffer {
                        dynamic: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, // iSampler
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: wgpu::BindingType::Sampler {
                        comparison: false,
                    },
                    count: None,
                },
            ],
            label: Some("update bind group layout"),
        });

        let paint_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, // PaintUniforms
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: wgpu::BindingType::UniformBuffer {
                        dynamic: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, // iInputsTex
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: wgpu::BindingType::SampledTexture {
                        multisampled: false,
                        dimension: wgpu::TextureViewDimension::D2,
                        component_type: wgpu::TextureComponentType::Uint,
                    },
                    count: NonZeroU32::new(n_inputs),
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2, // iNoiseTex
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: wgpu::BindingType::SampledTexture {
                        multisampled: false,
                        dimension: wgpu::TextureViewDimension::D2,
                        component_type: wgpu::TextureComponentType::Uint,
                    },
                    count: None,
                },
                //wgpu::BindGroupLayoutEntry {
                //    binding: 3, // iChannelTex
                //    visibility: wgpu::ShaderStage::FRAGMENT,
                //    ty: wgpu::BindingType::SampledTexture {
                //        multisampled: false,
                //        dimension: wgpu::TextureViewDimension::D2,
                //        component_type: wgpu::TextureComponentType::Uint,
                //    },
                //    count: NonZeroU32::new(n_inputs),
                //},
            ],
            label: Some("paint bind group layout"),
        });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[&update_bind_group_layout, &paint_bind_group_layout],
                push_constant_ranges: &[],
            });

        // Create a render pipeline, we will eventually want multiple of these for a multi-pass effect
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex_stage: wgpu::ProgrammableStageDescriptor {
                module: &vs_module,
                entry_point: "main",
            },
            fragment_stage: Some(wgpu::ProgrammableStageDescriptor {
                module: &fs_module,
                entry_point: "main",
            }),
            rasterization_state: Some(
                wgpu::RasterizationStateDescriptor {
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: wgpu::CullMode::Back,
                    depth_bias: 0,
                    depth_bias_slope_scale: 0.0,
                    depth_bias_clamp: 0.0,
                    clamp_depth: false,
                }
            ), 
            color_states: &[
                wgpu::ColorStateDescriptor {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    color_blend: wgpu::BlendDescriptor::REPLACE,
                    alpha_blend: wgpu::BlendDescriptor::REPLACE,
                    write_mask: wgpu::ColorWrite::ALL,
                },
            ],
            primitive_topology: wgpu::PrimitiveTopology::TriangleStrip,
            depth_stencil_state: None,
            vertex_state: wgpu::VertexStateDescriptor {
                index_format: wgpu::IndexFormat::Uint16,
                vertex_buffers: &[],
            },
            sample_count: 1,
            sample_mask: !0,
            alpha_to_coverage_enabled: false,
        });

        // The update uniform buffer for this effect
        let update_uniform_buffer = device.create_buffer(
            &wgpu::BufferDescriptor {
                label: Some("update uniform buffer"),
                size: std::mem::size_of::<UpdateUniforms>() as u64,
                usage: wgpu::BufferUsage::UNIFORM | wgpu::BufferUsage::COPY_DST,
                mapped_at_creation: false,
            }
        );

        // The paint uniform buffer for this effect
        let paint_uniform_buffer = device.create_buffer(
            &wgpu::BufferDescriptor {
                label: Some("paint uniform buffer"),
                size: std::mem::size_of::<PaintUniforms>() as u64,
                usage: wgpu::BufferUsage::UNIFORM | wgpu::BufferUsage::COPY_DST,
                mapped_at_creation: false,
            }
        );

        // The sampler that will be used for texture access within the shaders
        let sampler = device.create_sampler(
            &wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }
        );

        // The update bind group is actually static, since we will just issue updates the uniform buffer
        let update_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &update_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(update_uniform_buffer.slice(..))
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("update bind group"),
        });

        self.state = EffectNodeState::Ready(ReadyState {
            render_pipeline,
            update_bind_group,
            paint_bind_group_layout,
            update_uniform_buffer,
            paint_uniform_buffer,
            n_inputs,
        });
    }

    /// Updates the given EffectNode.
    /// This function should be called to advance the pattern held by this EffectNode.
    /// Here are some of the things this function is responsible for:
    ///  * Apply all changes from the EffectNodePendingChanges struct
    ///  * Poll completion of asynchronous work
    ///  * Get and save any new "globals" from the context (such as iTime and iAudio)
    /// The basic render loop pattern looks like this:
    ///  1. Construct a new EffectNode
    ///  2. Construct EffectNodePaintStates for each render chain
    ///  3. Call update once
    ///  4. Call paint once for each chain
    ///  5. Goto 3
    pub fn update(&mut self, context: &UpdateContext, device: &wgpu::Device, queue: &wgpu::Queue) {
        // Update internal state based on self.pending
        let name_changed = self.name != self.pending.name;
        if name_changed {
            self.name = self.pending.name.clone();
            // Always recompile if name changed
            match self.name {
                Some(_) => {self.start_compiling_shader(context);}
                None => {self.state = EffectNodeState::Uninitialized;},
            };
        } else if let EffectNodeState::Compiling {shader_compilation_work_handle: handle_opt} = &mut self.state {
            // See if compilation is finished
            let handle_ref = handle_opt.as_ref().unwrap();
            if !handle_ref.alive() {
                let handle = handle_opt.take().unwrap();
                match handle.join() {
                    WorkResult::Ok(result) => {
                        match result {
                            Ok(binary) => {
                                self.setup_render_pipeline(device, &binary);
                            },
                            Err(msg) => {
                                self.state = EffectNodeState::Error(msg.to_string());
                                println!("Shader compilation error: {}", msg.to_string());
                            },
                        }
                    },
                    WorkResult::Err(_) => {
                        self.state = EffectNodeState::Error("Shader compilation panicked".to_owned());
                    },
                }
            }
        }

        self.intensity = self.pending.intensity;

        if let EffectNodeState::Ready(ready_state) = &mut self.state {
            // Node is ready; we should set the uniforms
            // TODO set these dynamically, from context()
            let uniforms = UpdateUniforms {
                iAudio: [0., 0., 0., 0.],
                iStep: 0., // What's this?
                iTime: context.time(),
                iFrequency: 1.,
                iIntensity: self.intensity,
                iIntensityIntegral: 0.,
            };
            queue.write_buffer(&ready_state.update_uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }
    }

    /// Call this when a new chain is added to get a PaintState
    /// suitable for use with paint().
    pub fn new_paint_state<PaintContext: BlankTexture + NoiseTexture + Resolution>(&self, context: &PaintContext, device: &wgpu::Device) -> EffectNodePaintState {
        let (width, height) = context.resolution();

        let texture_desc = wgpu::TextureDescriptor {
            size: wgpu::Extent3d {
                width,
                height,
                depth: 1,
            },
            //array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsage::COPY_SRC
                | wgpu::TextureUsage::OUTPUT_ATTACHMENT
                | wgpu::TextureUsage::SAMPLED
                ,
            label: None,
        };

        let texture = device.create_texture(&texture_desc);
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(
            &wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }
        );

        EffectNodePaintState{
            input_textures: Vec::new(),
            output_texture: Rc::new(Texture {
                texture,
                view,
                sampler,
            }),
        }
    }

    /// Updates the given PaintState.
    /// Paint should be lightweight and not kick off any CPU work (update should do that.)
    pub fn paint<PaintContext: BlankTexture + NoiseTexture + Resolution>(&self, context: &PaintContext, device: &wgpu::Device, queue: &wgpu::Queue, paint_state: &mut EffectNodePaintState, inputs: &[Option<Rc<Texture>>]) -> (Vec<wgpu::CommandBuffer>, Rc<Texture>) {
        match &self.state {
            EffectNodeState::Ready(ready_state) => {
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Render Encoder"),
                });

                {
                    // Populate the paint uniforms
                    let (width, height) = context.resolution();
                    let uniforms = PaintUniforms {
                        iResolution: [width as f32, height as f32],
                        iFPS: 60., // TODO set dynamically
                    };
                    queue.write_buffer(&ready_state.paint_uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
                }

                // Populate the paint bind group

                // Make an array of input textures
                // TODO repeatedly creating all these views seems bad,
                // but TextureViewArray takes in &[TextureView], not &[&TextureView] so it's hard.
                let input_binding: Vec<wgpu::TextureView> = (0..ready_state.n_inputs).map(|i| {
                    match inputs.get(i as usize) {
                        Some(opt_tex) => match opt_tex {
                            Some(tex) => tex.texture.create_view(&Default::default()),
                            None => context.blank_texture().texture.create_view(&Default::default()),
                        },
                        None => context.blank_texture().texture.create_view(&Default::default()),
                    }
                }).collect();

                let paint_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &ready_state.paint_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0, // PaintUniforms
                            resource: wgpu::BindingResource::Buffer(ready_state.paint_uniform_buffer.slice(..))
                        },
                        wgpu::BindGroupEntry {
                            binding: 1, // iInputsTex
                            resource: wgpu::BindingResource::TextureViewArray(input_binding.as_slice())
                        },
                        wgpu::BindGroupEntry {
                            binding: 2, // iNoiseTex
                            resource: wgpu::BindingResource::TextureView(&context.noise_texture().view)
                        },
                        //wgpu::BindGroupEntry {
                        //    binding: 3, // iChannelTex
                        //    resource: wgpu::BindingResource::TextureViewArray()
                        //},
                    ],
                    label: Some("update bind group"),
                });

                {
                    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[
                            wgpu::RenderPassColorAttachmentDescriptor {
                                attachment: &paint_state.output_texture.view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(
                                        wgpu::Color {
                                            r: 0.1,
                                            g: 0.2,
                                            b: 0.3,
                                            a: 1.0,
                                        }
                                    ),
                                    store: true,
                                }
                            }
                        ],
                        depth_stencil_attachment: None,
                    });

                    render_pass.set_pipeline(&ready_state.render_pipeline);
                    render_pass.set_bind_group(0, &ready_state.update_bind_group, &[]); 
                    render_pass.set_bind_group(1, &paint_bind_group, &[]); 
                    render_pass.draw(0..4, 0..1);
                }

                (vec![encoder.finish()], paint_state.output_texture.clone())
            },
            _ => (vec![], context.blank_texture()),
        }
    }

    // Getters and setters
    pub fn name(&self) -> Option<&str> {
        match &self.name {
            None => None,
            Some(n) => Some(&n),
        }
    }

    pub fn set_name(&mut self, name: Option<&str>) {
        self.pending.name = match name {
            None => None,
            Some(s) => Some(s.to_owned()),
        };
    }

    pub fn intensity(&self) -> f32 {
        self.intensity
    }

    pub fn set_intensity(&mut self, intensity: f32) {
        self.pending.intensity = intensity;
    }
}
