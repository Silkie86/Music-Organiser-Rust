//! The galaxy drawn by the graphics card, the C# app's way (see `mo_app::galaxy_look`). Every star goes to the card
//! once, when the map or the filters change; after that each frame only sends the camera, so the view can glide and
//! drag at the screen's own pace with half a million stars.
//!
//! A frame is five short steps on the card:
//!
//! 1. stamp every star, a tiny soft dot, onto a layer where the light adds up;
//! 2. stamp them again onto a quarter-size layer;
//! 3. blur that layer, across and then down, into the glow;
//! 4. squeeze the brightness of the stamps and the glow into the picture, over the page's background, the stars the
//!    filters leave out on their own, as grey dust that never gets past a faint haze, so the ones that match stand out;
//! 5. draw the stars as stars over it: a core in the star's colour, white-hot in its very middle, in a soft glow.
//!    On a dark page their light is screened on (as the processor's picture does), so a cluster glows where they crowd
//!    but keeps its colour; on a light page (Paper) they're laid over it, as the processor's picture is laid over the
//!    page. Once the stars are far enough apart to tell apart every star is drawn so, the left-out ones as faint grey
//!    dust: first as small and bright as the painted dot under it, growing and brightening as the view flies in
//!    (`mo_app::galaxy_look::look`). Far out only the stars "Light by" lights most are, each a bright point of its own
//!    with a short, faint glow, so they stand out at every zoom. The very brightest get faint four-point spikes, and
//!    every star twinkles very slightly (only in brightness, and not while the mouse sweeps, drags or glides, so it
//!    never gets in the way of picking a sound).
//!
//! A twinkle only changes step 5, so for a twinkle the card keeps the picture of steps 1 to 4 and draws the stars
//! over it again.

use mo_app::galaxy_look::{self as look, Look, STAMP};
use mo_app::galaxy_view::View;
pub use wgpu;

/// The stamps' blur on the full-size layer and the quarter-size one, in each layer's pixels. A stamp is never smaller
/// than this, or the card would miss the ones that fall between pixels.
const SIGMA: f32 = 0.6;
const SIGMA_SMALL: f32 = 0.7;
/// The faint dot a left-out star is close up, in points (C#'s).
const DUST: f32 = 1.3;

pub const SHADER: &str = r#"
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
    // How painted the view is (1 far out, 0 close up).
    painted: f32,
    // 1 when Light by lights the stars (some of the shown ones are lit), 0 when they're all alike.
    light_on: f32,
    // The twinkle: the time in seconds, and how much (0 for none).
    time: f32,
    twinkle: f32,
    // From which star (in the order mo_app::galaxy_look::pack sorts them, the best lit last) they show as points of
    // their own far out, and get spikes; and how many stars there are (mo_app::galaxy_look::glint_starts).
    glint_start: f32,
    spike_start: f32,
    count: f32,
    // Pixels per point.
    scale: f32,
    // Crisp stars: how far they've gone from looking like the painted dots they took over from (0) to how they look
    // close up (1), and how quickly the painted picture's brightness squeezes (to start them as bright as its dots).
    bright: f32,
    gain: f32,
    _c: f32,
    // Crisp stars: the page's colour (red, green, blue, 0 to 1; the last is unused), for the left-out stars' dust.
    page: vec4<f32>,
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
    // A shown star's light in its colour; a left-out one's in the last channel only, as grey dust (step 4 squeezes it
    // on its own, to a low ceiling, so however many there are they stay a faint haze).
    @location(1) light: vec4<f32>,
};

@vertex
fn stamp_vertex(@builtin(vertex_index) v: u32, @location(0) at: vec2<f32>, @location(1) colour: u32,
                @location(2) weight: f32) -> Stamp {
    let local = corner(v) * (3.0 * camera.size + 0.5);
    var out: Stamp;
    out.position = vec4((at - camera.centre) * camera.unit + local * camera.pixel, 0.0, 1.0);
    out.local = local;
    let c = unpack4x8unorm(colour);
    let w = weight * camera.strength;
    out.light = select(vec4(c.rgb * w, 0.0), vec4(0.0, 0.0, 0.0, w), c.a < 0.5 / 255.0);
    return out;
}

@fragment
fn stamp_fragment(in: Stamp) -> @location(0) vec4<f32> {
    let s = camera.size;
    return in.light * exp(-dot(in.local, in.local) / (2.0 * s * s));
}

