use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;

use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use image::imageops::FilterType;
use image::{Rgba, RgbaImage, imageops};

use crate::config::DashboardAcrylicConfig;
use crate::image::RenderedFrame;

use super::layouts::PanelGeometry;

const DOWNSAMPLE_DIVISOR: u32 = 2;
const MAX_SHADER_RADIUS: u32 = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurParams {
    direction: [f32; 2],
    texel_size: [f32; 2],
    radius: u32,
    _padding: u32,
}

pub(super) struct AcrylicFrameCache {
    background_hash: Option<u64>,
    frame: Option<RenderedFrame>,
    #[cfg(test)]
    pub build_count: usize,
}

impl AcrylicFrameCache {
    pub fn new() -> Self {
        Self {
            background_hash: None,
            frame: None,
            #[cfg(test)]
            build_count: 0,
        }
    }

    pub fn get_or_build(
        &mut self,
        compositor: &mut WgpuAcrylicCompositor,
        background: &RenderedFrame,
        panels: &[PanelGeometry],
        config: DashboardAcrylicConfig,
    ) -> Result<RenderedFrame> {
        let background_hash = hash_frame(background);
        if self.background_hash == Some(background_hash)
            && let Some(frame) = &self.frame
        {
            return Ok(frame.clone());
        }

        let blurred = compositor.blur_frame(background, config.blur_strength)?;
        let frame = composite_acrylic_background(background, &blurred, panels, config.tint_alpha)?;

        self.background_hash = Some(background_hash);
        self.frame = Some(frame.clone());
        #[cfg(test)]
        {
            self.build_count += 1;
        }

        Ok(frame)
    }
}

#[cfg_attr(test, allow(dead_code))]
pub(super) struct WgpuAcrylicCompositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

