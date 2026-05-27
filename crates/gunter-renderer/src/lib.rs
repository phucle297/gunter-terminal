mod atlas;
pub use atlas::GlyphAtlas;

use std::sync::Arc;
use gunter_core::grid::{Grid, Color, CellFlags};
use wgpu::util::DeviceExt;
use winit::window::Window;

/// Default foreground: Atom One Dark #abb2bf
const DEFAULT_FG: Color = Color { r: 171, g: 178, b: 191 };
/// Default background: Atom One Dark #282c34
const DEFAULT_BG: Color = Color { r: 40, g: 44, b: 52 };

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
}

const VERTICES: &[Vertex] = &[
    Vertex { pos: [0.0, 0.0] },
    Vertex { pos: [1.0, 0.0] },
    Vertex { pos: [1.0, 1.0] },
    Vertex { pos: [0.0, 1.0] },
];

const INDICES: &[u16] = &[0, 1, 2, 0, 2, 3];

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CellInstance {
    cell_pos: [f32; 2],
    bg: [f32; 3],
    fg: [f32; 3],
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    flags: u32,   // bit 0 = underline, bit 1 = wide (2-cell width), bit 2 = skip (spacer)
    _pad: u32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    cell_size: [f32; 2],
}

pub struct GunterRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    atlas: GlyphAtlas,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
    instances: Vec<CellInstance>,
    last_cursor: (u16, u16),
}