// The numbers below are mo_app::galaxy_look's, which the processor's picture uses too (a test checks they match).
const UNLIT_SIZE: f32 = 0.7;
const MIN_LIT: f32 = 1.6;
const SPIKE_MIN: f32 = 10.0;
const GLOW: f32 = 0.36;
const GLOW_LIT: f32 = 0.3;
const HOT: f32 = 0.08;
const WHITE: f32 = 0.4;
const FAR_GLOW: f32 = 0.2;
const FAR_HALO: f32 = 2.2;
const HALO: f32 = 2.8;
const FALLOFF: f32 = 1.3;
const LIT_BRIGHTEST: f32 = 1.5;
const DUST_LIT: f32 = 0.5;
const DUST_GREY: f32 = 0.6;
const DUST_MOST: f32 = 0.22;

struct Crisp {
    @builtin(position) position: vec4<f32>,
    // The pixel's place from the star's middle, in pixels.
    @location(0) local: vec2<f32>,
    // The star's colour, times how strongly it shows and its twinkle.
    @location(1) colour: vec3<f32>,
    // The core's radius, how far its glow reaches and how long its spikes are (0 for none), in pixels.
    @location(2) radius: f32,
    @location(3) halo: f32,
    @location(4) spikes: f32,
    // How lit it is (0 to 1, 0 with Light by off).
    @location(6) lit: f32,
    // How strong its glow is: all of it close up, a little far out, where many glints would add up to white.
    @location(7) glow: f32,
    // How strongly it shows (what its colour was multiplied by), for laying it over a light page.
    @location(8) strength: f32,
};

// A steady number from 0 to 1 for each star, for its twinkle.
fn hash(n: u32) -> f32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return f32((x >> 22u) ^ x) / 4294967295.0;
}

@vertex
fn crisp_vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) star: u32, @location(0) at: vec2<f32>,
                @location(1) colour: u32, @location(2) weight: f32) -> Crisp {
    let c = unpack4x8unorm(colour);
    // 0 for a left-out star; 1 to 255 for a shown one, by how lit it is.
    let shown = c.a > 0.5 / 255.0;
    let lighting = camera.light_on > 0.5;
    let lit = select(0.0, max(c.a * 255.0 - 1.0, 0.0) / 254.0, lighting);
    // Once the stars have come apart every star shows; far out, where the stars are only painted, the best lit still
    // show as points. By rank: the light byte can't tell the top few hundred of a big library apart. It rises along
    // the ones that glint, and the last, the best lit, shows fully, even when it's the only one
    // (mo_app::galaxy_look::glint_at).
    let rank = f32(star);
    let glint = select(0.0, smoothstep(0.0, 1.0, (rank - camera.glint_start + 1.0) / max(camera.count - camera.glint_start, 1.0)),
                       lighting && rank >= camera.glint_start);
    // How bright its core is close up: with Light by on, as bright as it's lit, with its steady variety (its weight,
    // as `pack` made it; the processor's picture does the same). A left-out star is faint dust, fainter still with
    // Light by on, when the unlit stars on show are dimmed too.
    let close = select(0.15 * select(1.0, DUST_LIT, lighting), select(1.0, weight / LIT_BRIGHTEST, lighting), shown);
    // Where the stars have only just come apart, as bright as the painted dot it takes over from (its light squeezed
    // as the painted picture's is, its colour's brightest part heading for 1), going to its close-up brightness as
    // the view flies in, so it neither jumps nor dips.
    // A left-out star is grey dust, as bright as the painted dust it takes over from.
    let most = max(c.r, max(c.g, c.b));
    let painted_dot = select(DUST_MOST * (1.0 - exp(-camera.gain * weight)),
                             (1.0 - exp(-camera.gain * weight * most)) / max(most, 0.001), shown);
    let core = camera.strength * mix(painted_dot, close, camera.bright);
    // Far out a star that glints shows as brightly as close up, as much as it glints, and as the view flies in it
    // never gets dimmer.
    let show = select(core, max(core, glint * close), shown);
    // A well lit star is up to nearly three times as big (mo_app::galaxy_look::lit_size, the same curve); with Light by
    // on, an unlit one is smaller than an all-alike star, so the lit ones stand out. Where the stars have only just
    // come apart they're all as small as the painted dots, and grow into their sizes as the view flies in.
    let size = mix(1.0, (1.0 + 1.8 * sqrt(lit)) * select(1.0, UNLIT_SIZE, lighting), camera.bright);
    let near = camera.size * size;
    // With Light by on, far out a star that glints is a small bright point, never smaller than one you can see, and as
    // the view flies in it stays so until its own size is bigger, so it never shrinks, and is never smaller than a less
    // lit one beside it. (Far out only the ones that glint are drawn; the processor's picture draws every star at
    // every zoom, so there the ones that glint keep at least their own size instead: mo_app::galaxy_look::glint_point.)
    // A left-out star is a faint dot, never bigger than a shown one.
    let point = MIN_LIT * camera.scale * (1.0 + 0.6 * glint);
    let radius = select(min(camera.dust, camera.size), select(near, max(point, near), glint > 0.0), shown);
    // A shown star's glow reaches 2.8 times its core close up, a well lit one's up to twice that; far out, where
    // only a little of the glow shows, it's kept short.
    let halo = select(radius * 1.5, radius * mix(FAR_HALO, HALO + HALO * lit, camera.bright), shown);
    // The brightest few get faint four-point spikes, growing along them as the glints do.
    let spiky = smoothstep(0.0, 1.0, (rank - camera.spike_start + 1.0) / max(camera.count - camera.spike_start, 1.0));
    let spikes = select(0.0, spiky * max(radius * 7.0, SPIKE_MIN * camera.scale),
                        shown && lighting && rank >= camera.spike_start);
    // A very slight twinkle, only in brightness, each star at its own pace: by where it is on the map, so it keeps its
    // pace when the stars are packed again in another order. Each pace is a whole number of rounds an hour, so when
    // the clock wraps round each hour no star jumps.
    let id = (bitcast<u32>(at.x) * 73856093u) ^ (bitcast<u32>(at.y) * 19349663u);
    let pace = round((1.1 + 1.6 * hash(id)) * 3600.0 / 6.2831853) * 6.2831853 / 3600.0;
    let twinkle = 1.0 + camera.twinkle * sin(camera.time * pace + 6.2831853 * hash(id ^ 0x9e3779b9u));
    let local = corner(v) * (max(halo, spikes) + 1.0);
    var out: Crisp;
    // A star that doesn't show costs next to nothing: its corners all fall on one point, off the picture, so it has no
    // pixels to draw.
    out.position = select(vec4((at - camera.centre) * camera.unit + local * camera.pixel, 0.0, 1.0),
                          vec4(2.0, 2.0, 0.0, 1.0), show < 0.004);
    out.local = local;
    let strength = show * twinkle;
    out.colour = c.rgb * strength;
    out.strength = strength;
    out.radius = radius;
    out.halo = halo;
    out.spikes = spikes;
    out.lit = lit;
    out.glow = mix(FAR_GLOW, 1.0, camera.bright);
    return out;
}

