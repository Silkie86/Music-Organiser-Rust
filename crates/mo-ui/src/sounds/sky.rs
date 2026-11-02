//! The galaxy drawn by the graphics card, the C# app's way (see `mo_app::galaxy_look`). Every star goes to the card
//! once, when the map or the filters change; after that each frame only sends the camera, so the view can glide and
//! drag at the screen's own pace with half a million stars.
//!
//! A frame is five short steps on the card:
//!
//! 1. stamp every star, a tiny soft dot, onto a layer where the light adds up;
//! 2. stamp them again onto a quarter-size layer;
//! 3. blur that layer, across and then down, into the glow;
//! 4. squeeze the brightness of the stamps and the glow into the picture, over the page's background;
//! 5. close up, draw each star crisply over it: a bright core in a soft halo, the left-out stars as faint dust.

use mo_app::galaxy_look::{self as look, Look, STAMP};
use mo_app::galaxy_view::View;
use slint::wgpu_30::wgpu;

/// The stamps' blur on the full-size layer and the quarter-size one, in each layer's pixels. A stamp is never smaller
/// than this, or the card would miss the ones that fall between pixels.
const SIGMA: f32 = 0.6;
const SIGMA_SMALL: f32 = 0.7;
/// The faint dot a left-out star is close up, in points (C#'s).
const DUST: f32 = 1.3;

pub(super) const SHADER: &str = r#"
struct Camera {
    // The map point in the middle of the view.
    centre: vec2<f32>,
    // Map units to clip space, x and y (y runs up).
    unit: vec2<f32>,
    // One pixel in clip space.
    pixel: vec2<f32>,
    // Stamps: their blur in pixels. Crisp stars: the core's radius in pixels.
    size: f32,
    // Stamps: their strength. Crisp stars: how much they show (0 to 1).
    strength: f32,
    // Crisp stars: a left-out star's radius in pixels.
    dust: f32,
    _a: f32,
    _b: f32,
    _c: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

fn corner(v: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    return corners[v];
}

struct Stamp {
    @builtin(position) position: vec4<f32>,
    // The pixel's place from the star's middle, in pixels.
    @location(0) local: vec2<f32>,
    @location(1) light: vec3<f32>,
};

@vertex
fn stamp_vertex(@builtin(vertex_index) v: u32, @location(0) at: vec2<f32>, @location(1) colour: u32,
                @location(2) weight: f32) -> Stamp {
    let local = corner(v) * (3.0 * camera.size + 0.5);
    var out: Stamp;
    out.position = vec4((at - camera.centre) * camera.unit + local * camera.pixel, 0.0, 1.0);
    out.local = local;
    out.light = unpack4x8unorm(colour).rgb * weight * camera.strength;
    return out;
}

@fragment
fn stamp_fragment(in: Stamp) -> @location(0) vec4<f32> {
    let s = camera.size;
    let c = in.light * exp(-dot(in.local, in.local) / (2.0 * s * s));
    return vec4(c, max(c.r, max(c.g, c.b)));
}

struct Crisp {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) colour: vec3<f32>,
    // The core's radius in pixels, and 1 for a shown star, 0 for a left-out one.
    @location(2) radius: f32,
    @location(3) shown: f32,
};

@vertex
fn crisp_vertex(@builtin(vertex_index) v: u32, @location(0) at: vec2<f32>, @location(1) colour: u32,
                @location(2) weight: f32) -> Crisp {
    let c = unpack4x8unorm(colour);
    // 0 for a left-out star; 1 to 255 for a shown one, by how lit it is.
    let shown = c.a > 0.5 / 255.0;
    let lit = max(c.a * 255.0 - 1.0, 0.0) / 254.0;
    // As C#: a well lit star is up to nearly three times as big.
    let radius = select(camera.dust, camera.size * (1.0 + 1.8 * sqrt(lit)), shown);
    // A shown star's halo reaches 2.8 times its core.
    let local = corner(v) * (select(radius, radius * 2.8, shown) + 1.0);
    var out: Crisp;
    out.position = vec4((at - camera.centre) * camera.unit + local * camera.pixel, 0.0, 1.0);
    out.local = local;
    out.colour = c.rgb;
    out.radius = radius;
    out.shown = select(0.0, 1.0, shown);
    return out;
}

