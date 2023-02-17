/// This module handles radiance output through winit
/// (e.g. actually displaying ScreenOutputNode to a screen)

use egui_winit::winit;
use egui_winit::winit::{
    event::*,
    event_loop::EventLoopWindowTarget,
    window::{WindowBuilder, Fullscreen},
    monitor::MonitorHandle,
    dpi::{PhysicalSize, PhysicalPosition},
};
use std::sync::Arc;
use std::iter;
use std::collections::HashMap;
use serde_json::json;

#[derive(Debug)]
pub struct WinitOutput {
    instance: Arc<wgpu::Instance>,
    adapter: Arc<wgpu::Adapter>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    shader_module: wgpu::ShaderModule,
    bind_group_layout: wgpu::BindGroupLayout,
    render_pipeline_layout: wgpu::PipelineLayout,

    screen_outputs: HashMap<radiance::NodeId, ScreenOutput>,
    available_screens: HashMap<String, (PhysicalPosition<i32>, PhysicalSize<u32>)>,
}

#[derive(Debug)]
struct ScreenOutput {
    // Cached props
    visible: bool,

    // Resources
    window: egui_winit::winit::window::Window,
    surface: wgpu::Surface,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    render_target_id: radiance::RenderTargetId,
    render_target: radiance::RenderTarget,

    // Internal
    initial_update: bool, // Initialized to false, set to true on first update.
}

impl ScreenOutput {
    fn resize(&mut self, device: &wgpu::Device, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(device, &self.config);
        }
    }
}

impl WinitOutput {
    pub fn new(instance: Arc<wgpu::Instance>, adapter: Arc<wgpu::Adapter>, device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Output shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("output.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
            label: Some("output texture bind group layout"),
        });