@fragment
fn crisp_fragment(in: Crisp) -> @location(0) vec4<f32> {
    let d = length(in.local);
    let r = in.radius;
    // The core: a crisp disc in the star's colour, white-hot only in its very middle.
    let core = clamp(r + 0.5 - d, 0.0, 1.0);
    let hot = exp(-d * d / (HOT * r * r + 0.15));
    // The glow: light falling off softly from the core, gone by the halo's edge.
    let edge = clamp(1.0 - d / in.halo, 0.0, 1.0);
    let glow = (GLOW + GLOW_LIT * in.lit) * pow(edge, FALLOFF) * in.glow;
    // The spikes: two thin lines through the middle, fading along their length.
    var spike = 0.0;
    if (in.spikes > 0.0) {
        let a = abs(in.local);
        let fade = clamp(1.0 - d / in.spikes, 0.0, 1.0);
        spike = 0.3 * (exp(-a.y * 1.8) + exp(-a.x * 1.8)) * fade * fade;
    }
    // (Only shown stars come here: the left-out ones are drawn as dust, by `dust_fragment`.)
    // The glow (with the spikes) and the core are screened together, as the processor's picture screens a star's core
    // over its glow: where the glow is under the core it lights it a little more, and nothing goes past 1, so the core
    // keeps the star's own colour (light past 1 would be cut channel by channel, turning a blue star cyan).
    let around = min(glow + spike, 1.0);
    let g = in.colour * around;
    let k = in.colour * core;
    let white = WHITE * hot * core * in.glow * max(in.colour.r, max(in.colour.g, in.colour.b));
    // How strongly it shows, the same way (its alpha, at least each of its colours, so it stays premultiplied).
    let ga = in.strength * around;
    let ka = in.strength * core;
    // On a dark page the light is screened onto what's there (as the processor's picture does): where stars crowd
    // their glows build up, but a crowd keeps its colour longer than if the light were simply added, which burns a
    // dense cluster to white. On a light page screened light wouldn't show, so there it's laid over the picture by how
    // strongly it shows (its alpha), as the processor's picture is laid over the page.
    return vec4(min(g + k - g * k + vec3(white), vec3(1.0)), min(ga + ka - ga * ka + white, 1.0));
}