@fragment
fn crisp_fragment(in: Crisp) -> @location(0) vec4<f32> {
    let d = length(in.local);
    let core = clamp(in.radius + 0.5 - d, 0.0, 1.0);
    let halo = 0.43 * clamp(1.0 - d / (in.radius * 2.8), 0.0, 1.0);
    let a = select(0.15 * core, core + halo * (1.0 - core), in.shown > 0.5) * camera.strength;
    return vec4(in.colour * a, a);
}

// A triangle over the whole picture, for the steps that work pixel by pixel.
@vertex
fn whole_vertex(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(v & 1u) * 4 - 1);
    let y = f32(i32(v >> 1u) * 4 - 1);
    return vec4(x, y, 0.0, 1.0);
}

@group(0) @binding(0) var source: texture_2d<f32>;

fn blur(p: vec2<i32>, step: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(source));
    var sum = vec4(0.0);
    var total = 0.0;
    for (var i = -8; i <= 8; i++) {
        let w = exp(-f32(i * i) / (2.0 * 2.6 * 2.6));
        let q = p + step * i;
        // Only what's on the layer counts, so the glow doesn't fade towards the picture's edges.
        if (all(q >= vec2(0)) && all(q < size)) {
            sum += textureLoad(source, q, 0) * w;
            total += w;
        }
    }
    return sum / total;
}

@fragment
fn blur_across(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(vec2<i32>(at.xy), vec2(1, 0));
}

@fragment
fn blur_down(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(vec2<i32>(at.xy), vec2(0, 1));
}

struct Shade {
    core: f32,
    glow: f32,
    gain: f32,
    _a: f32,
    background: vec4<f32>,
};
@group(0) @binding(0) var<uniform> shade: Shade;
@group(0) @binding(1) var stamps: texture_2d<f32>;
@group(0) @binding(2) var glow: texture_2d<f32>;
@group(0) @binding(3) var linear: sampler;

@fragment
fn shade_fragment(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let s = textureLoad(stamps, vec2<i32>(at.xy), 0).rgb;
    let uv = at.xy / vec2<f32>(textureDimensions(stamps));
    let g = textureSampleLevel(glow, linear, uv, 0.0).rgb;
    let c = s * shade.core + g * shade.glow;
    let l = max(c.r, max(c.g, c.b));
    var lit = vec3(0.0);
    var a = 0.0;
    if (l >= 0.004) {
        // The brightest colour heads for 1 without ever reaching it, and the others keep their share of it.
        a = 1.0 - exp(-l * shade.gain);
        lit = c * (a / l);
    }
    return vec4(lit + shade.background.rgb * (1.0 - a), 1.0);
}
"#;

/// The layers a frame is drawn through, made for one size of picture.
struct Layers {
    width: u32,
    height: u32,
    stamps: wgpu::TextureView,
    small: wgpu::TextureView,
    across: wgpu::TextureView,
    glow: wgpu::TextureView,
    blur_across: wgpu::BindGroup,
    blur_down: wgpu::BindGroup,
    shade: wgpu::BindGroup,
    /// Two pictures, so one is drawn into while Slint shows the other.
    pictures: [wgpu::Texture; 2],
    next: usize,
}

/// The galaxy's painter on one graphics card.
pub(super) struct Sky {
    device: wgpu::Device,
    queue: wgpu::Queue,
    stamp: wgpu::RenderPipeline,
    crisp: wgpu::RenderPipeline,
    blur_across: wgpu::RenderPipeline,
    blur_down: wgpu::RenderPipeline,
    shade: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    shade_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The cameras for the full-size stamps, the quarter-size ones and the crisp stars, and the squeeze's numbers.
    cameras: [(wgpu::Buffer, wgpu::BindGroup); 3],
    shade_numbers: wgpu::Buffer,
    stars: Option<wgpu::Buffer>,
    count: u32,
    layers: Option<Layers>,
}