impl GunterRenderer {
    pub async fn new(window: Arc<Window>, cols: u16, rows: u16, font_size: f32) -> Self {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface: wgpu::Surface<'static> = instance.create_surface(window).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no wgpu adapter — ensure GPU/Vulkan drivers are available");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .expect("no wgpu device");

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Embedded fallback — never panics (JetBrains Mono, SIL OFL)
        static EMBEDDED_FONT: &[u8] =
            include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

        #[cfg(target_os = "windows")]
        let system_candidates: &[&str] = &[
            r"C:\Windows\Fonts\consola.ttf",
            r"C:\Windows\Fonts\cour.ttf",
            r"C:\Windows\Fonts\lucon.ttf",
        ];
        #[cfg(not(target_os = "windows"))]
        let system_candidates: &[&str] = &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf",
            "/usr/share/fonts/noto/NotoMono-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        ];

        let font_bytes: std::borrow::Cow<[u8]> = system_candidates
            .iter()
            .find_map(|p| std::fs::read(p).ok())
            .map(std::borrow::Cow::Owned)
            .unwrap_or(std::borrow::Cow::Borrowed(EMBEDDED_FONT));

        let atlas = GlyphAtlas::build(&font_bytes, font_size);

        let cell_w = atlas.cell_w as f32;
        let cell_h = atlas.cell_h as f32;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cell_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width),
                rows_per_image: Some(atlas.height),
            },
            wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
        );
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas_bgl"),
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
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&uniform_bgl, &atlas_bgl],
            push_constant_ranges: &[],
        });

        let instance_attrs = wgpu::vertex_attr_array![
            1 => Float32x2,
            2 => Float32x3,
            3 => Float32x3,
            4 => Float32x2,
            5 => Float32x2,
            6 => Uint32
        ];
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cell_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CellInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attrs,
                    },
                ],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertex_buf"),
            contents: bytemuck::cast_slice(VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("index_buf"),
            contents: bytemuck::cast_slice(INDICES),
            usage: wgpu::BufferUsages::INDEX,
        });
        let instance_count = (cols as usize) * (rows as usize);
        let instances: Vec<CellInstance> = vec![CellInstance {
            cell_pos: [0.0; 2],
            bg: [40.0 / 255.0, 44.0 / 255.0, 52.0 / 255.0],
            fg: [171.0 / 255.0, 178.0 / 255.0, 191.0 / 255.0],
            uv_min: [0.0; 2],
            uv_max: [0.0; 2],
            flags: 0,
            _pad: 0,
        }; instance_count];
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buf"),
            size: (std::mem::size_of::<CellInstance>() * instance_count) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniform_buf"),
            contents: bytemuck::bytes_of(&Uniforms {
                viewport: [config.width as f32, config.height as f32],
                cell_size: [cell_w, cell_h],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
            label: None,
        });
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &atlas_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
            label: None,
        });

        GunterRenderer {
            surface,
            device,
            queue,
            config,
            render_pipeline,
            vertex_buf,
            index_buf,
            instance_buf,
            uniform_buf,
            uniform_bind_group,
            atlas_texture,
            atlas_bind_group,
            atlas,
            cell_w,
            cell_h,
            cols,
            rows,
            instances,
            last_cursor: (0, 0),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [width as f32, height as f32],
                cell_size: [self.cell_w, self.cell_h],
            }),
        );
    }

    pub fn render(&mut self, grid: &mut Grid) {
        use gunter_core::grid::CursorStyle;

        // Rebuild instance buffer when grid dimensions change.
        let grid_count = grid.cols as usize * grid.rows as usize;
        if grid_count != self.instances.len() {
            self.cols = grid.cols;
            self.rows = grid.rows;
            self.instances = vec![CellInstance {
                cell_pos: [0.0; 2],
                bg: [DEFAULT_BG.r as f32 / 255.0, DEFAULT_BG.g as f32 / 255.0, DEFAULT_BG.b as f32 / 255.0],
                fg: [DEFAULT_FG.r as f32 / 255.0, DEFAULT_FG.g as f32 / 255.0, DEFAULT_FG.b as f32 / 255.0],
                uv_min: [0.0; 2],
                uv_max: [0.0; 2],
                flags: 0,
                _pad: 0,
            }; grid_count];
            self.instance_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instance_buf"),
                size: (std::mem::size_of::<CellInstance>() * grid_count) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        let instance_count = self.instances.len() as u32;

        if grid.scroll_offset > 0 {
            let offset = grid.scroll_offset;
            let sb_len = grid.scrollback.len();
            let cols = self.cols as usize;
            for dr in 0..(self.rows as usize) {
                for col in 0..cols {
                    let display_idx = dr * cols + col;
                    let cell = if dr < offset {
                        let sb_pos_opt = sb_len.checked_sub(offset - dr);
                        sb_pos_opt
                            .and_then(|p| grid.scrollback.get(p))
                            .and_then(|row| row.get(col))
                            .copied()
                            .unwrap_or_else(gunter_core::grid::Cell::blank)
                    } else {
                        let live_row = dr - offset;
                        let live_idx = live_row * cols + col;
                        grid.cells.get(live_idx).copied()
                            .unwrap_or_else(gunter_core::grid::Cell::blank)
                    };
                    let inst = build_instance(&mut self.atlas, &cell, col as f32, dr as f32);
                    self.instances[display_idx] = inst;
                }
            }
            self.queue.write_buffer(
                &self.instance_buf,
                0,
                bytemuck::cast_slice(&self.instances),
            );
            flush_dynamic_atlas(&self.atlas_texture, &mut self.atlas, &self.queue);
            grid.clear_dirty();
            self.last_cursor = (u16::MAX, u16::MAX);
        } else {
            let cursor_moved = self.last_cursor != grid.cursor;
            let mut changed = cursor_moved;

            // Update dirty cells
            for row in 0..self.rows {
                for col in 0..self.cols {
                    let idx = (row as usize) * (self.cols as usize) + (col as usize);
                    if grid.dirty[idx] {
                        let cell = grid.cells[idx];
                        self.instances[idx] = build_instance(&mut self.atlas, &cell, col as f32, row as f32);
                        changed = true;
                    }
                }
            }

            // Restore previous cursor cell to normal colours (prevent ghost)
            if cursor_moved {
                let (px, py) = self.last_cursor;
                if px != u16::MAX && py != u16::MAX {
                    let pidx = py as usize * self.cols as usize + px as usize;
                    if pidx < self.instances.len() {
                        let pcell = grid.cells[pidx];
                        self.instances[pidx] = build_instance(&mut self.atlas, &pcell, px as f32, py as f32);
                    }
                }
            }

            // Render cursor — override cursor cell (Block: swap fg/bg with cursor colour)
            if grid.cursor_visible {
                let (cx, cy) = grid.cursor;
                let idx = cy as usize * self.cols as usize + cx as usize;
                if idx < self.instances.len() {
                    let cell = &grid.cells[idx];
                    let (uv_min, uv_max) = if cell.wide {
                        self.atlas.uv_for_wide_char(cell.ch)
                    } else {
                        self.atlas.uv_for_char(cell.ch)
                    };
                    // Atom One Dark cursor #528bff = rgb(82, 139, 255)
                    let cursor_bg = [82.0 / 255.0, 139.0 / 255.0, 1.0f32];
                    let _ = CursorStyle::Block;
                    let cell_bg = cell.bg.resolve(DEFAULT_BG);
                    let mut flags = 0u32;
                    if cell.flags.contains(CellFlags::UNDERLINE) { flags |= 1; }
                    if cell.wide { flags |= 2; }
                    self.instances[idx] = CellInstance {
                        cell_pos: [cx as f32, cy as f32],
                        bg: cursor_bg,
                        fg: [
                            cell_bg.r as f32 / 255.0,
                            cell_bg.g as f32 / 255.0,
                            cell_bg.b as f32 / 255.0,
                        ],
                        uv_min,
                        uv_max,
                        flags,
                        _pad: 0,
                    };
                    changed = true;
                }
                self.last_cursor = grid.cursor;
            }

            if changed {
                self.queue.write_buffer(
                    &self.instance_buf,
                    0,
                    bytemuck::cast_slice(&self.instances),
                );
            }

            flush_dynamic_atlas(&self.atlas_texture, &mut self.atlas, &self.queue);
            grid.clear_dirty();
        }

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(e) => {
                eprintln!("surface error: {e:?}");
                return;
            }
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 40.0 / 255.0,
                            g: 44.0 / 255.0,
                            b: 52.0 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.render_pipeline);
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_bind_group(1, &self.atlas_bind_group, &[]);
            rp.set_vertex_buffer(0, self.vertex_buf.slice(..));
            rp.set_vertex_buffer(1, self.instance_buf.slice(..));
            rp.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..6, 0, 0..instance_count);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }

    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_w, self.cell_h)
    }
}