// A left-out star close up: a faint grey dot, laid a little way from the page towards grey by how strongly it
// shows. It's drawn with the lighter (on a dark page) or the darker (on a light one) of it and what's there, never
// added, so where thousands of them crowd together they stay as faint as one.
@fragment
fn dust_fragment(in: Crisp) -> @location(0) vec4<f32> {
    let core = clamp(in.radius + 0.5 - length(in.local), 0.0, 1.0);
    let a = min(in.strength * core, DUST_MOST);
    if (a < 0.002) {
        discard;
    }
    return vec4(mix(camera.page.rgb, vec3(DUST_GREY), a), 1.0);
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
    let st = textureLoad(stamps, vec2<i32>(at.xy), 0);
    let uv = at.xy / vec2<f32>(textureDimensions(stamps));
    let gl = textureSampleLevel(glow, linear, uv, 0.0);
    let c = st.rgb * shade.core + gl.rgb * shade.glow;
    let l = max(c.r, max(c.g, c.b));
    var lit = vec3(0.0);
    var a = 0.0;
    if (l >= 0.004) {
        // The brightest colour heads for 1 without ever reaching it, and the others keep their share of it.
        a = 1.0 - exp(-l * shade.gain);
        lit = c * (a / l);
    }
    // The left-out stars' grey dust, under the shown ones: squeezed the same way, but only ever up to DUST_MOST of the
    // way from the page to grey.
    let dust = DUST_MOST * (1.0 - exp(-(st.a * shade.core + gl.a * shade.glow) * shade.gain));
    let under = mix(shade.background.rgb, vec3(DUST_GREY), dust);
    return vec4(lit + under * (1.0 - a), 1.0);
}
"#;

/// What else lights the stars on one picture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shine {
    /// Light by lights them (not Nothing, and the measure is known).
    pub light_on: bool,
    /// The twinkle's clock, in seconds, and how much the stars twinkle (0 for not at all).
    pub time: f32,
    pub twinkle: f32,
}

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
    /// The painted picture (steps 1 to 4), kept so a twinkle only draws the stars over it again, and what it was drawn
    /// for.
    base: wgpu::Texture,
    base_for: Option<Painted>,
    /// Two pictures, so one is drawn into while Slint shows the other.
    pictures: [wgpu::Texture; 2],
    next: usize,
}

/// What the painted picture depends on: the two stamps' cameras, the squeeze's numbers and which stars were sent.
#[derive(Clone, Copy, PartialEq)]
struct Painted {
    cameras: [[f32; 8]; 2],
    shade: [f32; 8],
    stars: u64,
}

/// The galaxy's painter on one graphics card.
pub struct Sky {
    device: wgpu::Device,
    queue: wgpu::Queue,
    stamp: wgpu::RenderPipeline,
    /// The stars as stars, screened on (a dark page) or laid over (a light one).
    crisp: wgpu::RenderPipeline,
    crisp_over: wgpu::RenderPipeline,
    /// The left-out stars as faint grey dust, the same two ways.
    dust: wgpu::RenderPipeline,
    dust_over: wgpu::RenderPipeline,
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
    /// How many of the stars, at the start, the filters leave out (`pack` puts them first).
    left_out: u32,
    /// How many of the stars, at the end, are lit by the measure: far out only the best lit of them are drawn as stars.
    lit: u32,
    /// Counts the stars sent, so the painted picture kept for a twinkle is never one of stars sent before.
    sent: u64,
    layers: Option<Layers>,
}

const LAYER: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const PICTURE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The camera as the shader reads it (`Camera`): 24 f32.
const CAMERA_BYTES: u64 = 96;

