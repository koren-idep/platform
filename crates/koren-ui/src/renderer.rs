//! GPU-accelerated batched `wgpu` renderer with `cosmic-text` glyph atlas.

use crate::draw::{DrawList, DrawOp, FontWeight, QuadCommand, Rect, TextCommand};
use crate::theme::Color;
use bytemuck::{Pod, Zeroable};
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight,
};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

const ATLAS_SIZE: u32 = 2048;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadInstance {
    pos_size: [f32; 4],
    color: [f32; 4],
    border_color: [f32; 4],
    params: [f32; 4],
    shadow_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TextInstance {
    pos_size: [f32; 4],
    uv_rect: [f32; 4],
    color: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct GlyphEntry {
    uv: [f32; 4],
    width: u32,
    height: u32,
    left: i32,
    top: i32,
}

struct GlyphAtlas {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    entries: HashMap<CacheKey, GlyphEntry>,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
}

impl GlyphAtlas {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("koren-ui-glyph-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("koren-ui-glyph-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("koren-ui-glyph-bg"),
            layout,
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
        });
        Self {
            texture,
            bind_group,
            entries: HashMap::with_capacity(1024),
            cursor_x: 1,
            cursor_y: 1,
            row_height: 0,
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.cursor_x = 1;
        self.cursor_y = 1;
        self.row_height = 0;
    }

    fn insert(
        &mut self,
        queue: &wgpu::Queue,
        key: CacheKey,
        width: u32,
        height: u32,
        left: i32,
        top: i32,
        data: &[u8],
    ) -> Option<GlyphEntry> {
        if width == 0 || height == 0 {
            let entry = GlyphEntry {
                uv: [0.0; 4],
                width: 0,
                height: 0,
                left,
                top,
            };
            self.entries.insert(key, entry);
            return Some(entry);
        }
        if width + 2 >= ATLAS_SIZE || height + 2 >= ATLAS_SIZE {
            return None;
        }
        if self.cursor_x + width + 1 >= ATLAS_SIZE {
            self.cursor_x = 1;
            self.cursor_y += self.row_height + 1;
            self.row_height = 0;
        }
        if self.cursor_y + height + 1 >= ATLAS_SIZE {
            self.clear();
        }
        let x = self.cursor_x;
        let y = self.cursor_y;
        self.cursor_x += width + 1;
        self.row_height = self.row_height.max(height);

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let inv = 1.0 / ATLAS_SIZE as f32;
        let entry = GlyphEntry {
            uv: [
                x as f32 * inv,
                y as f32 * inv,
                (x + width) as f32 * inv,
                (y + height) as f32 * inv,
            ],
            width,
            height,
            left,
            top,
        };
        self.entries.insert(key, entry);
        Some(entry)
    }
}

/// Text shaping and measurement engine backed by `cosmic-text`.
pub struct TextEngine {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl TextEngine {
    /// Creates a new text engine loading system fonts.
    #[must_use]
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Measures the single-line width in logical pixels of `text`.
    pub fn measure_width(
        &mut self,
        text: &str,
        size: f32,
        weight: FontWeight,
        monospace: bool,
    ) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let metrics = Metrics::new(size, size * 1.35);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, None, None);
        let attrs = make_attrs(weight, monospace, false);
        buffer.set_text(&mut self.font_system, text, &attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
            .layout_runs()
            .map(|r| r.line_w)
            .fold(0.0_f32, f32::max)
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn make_attrs(weight: FontWeight, monospace: bool, italic: bool) -> Attrs<'static> {
    let mut attrs = Attrs::new();
    attrs = attrs.family(if monospace {
        Family::Monospace
    } else {
        Family::SansSerif
    });
    attrs = attrs.weight(match weight {
        FontWeight::Regular => Weight::NORMAL,
        FontWeight::Medium => Weight::MEDIUM,
        FontWeight::SemiBold => Weight::SEMIBOLD,
        FontWeight::Bold => Weight::BOLD,
    });
    if italic {
        attrs = attrs.style(Style::Italic);
    }
    attrs
}

enum BatchKind {
    Quad {
        scissor: [u32; 4],
        range: std::ops::Range<u32>,
    },
    Text {
        scissor: [u32; 4],
        range: std::ops::Range<u32>,
    },
}

/// GPU renderer managing the `wgpu` surface, pipelines, and batched frame submission.
pub struct WgpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    uniform_buffer: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    quad_pipeline: wgpu::RenderPipeline,
    text_pipeline: wgpu::RenderPipeline,
    atlas: GlyphAtlas,
}

impl WgpuRenderer {
    /// Initializes `wgpu` for the given `window`.
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window)
            .map_err(|e| format!("failed to create wgpu surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| format!("no compatible GPU adapter found: {e}"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("koren-ui-device"),
            ..Default::default()
        }))
        .map_err(|e| format!("failed to create GPU device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        // Prefer 8-bit non-sRGB format so theme hex colors match screen pixels 1:1.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .unwrap_or(caps.formats[0]);

        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("koren-ui-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniforms = Uniforms {
            viewport: [config.width as f32, config.height as f32],
            _pad: [0.0; 2],
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("koren-ui-uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("koren-ui-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("koren-ui-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let glyph_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("koren-ui-glyph-bgl"),
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
            ],
        });

        let quad_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("koren-ui-quad-layout"),
            bind_group_layouts: &[&uniform_bgl],
            push_constant_ranges: &[],
        });

        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("koren-ui-quad-pipeline"),
            layout: Some(&quad_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_quad"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<QuadInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Float32x4,
                        3 => Float32x4,
                        4 => Float32x4
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_quad"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let text_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("koren-ui-text-layout"),
            bind_group_layouts: &[&uniform_bgl, &glyph_bgl],
            push_constant_ranges: &[],
        });

        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("koren-ui-text-pipeline"),
            layout: Some(&text_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_text"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TextInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Float32x4
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_text"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let atlas = GlyphAtlas::new(&device, &glyph_bgl);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            uniform_buffer,
            uniform_bg,
            quad_pipeline,
            text_pipeline,
            atlas,
        })
    }

    /// Resizes the GPU swapchain surface.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        let uniforms = Uniforms {
            viewport: [width as f32, height as f32],
            _pad: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Renders `draw_list` scaled by `scale` onto the swapchain and presents.
    pub fn render(
        &mut self,
        clear_color: Color,
        draw_list: &DrawList,
        text_engine: &mut TextEngine,
        scale: f32,
    ) -> Result<(), String> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(e) => return Err(format!("surface acquire failed: {e}")),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut quads: Vec<QuadInstance> = Vec::with_capacity(256);
        let mut texts: Vec<TextInstance> = Vec::with_capacity(1024);
        let mut batches: Vec<BatchKind> = Vec::with_capacity(32);

        let full_scissor = [0, 0, self.config.width, self.config.height];

        for op in draw_list.iter_ops() {
            match op {
                DrawOp::Quad(q) => {
                    let scissor =
                        clip_to_scissor(q.clip, scale, self.config.width, self.config.height)
                            .unwrap_or(full_scissor);
                    if scissor[2] == 0 || scissor[3] == 0 {
                        continue;
                    }
                    let idx = quads.len() as u32;
                    quads.push(quad_to_instance(q, scale));
                    match batches.last_mut() {
                        Some(BatchKind::Quad { scissor: s, range }) if *s == scissor => {
                            range.end = idx + 1;
                        }
                        _ => batches.push(BatchKind::Quad {
                            scissor,
                            range: idx..idx + 1,
                        }),
                    }
                }
                DrawOp::Text(t) => {
                    let scissor =
                        clip_to_scissor(t.clip, scale, self.config.width, self.config.height)
                            .unwrap_or(full_scissor);
                    if scissor[2] == 0 || scissor[3] == 0 {
                        continue;
                    }
                    let start = texts.len() as u32;
                    self.shape_and_emit_text(t, scale, text_engine, &mut texts);
                    let end = texts.len() as u32;
                    if end > start {
                        match batches.last_mut() {
                            Some(BatchKind::Text { scissor: s, range }) if *s == scissor => {
                                range.end = end;
                            }
                            _ => batches.push(BatchKind::Text {
                                scissor,
                                range: start..end,
                            }),
                        }
                    }
                }
            }
        }

        let quad_buf = if quads.is_empty() {
            None
        } else {
            Some(
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("koren-ui-quad-instances"),
                        contents: bytemuck::cast_slice(&quads),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
            )
        };
        let text_buf = if texts.is_empty() {
            None
        } else {
            Some(
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("koren-ui-text-instances"),
                        contents: bytemuck::cast_slice(&texts),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
            )
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("koren-ui-frame-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("koren-ui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear_color.r),
                            g: f64::from(clear_color.g),
                            b: f64::from(clear_color.b),
                            a: f64::from(clear_color.a),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            for batch in batches {
                match batch {
                    BatchKind::Quad { scissor, range } => {
                        if let Some(ref buf) = quad_buf {
                            pass.set_pipeline(&self.quad_pipeline);
                            pass.set_bind_group(0, &self.uniform_bg, &[]);
                            pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                            pass.set_vertex_buffer(0, buf.slice(..));
                            pass.draw(0..6, range);
                        }
                    }
                    BatchKind::Text { scissor, range } => {
                        if let Some(ref buf) = text_buf {
                            pass.set_pipeline(&self.text_pipeline);
                            pass.set_bind_group(0, &self.uniform_bg, &[]);
                            pass.set_bind_group(1, &self.atlas.bind_group, &[]);
                            pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                            pass.set_vertex_buffer(0, buf.slice(..));
                            pass.draw(0..6, range);
                        }
                    }
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    fn shape_and_emit_text(
        &mut self,
        cmd: &TextCommand,
        scale: f32,
        engine: &mut TextEngine,
        out: &mut Vec<TextInstance>,
    ) {
        if cmd.text.is_empty() {
            return;
        }
        let metrics = Metrics::new(cmd.size * scale, cmd.line_height * scale);
        let mut buffer = Buffer::new(&mut engine.font_system, metrics);
        buffer.set_size(
            &mut engine.font_system,
            cmd.max_width.map(|w| w * scale),
            None,
        );
        let attrs = make_attrs(cmd.weight, cmd.monospace, cmd.italic);
        buffer.set_text(
            &mut engine.font_system,
            &cmd.text,
            &attrs,
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut engine.font_system, false);

        let origin_x = (cmd.pos.x * scale).round();
        let origin_y = (cmd.pos.y * scale).round();
        let color = cmd.color.to_array();

        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((origin_x, origin_y), 1.0);
                let entry = if let Some(cached) = self.atlas.entries.get(&physical.cache_key) {
                    *cached
                } else {
                    let Some(image) = engine
                        .swash_cache
                        .get_image_uncached(&mut engine.font_system, physical.cache_key)
                    else {
                        continue;
                    };
                    let w = image.placement.width;
                    let h = image.placement.height;
                    let data: Vec<u8> = match image.content {
                        cosmic_text::SwashContent::Mask => image.data,
                        cosmic_text::SwashContent::SubpixelMask => image
                            .data
                            .chunks_exact(4)
                            .map(|c| {
                                ((u16::from(c[0]) + u16::from(c[1]) + u16::from(c[2])) / 3) as u8
                            })
                            .collect(),
                        cosmic_text::SwashContent::Color => {
                            image.data.chunks_exact(4).map(|c| c[3]).collect()
                        }
                    };
                    let Some(inserted) = self.atlas.insert(
                        &self.queue,
                        physical.cache_key,
                        w,
                        h,
                        image.placement.left,
                        image.placement.top,
                        &data,
                    ) else {
                        continue;
                    };
                    inserted
                };

                if entry.width == 0 || entry.height == 0 {
                    continue;
                }

                let gx = (physical.x + entry.left) as f32;
                let gy = (physical.y as f32 + run.line_y) - entry.top as f32;
                out.push(TextInstance {
                    pos_size: [gx, gy, entry.width as f32, entry.height as f32],
                    uv_rect: entry.uv,
                    color,
                });
            }
        }
    }
}

fn quad_to_instance(q: &QuadCommand, scale: f32) -> QuadInstance {
    QuadInstance {
        pos_size: [
            q.rect.x * scale,
            q.rect.y * scale,
            q.rect.width * scale,
            q.rect.height * scale,
        ],
        color: q.color.to_array(),
        border_color: q.border_color.to_array(),
        params: [
            q.corner_radius * scale,
            q.border_width * scale,
            q.shadow_blur * scale,
            0.0,
        ],
        shadow_color: q.shadow_color.to_array(),
    }
}

fn clip_to_scissor(clip: Option<Rect>, scale: f32, max_w: u32, max_h: u32) -> Option<[u32; 4]> {
    let r = clip?;
    let x = (r.x * scale).floor().max(0.0) as u32;
    let y = (r.y * scale).floor().max(0.0) as u32;
    let x1 = ((r.x + r.width) * scale).ceil().min(max_w as f32).max(0.0) as u32;
    let y1 = ((r.y + r.height) * scale).ceil().min(max_h as f32).max(0.0) as u32;
    if x >= max_w || y >= max_h || x1 <= x || y1 <= y {
        Some([0, 0, 0, 0])
    } else {
        Some([x, y, x1 - x, y1 - y])
    }
}
