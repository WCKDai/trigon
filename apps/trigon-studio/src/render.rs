//! The 3D viewport on the GPU: the scene is drawn with 4x MSAA (when the adapter offers it) into
//! a texture of the app's own,
//! which egui then shows as an image. Drawing offscreen keeps egui's own surface at one sample
//! (no sample-count matching with egui-wgpu) and gives `ui.screenshot` the same pixels.
//!
//! Passes: opaque faces (pushed back a little with a depth bias), feature edges as screen-space
//! quads, translucent faces (glass, demolished), then the change highlight, faint through
//! everything and strong where it is visible.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use wgpu::util::DeviceExt;

use crate::scene::{EDGE_FLOATS, Scene, VERTEX_FLOATS};

/// Samples per pixel wanted; adapters that can't (some OpenGL ones) draw with one.
const MSAA: u32 = 4;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const EGUI_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Largest offscreen image, pixels per side.
const MAX_SIDE: u32 = 4096;
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.79, g: 0.83, b: 0.86, a: 1.0 };

const SHADER: &str = r#"
struct U {
    view_proj: mat4x4<f32>,
    light: vec4<f32>,
    // x: cut height, y: cut on, z: alpha scale, w: cut faces dark
    clip: vec4<f32>,
    // x, y: size in pixels, z: half the edge width in pixels
    viewport: vec4<f32>,
    edge: vec4<f32>,
    eye: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_face(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = u.view_proj * vec4<f32>(p, 1.0);
    o.normal = n;
    o.color = c;
    o.world = p;
    return o;
}

@fragment
fn fs_face(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (u.clip.y > 0.5 && i.world.z > u.clip.x) {
        discard;
    }
    if (u.clip.y > 0.5 && u.clip.w > 0.5 && !front) {
        // Looking into a cut wall: its inside reads as the section.
        return vec4<f32>(0.13, 0.12, 0.12, 1.0);
    }
    var n = normalize(i.normal);
    if (dot(n, u.eye.xyz - i.world) < 0.0) {
        n = -n;
    }
    let l1 = normalize(u.light.xyz);
    let l2 = normalize(vec3<f32>(-l1.x, -l1.y, 0.5));
    let diffuse = max(dot(n, l1), 0.0) * 0.60 + max(dot(n, l2), 0.0) * 0.20;
    let ambient = u.light.w * (0.80 + 0.20 * n.z);
    return vec4<f32>(i.color.rgb * (ambient + diffuse), i.color.a * u.clip.z);
}

@fragment
fn fs_flat(i: VOut) -> @location(0) vec4<f32> {
    if (u.clip.y > 0.5 && i.world.z > u.clip.x) {
        discard;
    }
    return vec4<f32>(i.color.rgb, i.color.a * u.clip.z);
}

struct EOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) z: f32,
};

@vertex
fn vs_edge(@location(0) a: vec3<f32>, @location(1) b: vec3<f32>, @location(2) corner: vec2<f32>) -> EOut {
    var o: EOut;
    var ca = u.view_proj * vec4<f32>(a, 1.0);
    var cb = u.view_proj * vec4<f32>(b, 1.0);
    let near = 0.0001;
    if (ca.w < near && cb.w < near) {
        o.pos = vec4<f32>(0.0, 0.0, -2.0, 1.0);
        o.z = 0.0;
        return o;
    }
    if (ca.w < near) {
        ca = mix(ca, cb, (near - ca.w) / (cb.w - ca.w));
    }
    if (cb.w < near) {
        cb = mix(cb, ca, (near - cb.w) / (ca.w - cb.w));
    }
    let size = u.viewport.xy;
    var dir = (cb.xy / cb.w - ca.xy / ca.w) * size;
    let len = length(dir);
    if (len < 0.000001) {
        dir = vec2<f32>(1.0, 0.0);
    } else {
        dir = dir / len;
    }
    let side = vec2<f32>(-dir.y, dir.x) * corner.y * u.viewport.z * 2.0 / size;
    let base = select(ca, cb, corner.x > 0.5);
    o.pos = vec4<f32>(base.xy + side * base.w, base.z, base.w);
    o.z = select(a.z, b.z, corner.x > 0.5);
    return o;
}

@fragment
fn fs_edge(i: EOut) -> @location(0) vec4<f32> {
    if (u.clip.y > 0.5 && i.z > u.clip.x + 0.0005) {
        discard;
    }
    return u.edge;
}
"#;

/// What one frame of the viewport shows.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub view_proj: [f32; 16],
    pub eye: [f32; 3],
    pub clip_z: Option<f32>,
    /// Pixels.
    pub size: [u32; 2],
    /// Half the edge width, pixels.
    pub half_line: f32,
    /// The change highlight's strength, 0..1.
    pub highlight: f32,
    pub cut_faces_dark: bool,
}

struct Targets {
    size: [u32; 2],
    /// The multisampled colour target, or `None` when drawing straight into `resolve`.
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    /// The same pixels read without sRGB decoding: egui's shader takes texture values as
    /// gamma-encoded.
    egui_view: wgpu::TextureView,
}