impl Sky {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Sky {
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
        // The picture stays opaque: the stars only change its colour.
        let keep_alpha = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let crisp_with = |label: &str, fragment: &str, dst_factor, operation| {
            pipeline(
                label,
                &camera_layout,
                "crisp_vertex",
                &stars_in,
                fragment,
                PICTURE,
                Some(wgpu::BlendState {
                    color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor, operation },
                    alpha: keep_alpha,
                }),
            )
        };
        // A dark page: screened on (what's there times one minus the star's light, plus the light). A light page:
        // laid over (what's there times one minus how strongly the star shows, plus its light).
        let plus = wgpu::BlendOperation::Add;
        let crisp = crisp_with("galaxy crisp stars", "crisp_fragment", wgpu::BlendFactor::OneMinusSrc, plus);
        let crisp_over = crisp_with(
            "galaxy crisp stars over a light page",
            "crisp_fragment",
            wgpu::BlendFactor::OneMinusSrcAlpha,
            plus,
        );
        // The left-out stars' dust: the lighter of it and what's there on a dark page, the darker on a light one.
        let one = wgpu::BlendFactor::One;
        let dust = crisp_with("galaxy dust", "dust_fragment", one, wgpu::BlendOperation::Max);
        let dust_over = crisp_with("galaxy dust over a light page", "dust_fragment", one, wgpu::BlendOperation::Min);
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
            let b = buffer("galaxy camera", CAMERA_BYTES);
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
            crisp_over,
            dust,
            dust_over,
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
            left_out: 0,
            lit: 0,
            sent: 0,
            layers: None,
        }
    }

    /// Sends the stars to the card (as `mo_app::galaxy_look::pack` made them: the last `lit` of them lit by the
    /// measure).
    pub fn set_stars(&mut self, bytes: &[u8], lit: u32) {
        let count = (bytes.len() / look::STAR_BYTES) as u32;
        self.sent += 1;
        if count == 0 {
            (self.stars, self.count, self.lit, self.left_out) = (None, 0, 0, 0);
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
        self.lit = lit.min(count);
        // A left-out star's light byte is 0, a shown one's 1 to 255, and `pack` puts the left-out ones first.
        let (stars, _) = bytes.as_chunks::<{ look::STAR_BYTES }>();
        let left_out = |star: &[u8; look::STAR_BYTES]| star[look::LIGHT_BYTE] == 0;
        self.left_out = stars.partition_point(left_out) as u32;
        debug_assert!(!stars[self.left_out as usize..].iter().any(left_out), "the left-out stars come first");
    }

    /// Forgets the stars (a new library is coming).
    pub fn clear_stars(&mut self) {
        (self.stars, self.count, self.lit, self.left_out) = (None, 0, 0, 0);
        self.sent += 1;
    }

    /// Paints the next picture afresh, even if nothing it's painted from has changed (the look test times whole frames with it).
    pub fn repaint(&mut self) {
        if let Some(layers) = self.layers.as_mut() {
            layers.base_for = None;
        }
    }

    pub fn has_stars(&self) -> bool {
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
                    // Copying in is the painted picture under the stars; copying out is for it, and the look test.
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST,
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
                base: make("galaxy painted", width, height, PICTURE),
                base_for: None,
                pictures: [make("galaxy", width, height, PICTURE), make("galaxy", width, height, PICTURE)],
                next: 0,
            });
        }
    }

    /// Draws `view` at `scale` pixels per point, with `look` and `shine`, over `background` (red, green, blue, 0 to 1),
    /// and returns the picture for Slint to show.
    pub fn draw(&mut self, view: &View, scale: f32, look: &Look, shine: Shine, background: [f32; 3]) -> wgpu::Texture {
        let (width, height) =
            (((view.width * scale).round() as u32).max(1), ((view.height * scale).round() as u32).max(1));
        let (sw, sh) = (width.div_ceil(4), height.div_ceil(4));
        // The quarter-size stamps hold a sixteenth of the light, as C#'s, spread over the bigger blur.
        let small = STAMP * (SIGMA * SIGMA) / (SIGMA_SMALL * SIGMA_SMALL) / 16.0;
        let crisp = 1.0 - look.painted;
        let count = self.count;
        // Which stars glint far out and get spikes, by rank (see `glint_starts`); none with nothing lit.
        let (glint_start, spike_start) =
            if shine.light_on { look::glint_starts(look, self.lit, count) } else { (count, count) };
        let stamps = [
            look::camera(view, scale, width, height, SIGMA, STAMP),
            look::camera(view, scale, sw, sh, SIGMA_SMALL, small),
        ];
        let cameras = [stamps[0], stamps[1], look::camera(view, scale, width, height, look.radius * scale, crisp)];
        for (i, c) in cameras.iter().enumerate() {
            let mut numbers = c.to_vec();
            numbers.extend([
                DUST * scale,
                look.painted,
                if shine.light_on { 1.0 } else { 0.0 },
                shine.time,
                shine.twinkle,
                glint_start as f32,
                spike_start as f32,
                count as f32,
                scale,
                look.bright,
                look.gain,
                0.0,
                background[0],
                background[1],
                background[2],
                1.0,
            ]);
            self.queue.write_buffer(&self.cameras[i].0, 0, &look::bytes(&numbers));
        }
        let [r, g, b] = background;
        let shade = [look.core, look.glow, look.gain, 0.0, r, g, b, 1.0];
        let painted = Painted { cameras: stamps, shade, stars: self.sent };

        self.make_layers(width, height);
        let Some(layers) = self.layers.as_mut() else { unreachable!("made just above") };
        let stars = &self.stars;
        // Far out, only the stars lit enough to glint are drawn as stars: the last ones, as `pack` sorts them.
        let as_stars = if crisp > 0.01 { 0..count } else { glint_start..count };
        // On a light page (Paper) the stars are laid over the picture; on a dark one their light is screened onto it.
        // Light as the app judges a colour light (`themes::is_light`).
        let light_page = mo_app::themes::is_light(
            [r, g, b].iter().fold(0, |rgb, c| (rgb << 8) | (c.clamp(0.0, 1.0) * 255.0).round() as u32),
        );
        let crisp_pipeline = if light_page { &self.crisp_over } else { &self.crisp };
        let dust_pipeline = if light_page { &self.dust_over } else { &self.dust };
        // The left-out stars come first, as `pack` sorts them: they're drawn as dust, the rest as stars over them. (Far
        // out only the lit ones from `glint_start` are drawn, all shown, so past every left-out one: no dust.)
        let left_out = self.left_out.min(count);
        let (as_dust, as_stars) =
            (as_stars.start..left_out.max(as_stars.start), left_out.max(as_stars.start)..as_stars.end);
        let (stamp, blur_across, blur_down, shade_pipeline) =
            (&self.stamp, &self.blur_across, &self.blur_down, &self.shade);
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
        // Steps 1 to 4 paint the picture under the stars. Nothing they're painted from changes for a twinkle (only the
        // stars' brightness does), so the one kept from last time is used as it is.
        if layers.base_for != Some(painted) {
            self.queue.write_buffer(&self.shade_numbers, 0, &look::bytes(&shade));
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
            // 4: the squeeze, into the painted picture.
            {
                let base = layers.base.create_view(&wgpu::TextureViewDescriptor::default());
                let mut p = pass(&mut encoder, &base, None);
                p.set_pipeline(shade_pipeline);
                p.set_bind_group(0, &layers.shade, &[]);
                p.draw(0..3, 0..1);
            }
            layers.base_for = Some(painted);
        }
        encoder.copy_texture_to_texture(layers.base.as_image_copy(), picture.as_image_copy(), picture.size());
        // 5: the stars as stars: every one close up, only the best lit far out.
        if (crisp > 0.01 || shine.light_on)
            && let Some(stars) = stars
        {
            let mut p = pass(&mut encoder, &target, None);
            p.set_bind_group(0, groups[2], &[]);
            p.set_vertex_buffer(0, stars.slice(..));
            if !as_dust.is_empty() {
                p.set_pipeline(dust_pipeline);
                p.draw(0..6, as_dust);
            }
            p.set_pipeline(crisp_pipeline);
            p.draw(0..6, as_stars);
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

    #[test]
    fn the_shader_uses_the_processor_pictures_numbers() {
        for (name, value) in [
            ("UNLIT_SIZE", look::UNLIT_SIZE),
            ("MIN_LIT", look::MIN_LIT),
            ("SPIKE_MIN", look::SPIKE_MIN),
            ("GLOW", look::GLOW),
            ("GLOW_LIT", look::GLOW_LIT),
            ("HOT", look::HOT),
            ("WHITE", look::WHITE),
            ("FAR_GLOW", look::FAR_GLOW),
            ("FAR_HALO", look::FAR_HALO),
            ("HALO", look::HALO),
            ("FALLOFF", look::FALLOFF),
            ("LIT_BRIGHTEST", look::LIT_BRIGHTEST),
            ("DUST_LIT", look::DUST_LIT),
            ("DUST_GREY", look::DUST_GREY),
            ("DUST_MOST", look::DUST_MOST),
        ] {
            assert!(SHADER.contains(&format!("const {name}: f32 = {value:?};")), "{name} isn't {value} in the shader");
        }
    }
}