const LAYER: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const PICTURE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Sky {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Sky {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("galaxy"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let texture = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let camera_layout = layout("galaxy camera", &[entry(0, uniform)]);
        let texture_layout = layout("galaxy layer", &[entry(0, texture)]);
        let shade_layout = layout(
            "galaxy squeeze",
            &[
                entry(0, uniform),
                entry(1, texture),
                entry(2, texture),
                entry(3, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        );
        let stars_in = [wgpu::VertexBufferLayout {
            array_stride: look::STAR_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32, 2 => Float32],
        }];
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = |label: &str,
                        layout: &wgpu::BindGroupLayout,
                        vertex: &str,
                        buffers: &[wgpu::VertexBufferLayout],
                        fragment: &str,
                        format,
                        blend| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &buffers.iter().map(|b| Some(b.clone())).collect::<Vec<_>>(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let stamp = pipeline(
            "galaxy stamps",
            &camera_layout,
            "stamp_vertex",
            &stars_in,
            "stamp_fragment",
            LAYER,
            Some(wgpu::BlendState { color: add, alpha: add }),
        );
        let crisp = pipeline(
            "galaxy crisp stars",
            &camera_layout,
            "crisp_vertex",
            &stars_in,
            "crisp_fragment",
            PICTURE,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let blur_across =
            pipeline("galaxy glow across", &texture_layout, "whole_vertex", &[], "blur_across", LAYER, None);
        let blur_down = pipeline("galaxy glow down", &texture_layout, "whole_vertex", &[], "blur_down", LAYER, None);
        let shade = pipeline("galaxy squeeze", &shade_layout, "whole_vertex", &[], "shade_fragment", PICTURE, None);
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let camera = || {
            let b = buffer("galaxy camera", 48);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("galaxy camera"),
                layout: &camera_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() }],
            });
            (b, group)
        };
        let cameras = [camera(), camera(), camera()];
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("galaxy glow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Sky {
            device: device.clone(),
            queue: queue.clone(),
            stamp,
            crisp,
            blur_across,
            blur_down,
            shade,

            texture_layout,
            shade_layout,
            sampler,
            cameras,
            shade_numbers: buffer("galaxy squeeze", 32),
            stars: None,
            count: 0,
            layers: None,
        }
    }

    /// Sends the stars to the card (as `mo_app::galaxy_look::pack` made them).
    pub(super) fn set_stars(&mut self, bytes: &[u8]) {
        let count = (bytes.len() / look::STAR_BYTES) as u32;
        if count == 0 {
            (self.stars, self.count) = (None, 0);
            return;
        }
        let fits = self.stars.as_ref().is_some_and(|b| b.size() >= bytes.len() as u64);
        if !fits {
            self.stars = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("galaxy stars"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(b) = &self.stars {
            self.queue.write_buffer(b, 0, bytes);
        }
        self.count = count;
    }

    /// Forgets the stars (a new library is coming).
    pub(super) fn clear_stars(&mut self) {
        (self.stars, self.count) = (None, 0);
    }

    pub(super) fn has_stars(&self) -> bool {
        self.stars.is_some()
    }

    /// The layers for a picture `width` by `height` pixels, made again when the size changes.
    fn make_layers(&mut self, width: u32, height: u32) {
        if self.layers.as_ref().is_none_or(|l| l.width != width || l.height != height) {
            let device = &self.device;
            let make = |label, w: u32, h: u32, format| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    // Copying out is for the look test.
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            };
            let view = |t: wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
            let (sw, sh) = (width.div_ceil(4), height.div_ceil(4));
            let stamps = view(make("galaxy stamps", width, height, LAYER));
            let small = view(make("galaxy small stamps", sw, sh, LAYER));
            let across = view(make("galaxy glow across", sw, sh, LAYER));
            let glow = view(make("galaxy glow", sw, sh, LAYER));
            let reads = |from: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("galaxy layer"),
                    layout: &self.texture_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(from) }],
                })
            };
            let shade = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("galaxy squeeze"),
                layout: &self.shade_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.shade_numbers.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&stamps) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&glow) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.layers = Some(Layers {
                width,
                height,
                blur_across: reads(&small),
                blur_down: reads(&across),
                shade,
                stamps,
                small,
                across,
                glow,
                pictures: [make("galaxy", width, height, PICTURE), make("galaxy", width, height, PICTURE)],
                next: 0,
            });
        }
    }

    /// Draws `view` at `scale` pixels per point, with `look`, over `background` (red, green, blue, 0 to 1), and
    /// returns the picture for Slint to show.
    pub(super) fn draw(&mut self, view: &View, scale: f32, look: &Look, background: [f32; 3]) -> wgpu::Texture {
        let (width, height) =
            (((view.width * scale).round() as u32).max(1), ((view.height * scale).round() as u32).max(1));
        let (sw, sh) = (width.div_ceil(4), height.div_ceil(4));
        // The quarter-size stamps hold a sixteenth of the light, as C#'s, spread over the bigger blur.
        let small = STAMP * (SIGMA * SIGMA) / (SIGMA_SMALL * SIGMA_SMALL) / 16.0;
        let crisp = 1.0 - look.painted;
        let cameras = [
            look::camera(view, scale, width, height, SIGMA, STAMP),
            look::camera(view, scale, sw, sh, SIGMA_SMALL, small),
            look::camera(view, scale, width, height, look.radius * scale, crisp),
        ];
        for (i, c) in cameras.iter().enumerate() {
            let mut numbers = c.to_vec();
            numbers.extend([DUST * scale, 0.0, 0.0, 0.0]);
            self.queue.write_buffer(&self.cameras[i].0, 0, &look::bytes(&numbers));
        }
        let [r, g, b] = background;
        self.queue.write_buffer(
            &self.shade_numbers,
            0,
            &look::bytes(&[look.core, look.glow, look.gain, 0.0, r, g, b, 1.0]),
        );

        self.make_layers(width, height);
        let Some(layers) = self.layers.as_mut() else { unreachable!("made just above") };
        let (count, stars) = (self.count, &self.stars);
        let (stamp, crisp_pipeline, blur_across, blur_down, shade) =
            (&self.stamp, &self.crisp, &self.blur_across, &self.blur_down, &self.shade);
        let groups = [&self.cameras[0].1, &self.cameras[1].1, &self.cameras[2].1];
        let device = &self.device;
        let picture = layers.pictures[layers.next].clone();
        layers.next = 1 - layers.next;
        let target = picture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("galaxy") });
        let pass = |encoder: &mut wgpu::CommandEncoder, to: &wgpu::TextureView, clear: Option<wgpu::Color>| {
            encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("galaxy"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: to,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime()
        };
        // 1 and 2: the stamps, full and quarter size.
        for (to, group) in [(&layers.stamps, groups[0]), (&layers.small, groups[1])] {
            let mut p = pass(&mut encoder, to, Some(wgpu::Color::TRANSPARENT));
            if let Some(stars) = stars {
                p.set_pipeline(stamp);
                p.set_bind_group(0, group, &[]);
                p.set_vertex_buffer(0, stars.slice(..));
                p.draw(0..6, 0..count);
            }
        }
        // 3: the glow.
        for (to, pipeline, group) in
            [(&layers.across, blur_across, &layers.blur_across), (&layers.glow, blur_down, &layers.blur_down)]
        {
            let mut p = pass(&mut encoder, to, None);
            p.set_pipeline(pipeline);
            p.set_bind_group(0, group, &[]);
            p.draw(0..3, 0..1);
        }
        // 4: the squeeze, into the picture.
        {
            let mut p = pass(&mut encoder, &target, None);
            p.set_pipeline(shade);
            p.set_bind_group(0, &layers.shade, &[]);
            p.draw(0..3, 0..1);
        }
        // 5: close up, the crisp stars.
        if crisp > 0.01
            && let Some(stars) = stars
        {
            let mut p = pass(&mut encoder, &target, None);
            p.set_pipeline(crisp_pipeline);
            p.set_bind_group(0, groups[2], &[]);
            p.set_vertex_buffer(0, stars.slice(..));
            p.draw(0..6, 0..count);
        }
        self.queue.submit(Some(encoder.finish()));
        picture
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::naga;

    /// The card only builds the shader when the window opens, so a mistake in it would only show on Solomon's PC. This
    /// checks it here the way wgpu does.
    #[test]
    fn the_galaxy_shader_is_valid() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("the shader parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("the shader is valid");
    }

    /// Draws 450,000 made-up stars in clumps on whatever graphics card there is (a software one will do) to PNGs in
    /// the folder GALAXY_GPU_LOOK names, at a few zooms, to judge the look by eye and to check the card takes every
    /// step.
    #[test]
    #[ignore]
    fn render() {
        use mo_app::galaxy_look::{look, pack};
        use mo_sounds::StarPoint;
        let Ok(out) = std::env::var("GALAXY_GPU_LOOK") else { return };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            crate::gpu::wait(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("a card");
        eprintln!("{:?}", adapter.get_info());
        let (device, queue) =
            crate::gpu::wait(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a device");
        let mut seed = 12345u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 1_000_000) as f32 / 1_000_000.0
        };
        let clumps: Vec<(f32, f32, f32, i32)> =
            (0..40).map(|i| (0.1 + rnd() * 0.8, 0.1 + rnd() * 0.8, 0.01 + rnd() * 0.08, i % 8)).collect();
        let (mut stars, mut colours) = (Vec::new(), Vec::new());
        for i in 0..450_000 {
            let c = clumps[i % clumps.len()];
            let (a, r) = (rnd() * std::f32::consts::TAU, (-(rnd().max(1e-6)).ln()).sqrt() * c.2);
            stars.push(Some(StarPoint {
                x: (c.0 + a.cos() * r).clamp(0.0, 1.0),
                y: (c.1 + a.sin() * r).clamp(0.0, 1.0),
            }));
            colours.push(if rnd() < 0.2 { (rnd() * 8.0) as i32 } else { c.3 });
        }
        let palette = [
            [242, 120, 180],
            [120, 170, 255],
            [255, 190, 80],
            [120, 220, 170],
            [190, 140, 255],
            [255, 120, 90],
            [100, 210, 230],
            [220, 220, 110],
            [110, 110, 110],
        ];
        // Every other star left out by the filters, as a filter would.
        let shown: Vec<usize> = (0..stars.len()).filter(|i| i % 3 != 0).collect();
        let grid = mo_app::galaxy_grid::Grid::new(&stars, &shown);
        let mut sky = Sky::new(&device, &queue);
        // Half the clumps lit, so close up they're the bigger, brighter stars.
        let light: Vec<f32> = (0..stars.len()).map(|i| if i % 2 == 0 { 1.0 } else { 0.0 }).collect();
        sky.set_stars(&pack(&stars, &colours, &palette, &shown, &light));
        for zoom in [1.0f32, 6.0, 60.0, 400.0] {
            let mut view = View::default();
            (view.width, view.height, view.zoom) = (1100.0, 800.0, zoom);
            (view.cx, view.cy) = (clumps[3].0, clumps[3].1);
            let (l, t, r, b) = mo_app::galaxy_look::on_view(&view);
            let lk = look(&view, grid.count_in(l, t, r, b));
            eprintln!("zoom {zoom}: {lk:?}");
            let started = std::time::Instant::now();
            let picture = sky.draw(&view, 1.0, &lk, [18.0 / 255.0; 3]);
            let (w, h) = (picture.width(), picture.height());
            let row = (w * 4).div_ceil(256) * 256;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row * h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                picture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
                },
                picture.size(),
            );
            queue.submit(Some(encoder.finish()));
            buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("read back"));
            device.poll(wgpu::PollType::wait_indefinitely()).expect("the card finishes");
            eprintln!("drawn and read back in {:?}", started.elapsed());
            let data = buffer.slice(..).get_mapped_range().expect("mapped");
            let mut img = image::RgbaImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let at = (y * row + x * 4) as usize;
                    img.put_pixel(x, y, image::Rgba([data[at], data[at + 1], data[at + 2], 255]));
                }
            }
            img.save(format!("{out}/galaxy-gpu-{zoom}.png")).expect("saved");
        }
    }
}