impl Targets {
    fn new(device: &wgpu::Device, size: [u32; 2], samples: u32) -> Targets {
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let tex = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: if samples == 1 { &[EGUI_FORMAT] } else { &[] },
            })
        };
        let msaa = (samples > 1).then(|| tex("trigon viewport msaa", COLOR, samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
        let depth = tex("trigon viewport depth", DEPTH, samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = tex(
            "trigon viewport",
            COLOR,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let egui_view = resolve.create_view(&wgpu::TextureViewDescriptor { format: Some(EGUI_FORMAT), ..Default::default() });
        Targets { size, msaa: msaa.as_ref().map(view), depth: view(&depth), resolve_view: view(&resolve), egui_view, resolve }
    }
}

struct Geometry {
    buffer: wgpu::Buffer,
    count: u32,
}

fn upload(device: &wgpu::Device, label: &str, data: &[f32], floats_per_vertex: usize) -> Option<Geometry> {
    // A model larger than the device takes is drawn in part rather than not at all.
    let max = (device.limits().max_buffer_size / 4) as usize;
    let len = data.len().min(max) / floats_per_vertex * floats_per_vertex;
    let data = data.get(..len)?;
    if data.is_empty() {
        return None;
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some(Geometry { buffer, count: (len / floats_per_vertex) as u32 })
}

pub struct Viewport3d {
    faces: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    edges: wgpu::RenderPipeline,
    highlight: wgpu::RenderPipeline,
    xray: wgpu::RenderPipeline,
    uniforms: [wgpu::Buffer; 2],
    binds: [wgpu::BindGroup; 2],
    scene: Option<Geometry>,
    opaque: u32,
    edge_geometry: Option<Geometry>,
    overlay: Option<Geometry>,
    selection: Option<Geometry>,
    targets: Option<Targets>,
    texture: Option<egui::TextureId>,
    samples: u32,
}

impl Viewport3d {
    pub fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Viewport3d {
        let supported = |f| adapter.get_texture_format_features(f).flags.sample_count_supported(MSAA);
        let samples = if supported(COLOR) && supported(DEPTH) { MSAA } else { 1 };
        let module = device
            .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("trigon viewport"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trigon viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trigon viewport"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let face_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: (VERTEX_FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4],
        })];
        let edge_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: (EDGE_FLOATS * 4) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
        })];
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
                        write: bool,
                        compare: wgpu::CompareFunction,
                        bias: i32,
                        blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), compilation_options: Default::default(), buffers },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState { constant: bias, slope_scale: if bias > 0 { 1.0 } else { 0.0 }, clamp: 0.0 },
                }),
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction::{Always, LessEqual};
        let faces = pipeline("trigon faces", "vs_face", "fs_face", &face_buffers, true, LessEqual, 2, None);
        let translucent = pipeline("trigon translucent", "vs_face", "fs_face", &face_buffers, false, LessEqual, 2, blend);
        let edges = pipeline("trigon edges", "vs_edge", "fs_edge", &edge_buffers, false, LessEqual, 0, blend);
        let highlight = pipeline("trigon highlight", "vs_face", "fs_flat", &face_buffers, false, LessEqual, 0, blend);
        let xray = pipeline("trigon highlight x-ray", "vs_face", "fs_flat", &face_buffers, false, Always, 0, blend);
        let uniform = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (UNIFORM_FLOATS * 4) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let uniforms = [uniform("trigon viewport uniforms"), uniform("trigon highlight uniforms")];
        let bind = |b: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trigon viewport"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() }],
            })
        };
        let binds = [bind(&uniforms[0]), bind(&uniforms[1])];
        Viewport3d {
            faces,
            translucent,
            edges,
            highlight,
            xray,
            uniforms,
            binds,
            scene: None,
            opaque: 0,
            edge_geometry: None,
            overlay: None,
            selection: None,
            targets: None,
            texture: None,
            samples,
        }
    }

    pub fn set_scene(&mut self, device: &wgpu::Device, scene: &Scene) {
        self.scene = upload(device, "trigon faces", &scene.vertices, VERTEX_FLOATS);
        self.opaque = self.scene.as_ref().map_or(0, |g| scene.opaque.min(g.count));
        self.edge_geometry = upload(device, "trigon edges", &scene.edges, EDGE_FLOATS);
    }

    /// The change highlight (fades with `View::highlight`).
    pub fn set_overlay(&mut self, device: &wgpu::Device, vertices: &[f32]) {
        self.overlay = upload(device, "trigon highlight", vertices, VERTEX_FLOATS);
    }

    /// The picked element, tinted.
    pub fn set_selection(&mut self, device: &wgpu::Device, vertices: &[f32]) {
        self.selection = upload(device, "trigon selection", vertices, VERTEX_FLOATS);
    }

    fn write_uniforms(&self, queue: &wgpu::Queue, v: &View) {
        for (k, alpha) in [(0, 1.0), (1, v.highlight)] {
            let mut u = [0f32; UNIFORM_FLOATS];
            u[..16].copy_from_slice(&v.view_proj);
            u[16..20].copy_from_slice(&[-0.45, -0.65, 0.75, 0.42]);
            u[20..24].copy_from_slice(&[
                v.clip_z.unwrap_or(0.0),
                if v.clip_z.is_some() { 1.0 } else { 0.0 },
                alpha,
                if v.cut_faces_dark { 1.0 } else { 0.0 },
            ]);
            u[24..28].copy_from_slice(&[v.size[0] as f32, v.size[1] as f32, v.half_line, 0.0]);
            u[28..32].copy_from_slice(&[0.035, 0.035, 0.04, 0.92]);
            u[32..35].copy_from_slice(&v.eye);
            queue.write_buffer(&self.uniforms[k], 0, bytemuck::cast_slice(&u));
        }
    }

    fn draw(&self, encoder: &mut wgpu::CommandEncoder, t: &Targets) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trigon viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: t.msaa.as_ref().unwrap_or(&t.resolve_view),
                depth_slice: None,
                resolve_target: t.msaa.as_ref().map(|_| &t.resolve_view),
                // Multisampled pixels are resolved and then dropped; a single-sample target is kept.
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(BACKGROUND),
                    store: if t.msaa.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.binds[0], &[]);
        if let Some(g) = &self.scene {
            pass.set_pipeline(&self.faces);
            pass.set_vertex_buffer(0, g.buffer.slice(..));
            pass.draw(0..self.opaque, 0..1);
        }
        if let Some(e) = &self.edge_geometry {
            pass.set_pipeline(&self.edges);
            pass.set_vertex_buffer(0, e.buffer.slice(..));
            pass.draw(0..e.count, 0..1);
        }
        if let Some(g) = &self.scene
            && g.count > self.opaque
        {
            pass.set_pipeline(&self.translucent);
            pass.set_vertex_buffer(0, g.buffer.slice(..));
            pass.draw(self.opaque..g.count, 0..1);
        }
        if let Some(s) = &self.selection {
            pass.set_pipeline(&self.highlight);
            pass.set_vertex_buffer(0, s.buffer.slice(..));
            pass.draw(0..s.count, 0..1);
        }
        if let Some(o) = &self.overlay {
            pass.set_bind_group(0, &self.binds[1], &[]);
            pass.set_vertex_buffer(0, o.buffer.slice(..));
            pass.set_pipeline(&self.xray);
            pass.draw(0..o.count, 0..1);
            pass.set_pipeline(&self.highlight);
            pass.draw(0..o.count, 0..1);
        }
    }

    /// Draw the view into the viewport texture (made or resized as needed) and return it for egui.
    pub fn render(&mut self, rs: &egui_wgpu::RenderState, v: &View) -> Option<egui::TextureId> {
        let size = [v.size[0].clamp(1, MAX_SIDE), v.size[1].clamp(1, MAX_SIDE)];
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let t = Targets::new(&rs.device, size, self.samples);
            let mut renderer = rs.renderer.write();
            match self.texture {
                Some(id) => renderer.update_egui_texture_from_wgpu_texture(&rs.device, &t.egui_view, wgpu::FilterMode::Linear, id),
                None => self.texture = Some(renderer.register_native_texture(&rs.device, &t.egui_view, wgpu::FilterMode::Linear)),
            }
            self.targets = Some(t);
        }
        let t = self.targets.as_ref()?;
        self.write_uniforms(&rs.queue, &View { size, ..*v });
        let mut encoder = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("trigon viewport") });
        self.draw(&mut encoder, t);
        rs.queue.submit([encoder.finish()]);
        self.texture
    }

    /// Draw the view offscreen and read it back: RGBA8 (sRGB), `size` pixels.
    pub fn capture(&mut self, rs: &egui_wgpu::RenderState, v: &View) -> Result<([u32; 2], Vec<u8>), String> {
        let size = [v.size[0].clamp(16, MAX_SIDE), v.size[1].clamp(16, MAX_SIDE)];
        let t = Targets::new(&rs.device, size, self.samples);
        self.write_uniforms(&rs.queue, &View { size, ..*v });
        let padded = (size[0] * 4).div_ceil(256) * 256;
        let buffer = rs.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trigon screenshot"),
            size: u64::from(padded) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("trigon screenshot") });
        self.draw(&mut encoder, &t);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.resolve, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(size[1]) },
            },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        rs.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        rs.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("the GPU did not finish the screenshot: {e}"))?;
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("cannot read the screenshot back: {e}"))?;
        let mapped = slice.get_mapped_range().map_err(|e| format!("cannot read the screenshot back: {e}"))?;
        let row = (size[0] * 4) as usize;
        let mut rgba = Vec::with_capacity(row * size[1] as usize);
        for r in mapped.chunks(padded as usize).take(size[1] as usize) {
            rgba.extend_from_slice(r.get(..row).unwrap_or_default());
        }
        drop(mapped);
        buffer.unmap();
        Ok((size, rgba))
    }
}

const UNIFORM_FLOATS: usize = 36;

/// RGBA8 pixels as a PNG.
pub fn png(size: [u32; 2], rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size[0], size[1]);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}