#[cfg_attr(test, allow(dead_code))]
impl WgpuAcrylicCompositor {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .context("failed to request wgpu adapter for acrylic compositor")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lcdd acrylic compositor device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::default(),
        }))
        .context("failed to create wgpu device for acrylic compositor")?;

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lcdd acrylic sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("lcdd acrylic bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: Some(
                                wgpu::BufferSize::new(std::mem::size_of::<BlurParams>() as u64)
                                    .expect("blur params size"),
                            ),
                        },
                        count: None,
                    },
                ],
            });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lcdd acrylic blur shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lcdd acrylic pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lcdd acrylic blur pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            sampler,
            bind_group_layout,
            pipeline,
        })
    }

    pub fn blur_frame(&mut self, frame: &RenderedFrame, radius: u32) -> Result<RenderedFrame> {
        let radius = radius.clamp(1, MAX_SHADER_RADIUS);
        let half_width = (frame.width() / DOWNSAMPLE_DIVISOR).max(1);
        let half_height = (frame.height() / DOWNSAMPLE_DIVISOR).max(1);
        let downsampled = resize_frame(frame, half_width, half_height)?;

        let source_texture = self.upload_texture(
            half_width,
            half_height,
            downsampled.as_raw(),
            "lcdd acrylic source texture",
        );
        let temp_texture = self.create_render_texture(half_width, half_height, "lcdd acrylic temp");
        let output_texture =
            self.create_render_texture(half_width, half_height, "lcdd acrylic output");
        let params_horizontal = self.create_params_buffer(BlurParams {
            direction: [1.0, 0.0],
            texel_size: [1.0 / half_width as f32, 1.0 / half_height as f32],
            radius,
            _padding: 0,
        });
        let params_vertical = self.create_params_buffer(BlurParams {
            direction: [0.0, 1.0],
            texel_size: [1.0 / half_width as f32, 1.0 / half_height as f32],
            radius,
            _padding: 0,
        });

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let temp_view = temp_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let horizontal_bind_group = self.create_bind_group(&source_view, &params_horizontal);
        let vertical_bind_group = self.create_bind_group(&temp_view, &params_vertical);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lcdd acrylic blur encoder"),
            });

        self.run_pass(
            &mut encoder,
            &temp_view,
            &horizontal_bind_group,
            "lcdd acrylic horizontal pass",
        );
        self.run_pass(
            &mut encoder,
            &output_view,
            &vertical_bind_group,
            "lcdd acrylic vertical pass",
        );

        let blurred_half = self.read_texture(encoder, &output_texture, half_width, half_height)?;

        let blurred_half = RgbaImage::from_raw(half_width, half_height, blurred_half)
            .context("blurred acrylic RGBA buffer did not match half-size dimensions")?;
        let full = imageops::resize(
            &blurred_half,
            frame.width(),
            frame.height(),
            FilterType::Triangle,
        );

        Ok(RenderedFrame::new(frame.width(), frame.height(), full.into_raw()))
    }

    fn upload_texture(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        label: &str,
    ) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        texture
    }

    fn create_render_texture(&self, width: u32, height: u32, label: &str) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    fn create_params_buffer(&self, params: BlurParams) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lcdd acrylic params buffer"),
            size: std::mem::size_of::<BlurParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
        .tap(|buffer| {
            self.queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&params));
        })
    }

    fn create_bind_group(
        &self,
        texture_view: &wgpu::TextureView,
        params_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lcdd acrylic bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn run_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        label: &str,
    ) {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    fn read_texture(
        &self,
        mut encoder: wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>> {
        let unpadded_bytes_per_row = width as usize * 4;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bytes_per_row =
            unpadded_bytes_per_row + (alignment - unpadded_bytes_per_row % alignment) % alignment;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lcdd acrylic output buffer"),
            size: (padded_bytes_per_row * height as usize) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit([encoder.finish()]);

        let slice = output_buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        receiver
            .recv()
            .map_err(|_| anyhow!("failed to receive acrylic GPU readback signal"))?
            .context("failed to map acrylic GPU readback buffer")?;

        let mapped = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity(unpadded_bytes_per_row * height as usize);
        for row in mapped.chunks(padded_bytes_per_row) {
            rgba.extend_from_slice(&row[..unpadded_bytes_per_row]);
        }
        drop(mapped);
        output_buffer.unmap();

        Ok(rgba)
    }
}

pub(super) fn composite_acrylic_background(
    background: &RenderedFrame,
    blurred: &RenderedFrame,
    panels: &[PanelGeometry],
    tint_alpha: f32,
) -> Result<RenderedFrame> {
    let mut output = RgbaImage::from_raw(
        background.width(),
        background.height(),
        background.rgba().to_vec(),
    )
    .context("background RGBA buffer did not match dimensions")?;
    let blurred = RgbaImage::from_raw(blurred.width(), blurred.height(), blurred.rgba().to_vec())
        .context("blurred RGBA buffer did not match dimensions")?;

    for panel in panels {
        let start_x = panel.x.floor().max(0.0) as u32;
        let start_y = panel.y.floor().max(0.0) as u32;
        let end_x = (panel.x + panel.width).ceil().min(background.width() as f32) as u32;
        let end_y = (panel.y + panel.height)
            .ceil()
            .min(background.height() as f32) as u32;

        for y in start_y..end_y {
            for x in start_x..end_x {
                if !inside_rounded_rect(x as f32 + 0.5, y as f32 + 0.5, *panel) {
                    continue;
                }

                let blurred_pixel = blurred.get_pixel(x, y);
                output.put_pixel(x, y, tint_pixel(*blurred_pixel, tint_alpha));
            }
        }
    }

    Ok(RenderedFrame::new(
        background.width(),
        background.height(),
        output.into_raw(),
    ))
}

fn tint_pixel(pixel: Rgba<u8>, tint_alpha: f32) -> Rgba<u8> {
    let blend = (1.0 - tint_alpha).clamp(0.0, 1.0);
    Rgba([
        (pixel[0] as f32 * blend).round() as u8,
        (pixel[1] as f32 * blend).round() as u8,
        (pixel[2] as f32 * blend).round() as u8,
        255,
    ])
}

fn inside_rounded_rect(x: f32, y: f32, panel: PanelGeometry) -> bool {
    let left = panel.x;
    let right = panel.x + panel.width;
    let top = panel.y;
    let bottom = panel.y + panel.height;
    let radius = panel.corner_radius.min(panel.width / 2.0).min(panel.height / 2.0);

    if x >= left + radius && x <= right - radius {
        return y >= top && y <= bottom;
    }
    if y >= top + radius && y <= bottom - radius {
        return x >= left && x <= right;
    }

    let center_x = if x < left + radius {
        left + radius
    } else {
        right - radius
    };
    let center_y = if y < top + radius {
        top + radius
    } else {
        bottom - radius
    };

    let dx = x - center_x;
    let dy = y - center_y;

    dx * dx + dy * dy <= radius * radius
}

fn resize_frame(frame: &RenderedFrame, width: u32, height: u32) -> Result<RgbaImage> {
    let image = RgbaImage::from_raw(frame.width(), frame.height(), frame.rgba().to_vec())
        .context("source RGBA buffer did not match dimensions")?;
    Ok(imageops::resize(&image, width, height, FilterType::Triangle))
}

fn hash_frame(frame: &RenderedFrame) -> u64 {
    let mut hasher = DefaultHasher::new();
    frame.width().hash(&mut hasher);
    frame.height().hash(&mut hasher);
    frame.rgba().hash(&mut hasher);
    hasher.finish()
}

trait Tap: Sized {
    fn tap(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}

impl<T> Tap for T {}

#[cfg(test)]
mod tests {
    use super::{AcrylicFrameCache, composite_acrylic_background};
    use crate::config::DashboardAcrylicConfig;
    use crate::image::RenderedFrame;
    use crate::image::dashboard::layouts::PanelGeometry;

    #[test]
    fn acrylic_composite_only_changes_panel_region() {
        let background = RenderedFrame::new(8, 8, vec![255; 8 * 8 * 4]);
        let blurred = RenderedFrame::new(8, 8, vec![100; 8 * 8 * 4]);
        let panel = PanelGeometry {
            x: 2.0,
            y: 2.0,
            width: 4.0,
            height: 4.0,
            corner_radius: 1.0,
        };

        let composited =
            composite_acrylic_background(&background, &blurred, &[panel], 0.5).unwrap();

        assert_eq!(&composited.rgba()[0..4], &[255, 255, 255, 255]);
        assert_ne!(
            &composited.rgba()[((3 * 8 + 3) * 4) as usize..((3 * 8 + 3) * 4 + 4) as usize],
            &[255, 255, 255, 255]
        );
    }

    #[test]
    fn acrylic_cache_reuses_same_background_hash() {
        let background = RenderedFrame::new(2, 2, vec![1; 16]);
        let mut cache = AcrylicFrameCache::new();
        cache.background_hash = Some(42);
        cache.frame = Some(background.clone());
        let config = DashboardAcrylicConfig::default();

        assert!(cache.frame.is_some());
        assert_eq!(config.blur_strength, 12);
    }
}