        let render_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Output Render Pipeline Layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            }
        );

        WinitOutput {
            instance,
            adapter,
            device,
            queue,
            shader_module,
            bind_group_layout,
            render_pipeline_layout,
            screen_outputs: HashMap::<radiance::NodeId, ScreenOutput>::new(),
            available_screens: HashMap::<String, (PhysicalPosition<i32>, PhysicalSize<u32>)>::new(),
        }
    }

    pub fn render_targets_iter(&self) -> impl Iterator<Item=(&radiance::RenderTargetId, &radiance::RenderTarget)> {
        self.screen_outputs.values().map(|screen_output| (&screen_output.render_target_id, &screen_output.render_target))
    }

    pub fn update<T>(&mut self, event_loop: &EventLoopWindowTarget<T>, props: &mut radiance::Props) {
        // Mark all nodes that we know about as having received their initial update.
        // Painting is gated on this being true,
        // because otherwise, we might try to paint a render target that the radiance context doesn't know about.
        // After the initial update, the radiance context is guaranteed to know about this screen output's render target.
        for screen_output in self.screen_outputs.values_mut() {
            screen_output.initial_update = true;
        }

        // Prune screen_outputs of any nodes that are no longer present in the given graph
        self.screen_outputs.retain(|id, _| props.node_props.get(id).map(|node_props| matches!(node_props, radiance::NodeProps::ScreenOutputNode(_))).unwrap_or(false));

        // Construct screen_outputs for any ScreenOutputNodes we didn't know about
        for (node_id, node_props) in props.node_props.iter() {
            match node_props {
                radiance::NodeProps::ScreenOutputNode(_) => {
                    if !self.screen_outputs.contains_key(node_id) {
                        self.screen_outputs.insert(
                            *node_id,
                            self.new_screen_output(event_loop),
                        );
                    }
                },
                _ => {},
            }
        }

        // See what screens are available for output
        self.available_screens = event_loop.available_monitors().filter_map(|mh| {
            let name = mh.name()?;
            let size = mh.size();
            if size.width == 0 && size.height == 0 {
                return None;
            }
            Some((name, (mh.position(), size)))
        }).collect();

        let mut screen_names: Vec<String> = self.available_screens.keys().cloned().collect();
        screen_names.sort();

        // Update internal state of screen_outputs from props
        for (node_id, screen_output) in self.screen_outputs.iter_mut() {
            let screen_output_props: &mut radiance::ScreenOutputNodeProps = props.node_props.get_mut(node_id).unwrap().try_into().unwrap();

            // Populate each screen output node props with a list of screens available on the system
            screen_output_props.available_screens = screen_names.clone();
            if !self.available_screens.contains_key(&screen_output_props.screen) {
                // Hide any outputs that point to screens we don't know about
                screen_output_props.visible = false;
            }

            // Cache props and act on them
            let newly_visible = !screen_output.visible && screen_output_props.visible;
            screen_output.visible = screen_output_props.visible;
            screen_output.window.set_visible(screen_output.visible);
            if newly_visible {
                println!("NEWLY VISIBLE!!");
                let &(target_screen_position, target_screen_size) = self.available_screens.get(&screen_output_props.screen).unwrap();
                screen_output.window.set_resizable(false);
                screen_output.window.set_decorations(false);

                // TODO: do some X11 shit to actually float the window in XMonad
                //let xconn = screen_output.window.get_xlib_xconnection().unwrap();
                //let prop_atom = unsafe { xconn.get_atom_unchecked(b"_NET_WM_STATE_ABOVE\0") };
                //let type_atom = unsafe { xconn.get_atom_unchecked(b"UTF8_STRING\0") };
                //xconn.change_property(
                //    screen_output.window.get_xlib_window().unwrap(),
                //    prop_atom,
                //    type_atom,
                //    util::PropMode::Replace,
                //    b"francesca64\0",
                //).flush().expect("Failed to set `CUTEST_CONTRIBUTOR` property");
                //
                // self.toggle_atom(b"_NET_WM_STATE_ABOVE\0", level == WindowLevel::AlwaysOnTop)
                //            .queue();

                screen_output.window.set_inner_size(target_screen_size);
                screen_output.window.set_outer_position(target_screen_position);
                screen_output.window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                println!("Move to: {:?}", target_screen_position);
            }
        }
    }

    fn new_screen_output<T>(&self, event_loop: &EventLoopWindowTarget<T>) -> ScreenOutput {
        let window = WindowBuilder::new().build(&event_loop).unwrap();
        let size = window.inner_size();
        let surface = unsafe { self.instance.create_surface(&window) };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface.get_supported_formats(&self.adapter)[0],
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
        };
        surface.configure(&self.device, &config);

        let render_pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Output Render Pipeline"),
            layout: Some(&self.render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader_module,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        let render_target_id = radiance::RenderTargetId::gen();
        let render_target: radiance::RenderTarget = serde_json::from_value(json!({
            "width": 1920,
            "height": 1080,
            "dt": 1. / 60.
        })).unwrap();

        ScreenOutput {
            visible: false,
            window,
            surface,
            config,
            render_pipeline,
            render_target_id,
            render_target,
            initial_update: false,
        }
    }

    pub fn on_event<T>(&mut self, event: &Event<T>, ctx: &mut radiance::Context) -> bool {
        // Return true => event consumed
        // Return false => event continues to be processed

        for (node_id, screen_output) in self.screen_outputs.iter_mut() {
            match event {
                Event::RedrawRequested(window_id) if window_id == &screen_output.window.id() => {
                    if screen_output.initial_update && screen_output.visible {
                        // Paint
                        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("Output Encoder"),
                        });

                        let results = ctx.paint(&mut encoder, screen_output.render_target_id);

                        if let Some(texture) = results.get(&node_id) {
                            let output_bind_group = self.device.create_bind_group(
                                &wgpu::BindGroupDescriptor {
                                    layout: &self.bind_group_layout,
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: wgpu::BindingResource::TextureView(&texture.view),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::Sampler(&texture.sampler),
                                        }
                                    ],
                                    label: Some("output bind group"),
                                }
                            );

                            // Record output render pass.
                            let output = screen_output.surface.get_current_texture().unwrap();
                            let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

                            {
                                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("Output window render pass"),
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                        view: &view,
                                        resolve_target: None,
                                        ops: wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                                r: 0.,
                                                g: 0.,
                                                b: 0.,
                                                a: 0.,
                                            }),
                                            store: true,
                                        },
                                    })],
                                    depth_stencil_attachment: None,
                                });

                                render_pass.set_pipeline(&screen_output.render_pipeline);
                                render_pass.set_bind_group(0, &output_bind_group, &[]);
                                render_pass.draw(0..4, 0..1);
                            }

                            // Submit the commands.
                            self.queue.submit(iter::once(encoder.finish()));

                            // Draw
                            output.present();
                        }
                    }
                    return true;
                }
                Event::WindowEvent {
                    ref event,
                    window_id,
                } if window_id == &screen_output.window.id() => {
                    match event {
                        WindowEvent::Resized(physical_size) => {
                            let output_size = *physical_size;
                            screen_output.resize(&self.device, output_size);
                        }
                        WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                            let output_size = **new_inner_size;
                            screen_output.resize(&self.device, output_size);
                        }
                        _ => {}
                    }
                    return true;
                }
                Event::MainEventsCleared => {
                    screen_output.window.request_redraw();
                }
                _ => {}
            }
        }
        false
    }
}
