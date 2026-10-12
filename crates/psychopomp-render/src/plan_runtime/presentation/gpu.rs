//! Native window presentation only. Scene pixels still come from the existing
//! renderer; the GPU replaces CPU rescaling and window-buffer conversion.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use winit::window::Window;

use super::{Filter, viewport};
use crate::exposure::{HEIGHT, WIDTH};

pub(super) enum Paint {
    Presented(Timing),
    Retry,
    Occluded,
}

pub(super) struct Timing {
    pub prepare: Duration,
    pub present: Duration,
    pub acquire: Duration,
    /// Upload + encoding + GPU completion, excluding waiting for a drawable.
    /// Only measured in --benchmark-gpu; normal playback never waits here.
    pub completed_work: Option<Duration>,
}

pub(super) struct Presenter {
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    texture: wgpu::Texture,
    smooth: wgpu::BindGroup,
    pixelated: wgpu::BindGroup,
    uploaded: Option<Arc<Vec<u8>>>,
    window: Arc<Window>,
    reconfigure: bool,
}

impl Presenter {
    pub(super) async fn new(window: Arc<Window>) -> Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window.clone())
            .context("create native GPU surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .context("request native display adapter")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("psychopomp native display"),
                ..Default::default()
            })
            .await
            .context("create native display device")?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("native surface configuration")?;
        config.format = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(wgpu::TextureFormat::is_srgb)
            .context("native display requires an sRGB surface")?;
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);
        let shader = device.create_shader_module(wgpu::include_wgsl!("present.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native frame presentation"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sampled scene pixels"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = |filter| {
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native frame filter"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let smooth = binding(wgpu::FilterMode::Linear);
        let pixelated = binding(wgpu::FilterMode::Nearest);
        Ok(Self {
            instance,
            surface,
            device,
            queue,
            config,
            pipeline,
            texture,
            smooth,
            pixelated,
            uploaded: None,
            window,
            reconfigure: false,
        })
    }

    pub(super) fn paint(
        &mut self,
        pixels: Option<&Arc<Vec<u8>>>,
        filter: Filter,
        measure_completion: bool,
    ) -> Result<Paint> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(Paint::Occluded);
        }
        if self.reconfigure || self.config.width != size.width || self.config.height != size.height
        {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
            self.reconfigure = false;
        }
        let started = Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                self.reconfigure = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Timeout => return Ok(Paint::Retry),
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(Paint::Occluded),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure = true;
                return Ok(Paint::Retry);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.window.clone())?;
                self.reconfigure = true;
                return Ok(Paint::Retry);
            }
            wgpu::CurrentSurfaceTexture::Validation => bail!("native surface validation failed"),
        };
        let acquire = started.elapsed();
        let upload_started = Instant::now();
        if let Some(pixels) = pixels
            && self
                .uploaded
                .as_ref()
                .is_none_or(|previous| !Arc::ptr_eq(previous, pixels))
        {
            if pixels.len() != WIDTH as usize * HEIGHT as usize * 4 {
                bail!("native frame has invalid RGBA dimensions");
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(WIDTH * 4),
                    rows_per_image: Some(HEIGHT),
                },
                wgpu::Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
            );
            self.uploaded = Some(pixels.clone());
        }
        let prepare = upload_started.elapsed();
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("present native frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.uploaded.is_some() {
                let [x, y, width, height] = viewport([size.width, size.height]);
                pass.set_viewport(x as f32, y as f32, width as f32, height as f32, 0., 1.);
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(
                    0,
                    match filter {
                        Filter::Smooth => &self.smooth,
                        Filter::Pixelated => &self.pixelated,
                    },
                    &[],
                );
                pass.draw(0..3, 0..1);
            }
        }
        let submission = self.queue.submit([encoder.finish()]);
        let completed_work = if measure_completion {
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(Duration::from_secs(5)),
                })
                .context("wait for benchmark GPU completion")?;
            Some(upload_started.elapsed())
        } else {
            None
        };
        self.window.pre_present_notify();
        self.queue.present(frame);
        Ok(Paint::Presented(Timing {
            prepare,
            present: started.elapsed().saturating_sub(prepare),
            acquire,
            completed_work,
        }))
    }
}