fn build_instance(atlas: &mut GlyphAtlas, cell: &gunter_core::grid::Cell, col: f32, row: f32) -> CellInstance {
    let (uv_min, uv_max) = if cell.wide {
        atlas.uv_for_wide_char(cell.ch)
    } else {
        atlas.uv_for_char(cell.ch)
    };
    let mut fg_resolved = cell.fg.resolve(DEFAULT_FG);
    let mut bg_resolved = cell.bg.resolve(DEFAULT_BG);
    if cell.flags.contains(CellFlags::INVERSE) {
        std::mem::swap(&mut fg_resolved, &mut bg_resolved);
    }
    let mut flags = 0u32;
    if cell.flags.contains(CellFlags::UNDERLINE) { flags |= 1; }
    if cell.wide { flags |= 2; }
    if cell.wide_spacer { flags |= 4; }
    CellInstance {
        cell_pos: [col, row],
        bg: [bg_resolved.r as f32 / 255.0, bg_resolved.g as f32 / 255.0, bg_resolved.b as f32 / 255.0],
        fg: [fg_resolved.r as f32 / 255.0, fg_resolved.g as f32 / 255.0, fg_resolved.b as f32 / 255.0],
        uv_min,
        uv_max,
        flags,
        _pad: 0,
    }
}

fn flush_dynamic_atlas(atlas_texture: &wgpu::Texture, atlas: &mut GlyphAtlas, queue: &wgpu::Queue) {
    if !atlas.dynamic_dirty { return; }
    let dyn_y = atlas.dyn_pixel_y;
    let dyn_h = atlas.height - dyn_y;
    let dyn_offset = (dyn_y * atlas.width) as usize;
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: dyn_y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        &atlas.data[dyn_offset..],
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(atlas.width),
            rows_per_image: Some(dyn_h),
        },
        wgpu::Extent3d {
            width: atlas.width,
            height: dyn_h,
            depth_or_array_layers: 1,
        },
    );
    atlas.dynamic_dirty = false;
}
