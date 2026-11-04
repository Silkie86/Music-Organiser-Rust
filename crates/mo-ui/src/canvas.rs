//! The graphics card's painter for the views the app draws itself (the arrangement, the MIDI page's piano roll, the
//! sounds' waveforms): it draws the [`mo_app::shapes::Scene`] a view works out, which the processor draws the same way
//! where there's no card (`Scene::paint`). A picture is one draw on the card: every shape is two triangles, which the card fills with its
//! colour, cutting the corners round and softening the edges where the shape asks for it, each shape over the ones
//! before. A waveform's lines are softened as the window's own drawing library softens the lines of a path instead,
//! as that's what the window draws where there's no picture.

use std::cell::RefCell;
use std::thread::LocalKey;

use mo_app::shapes::{self, Colour, SHAPE_BYTES, Scene};
use slint::wgpu_30::wgpu;
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::{gpu, watchdog};

pub(crate) const SHADER: &str = r#"
struct Frame {
    // The picture's size in pixels, pixels to a point, and how much a picture too big for the card was shrunk to fit
    // (1 for one that fits).
    size: vec2<f32>,
    scale: f32,
    fit: f32,
    // For a picture drawn as the window draws (a waveform's, the Sounds tiles'), how wide the window's drawing library
    // makes the soft edges of what it draws, in pixels (lines are then drawn as it draws a path's, `stroked`, and
    // rounded rectangles as soft as its fills); 0 for the arrangement's, soft as the processor's.
    fringe: f32,
    // Unused: a frame is 32 bytes.
    unused_a: f32,
    unused_b: f32,
    unused_c: f32,
};
@group(0) @binding(0) var<uniform> frame: Frame;

// What kind of shape (mo_app::shapes, which packs them).
const RECT: u32 = 0u;
const ROUNDED: u32 = 1u;
const LINE: u32 = 2u;

struct Shape {
    @builtin(position) position: vec4<f32>,
    // The fill (a line's colour) and the edge, premultiplied.
    @location(0) @interpolate(flat) fill: vec4<f32>,
    @location(1) @interpolate(flat) edge: vec4<f32>,
    // A rounded rectangle's left, top, right and bottom, or a line's two ends, in pixels.
    @location(2) @interpolate(flat) at: vec4<f32>,
    // A rounded rectangle's corner radius, or half a line's width, in pixels.
    @location(3) @interpolate(flat) size: f32,
    @location(4) @interpolate(flat) kind: u32,
};

// The corners of the two triangles that make a shape, from 0 to 1 each way.
fn corner(v: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    return corners[v];
}

// Along a line and across it, from its first end to its second (along to the right when it has no length).
fn along(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let d = b - a;
    let l = length(d);
    return select(vec2(1.0, 0.0), d / max(l, 1e-6), l > 1e-6);
}

@vertex
fn shape_vertex(@builtin(vertex_index) v: u32, @location(0) at: vec4<f32>, @location(1) fill: u32,
                @location(2) edge: u32, @location(3) size: f32, @location(4) kind: u32) -> Shape {
    let s = frame.scale;
    let px = at * s;
    let c = corner(v);
    var p = mix(px.xy, px.zw, c);
    if (kind == RECT) {
        // Packed in whole pixels already (mo_app::shapes::crisp, as the processor fills them).
        p = mix(at.xy, at.zw, c) * frame.fit;
    } else if (kind == LINE) {
        // A strip along the line, a pixel longer at each end and wider each side for its soft edges.
        let d = along(px.xy, px.zw);
        let across = vec2(-d.y, d.x);
        let half = size * s * 0.5 + 1.0;
        let l = length(px.zw - px.xy);
        p = px.xy + d * mix(-1.0, l + 1.0, c.x) + across * mix(-half, half, c.y);
    } else if (kind == ROUNDED) {
        // A pixel more all round for the soft edge, and half the edge's width.
        let grow = 1.0 + 0.5 * s;
        p = mix(px.xy - vec2(grow), px.zw + vec2(grow), c);
    }
    var out: Shape;
    out.position = vec4(p.x / frame.size.x * 2.0 - 1.0, 1.0 - p.y / frame.size.y * 2.0, 0.0, 1.0);
    let f = unpack4x8unorm(fill);
    let e = unpack4x8unorm(edge);
    out.fill = vec4(f.rgb * f.a, f.a);
    out.edge = vec4(e.rgb * e.a, e.a);
    out.at = px;
    out.size = select(size * s, size * s * 0.5, kind == LINE);
    out.kind = kind;
    return out;
}

// How much of a pixel a line `half` wide each side and `l` long covers, the pixel's middle `t` along it from its first
// end and `off` from it across, drawn as the window's drawing library (femtovg, as Slint sets it up) strokes a path's
// line with square ends: its soft edges are `f` wide, half inside the line and half out, at its sides and its ends
// (in femtovg's path/cache.rs `expand_stroke` and its shader's `strokeMask`). A line thinner than `f` is drawn that
// wide and fainter by its share of it, squared. A line shorter than `f` (a quiet peak) has no middle, so the soft parts
// of its two ends are laid one over the other (femtovg draws each end on its own, over the other).
fn stroked(half: f32, l: f32, t: f32, off: f32, f: f32) -> f32 {
    let thin = min(2.0 * half / f, 1.0);
    let side = clamp((max(half, 0.5 * f) + 0.5 * f - off) / f, 0.0, 1.0) * thin * thin;
    // 0 half an edge before each end, 1 half an edge after it.
    let start = (t + 0.5 * f) / f;
    let end = (l - t + 0.5 * f) / f;
    if (l > f) {
        return side * clamp(min(start, end), 0.0, 1.0);
    }
    let first = select(0.0, side * start, start >= 0.0 && start <= 1.0);
    let second = select(0.0, side * end, end >= 0.0 && end <= 1.0);
    return first + second - first * second;
}

@fragment
fn shape_fragment(in: Shape) -> @location(0) vec4<f32> {
    // A crisp rectangle fills the whole pixels its triangles cover.
    if (in.kind == RECT) {
        return in.fill;
    }
    // The pixel's middle.
    let p = in.position.xy;
    if (in.kind == LINE) {
        let a = in.at.xy;
        let d = along(a, in.at.zw);
        let l = length(in.at.zw - a);
        let t = dot(p - a, d);
        let off = abs(dot(p - a, vec2(-d.y, d.x)));
        if (frame.fringe > 0.0) {
            return in.fill * stroked(in.size, l, t, off, frame.fringe);
        }
        // Soft along its sides; cut square at its ends.
        let cover = clamp(in.size + 0.5 - off, 0.0, 1.0) * clamp(t + 0.5, 0.0, 1.0) * clamp(l - t + 0.5, 0.0, 1.0);
        return in.fill * cover;
    }
    // A rounded rectangle: how far the pixel is outside its border (less than 0 inside).
    let middle = (in.at.xy + in.at.zw) * 0.5;
    let half = (in.at.zw - in.at.xy) * 0.5;
    let r = in.size;
    let q = abs(p - middle) - half + vec2(r);
    let out_by = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
    // Soft over a pixel, or, for a picture drawn as the window draws (the Sounds tiles'), over the width of the
    // window's soft edges, which it gives a shape's fill and its edge alike (femtovg's `fringe_width`).
    let f = select(1.0, frame.fringe, frame.fringe > 0.0);
    let fill = in.fill * clamp(0.5 - out_by / f, 0.0, 1.0);
    // The edge: a line a point wide along the border, over the fill.
    let edge = in.edge * clamp((0.5 * frame.scale + 0.5 * f - abs(out_by)) / f, 0.0, 1.0);
    return edge + fill * (1.0 - edge.a);
}
"#;

const PICTURE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The frame as the shader reads it (`Frame`): 8 f32.
const FRAME_BYTES: u64 = 32;
/// The most shapes drawn at once (32 MB of them): a view with more is drawn a part at a time, each over the one before,
/// so a huge song doesn't need a huge buffer on the card (or one bigger than the card allows).
const MOST_AT_ONCE: usize = 1 << 20;

/// What the pictures are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Whole pictures, the shapes over their background (the arrangement).
    Picture,
    /// Shapes of one colour over nothing, to lay over what's under them (a waveform): the background's colour
    /// everywhere and how much of it shows in the alpha, which is how the window reads a picture from the card (a
    /// colour see-through by half is the colour, half there).
    OneColour,
}

/// The pictures drawn into, made for one size: two, so one is drawn into while Slint shows the other.
struct Pictures {
    width: u32,
    height: u32,
    textures: [wgpu::Texture; 2],
    next: usize,
}

/// The painter on one graphics card.
pub(crate) struct Canvas {
    device: wgpu::Device,
    queue: wgpu::Queue,
    kind: Kind,
    pipeline: wgpu::RenderPipeline,
    frame: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    shapes: Option<wgpu::Buffer>,
    /// How many shapes this card takes at once: [`MOST_AT_ONCE`], or fewer when its biggest buffer is smaller.
    most: usize,
    pictures: Option<Pictures>,
}

/// The layout of a shape as the card reads it, from `mo_app::shapes::Scene::pack`.
const SHAPE_IN: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: SHAPE_BYTES as u64,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32, 2 => Uint32, 3 => Float32, 4 => Uint32],
};

impl Canvas {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, kind: Kind) -> Canvas {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shapes"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shapes frame"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shapes"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        // Each shape over what's there, its colour premultiplied (as the processor's painter lays one over another).
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shapes"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("shape_vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(SHAPE_IN)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("shape_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: PICTURE,
                    blend: Some(wgpu::BlendState { color: over, alpha: over }),
                    // One colour: only how much shows is drawn, the colour staying the background's.
                    write_mask: match kind {
                        Kind::Picture => wgpu::ColorWrites::ALL,
                        Kind::OneColour => wgpu::ColorWrites::ALPHA,
                    },
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shapes frame"),
            size: FRAME_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shapes frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame.as_entire_binding() }],
        });
        let most = (device.limits().max_buffer_size / SHAPE_BYTES as u64).clamp(1, MOST_AT_ONCE as u64) as usize;
        Canvas {
            device: device.clone(),
            queue: queue.clone(),
            kind,
            pipeline,
            frame,
            frame_group,
            shapes: None,
            most,
            pictures: None,
        }
    }

    /// A picture `width` by `height` pixels to draw into.
    fn picture(&self, width: u32, height: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shapes picture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PICTURE,
            // Shown by Slint, which needs both; copied out by the look tests.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// The pictures for `width` by `height` pixels, made again when the size changes.
    fn pictures(&mut self, width: u32, height: u32) -> &mut Pictures {
        if self.pictures.as_ref().is_none_or(|p| p.width != width || p.height != height) {
            let textures = [self.picture(width, height), self.picture(width, height)];
            self.pictures = Some(Pictures { width, height, textures, next: 0 });
        }
        self.pictures.get_or_insert_with(|| unreachable!("made just above"))
    }

    /// A picture's size on this card: `width` by `height` pixels, or as big as the card can hold at a scale to fit (by
    /// how much, the last number), at least a pixel each way.
    fn fit(&self, (width, height): (u32, u32)) -> (u32, u32, f32) {
        let most = self.device.limits().max_texture_dimension_2d.clamp(1, shapes::MOST_PIXELS);
        let fit = (most as f32 / width.max(1) as f32).min(most as f32 / height.max(1) as f32).min(1.0);
        if fit < 1.0 {
            let fitted = |v: u32| ((v as f32 * fit).floor() as u32).clamp(1, most);
            (fitted(width), fitted(height), fit)
        } else {
            (width.max(1), height.max(1), 1.0)
        }
    }

    /// Draws `shapes` (as `mo_app::shapes::Scene::pack` packed them for `scale`) at `scale` pixels to a point, on a
    /// picture `width` by `height` pixels, over `background` (0xRRGGBB), and gives the picture for Slint to show. A
    /// picture bigger than the card can hold is drawn as big as it can, at a scale to fit, and Slint stretches it.
    /// Drawn into one of two pictures in turn, so the one drawn before is the one to show until this one is. `fringe`
    /// is as for [`Canvas::draw_new`]: 0 for lines soft as the processor's, or the width of the window's soft edges, in
    /// pixels, for lines drawn as it draws a path's.
    pub(crate) fn draw(
        &mut self,
        size: (u32, u32),
        scale: f32,
        background: u32,
        shapes: &[u8],
        fringe: f32,
    ) -> wgpu::Texture {
        let (width, height, fit) = self.fit(size);
        let pictures = self.pictures(width, height);
        let picture = pictures.textures[pictures.next].clone();
        pictures.next = 1 - pictures.next;
        self.draw_on(&picture, (scale, fit), background, shapes, fringe * fit);
        picture
    }

    /// As [`Canvas::draw`], on a picture of its own, for pictures that are kept (a waveform's), with its lines drawn as
    /// the window's drawing library draws a path's, their soft edges `fringe` pixels wide.
    pub(crate) fn draw_new(
        &mut self,
        size: (u32, u32),
        scale: f32,
        background: u32,
        shapes: &[u8],
        fringe: f32,
    ) -> wgpu::Texture {
        let (width, height, fit) = self.fit(size);
        let picture = self.picture(width, height);
        self.draw_on(&picture, (scale, fit), background, shapes, fringe * fit);
        picture
    }

    /// Draws `shapes` (packed for `scale`) on `picture` at `scale` pixels to a point, shrunk by `fit` to fit the
    /// picture, over `background`; lines with soft edges `fringe` pixels wide, as the window's drawing library draws
    /// them, unless it's 0.
    fn draw_on(
        &mut self,
        picture: &wgpu::Texture,
        (scale, fit): (f32, f32),
        background: u32,
        shapes: &[u8],
        fringe: f32,
    ) {
        let frame = [picture.width() as f32, picture.height() as f32, scale * fit, fit, fringe, 0.0, 0.0, 0.0];
        let bytes: Vec<u8> = frame.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.queue.write_buffer(&self.frame, 0, &bytes);
        let shapes = &shapes[..shapes.len() / SHAPE_BYTES * SHAPE_BYTES];
        let target = picture.create_view(&wgpu::TextureViewDescriptor::default());
        let channel = |shift: u32| f64::from((background >> shift) & 0xFF) / 255.0;
        // One colour starts as its colour with none of it showing.
        let shows = if self.kind == Kind::OneColour { 0.0 } else { 1.0 };
        let clear = wgpu::Color { r: channel(16), g: channel(8), b: channel(0), a: shows };
        // The first part goes over the background; with no shapes at all, that's all there is.
        let parts: Vec<&[u8]> =
            if shapes.is_empty() { vec![&[]] } else { shapes.chunks(self.most * SHAPE_BYTES).collect() };
        for (i, part) in parts.into_iter().enumerate() {
            self.draw_part(&target, if i == 0 { wgpu::LoadOp::Clear(clear) } else { wgpu::LoadOp::Load }, part);
        }
    }

    /// Draws up to [`Canvas::most`] shapes on `target`, over what `load` leaves there. Each part is sent to the card on
    /// its own, so the next part can use the same buffer.
    fn draw_part(&mut self, target: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>, part: &[u8]) {
        let count = part.len() / SHAPE_BYTES;
        if count > 0 && self.shapes.as_ref().is_none_or(|b| b.size() < part.len() as u64) {
            // Room for half as many again, so a view that grows a little doesn't make a new one every time.
            let room = (count + count / 2).min(self.most);
            self.shapes = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shapes"),
                // A whole number of shapes is always a whole number of the 4 bytes the card copies at a time.
                size: (room * SHAPE_BYTES) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if count > 0
            && let Some(b) = &self.shapes
        {
            self.queue.write_buffer(b, 0, part);
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shapes") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shapes"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if count > 0
                && let Some(b) = &self.shapes
            {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.frame_group, &[]);
                pass.set_vertex_buffer(0, b.slice(..part.len() as u64));
                pass.draw(0..6, 0..count as u32);
            }
        }
        // Sent now: the next part's shapes go into the same buffer, and the card copies them in only at the next send.
        self.queue.submit(Some(encoder.finish()));
    }
}

/// A view's picture as its drawing thread leaves it for the window's thread to put up ([`show`]): drawn already, by the
/// processor, or its shapes packed for the graphics card to draw.
pub(crate) enum Drawing {
    Pixels(SharedPixelBuffer<Rgba8Pixel>),
    Shapes {
        bytes: Vec<u8>,
        /// The picture's size in pixels, and pixels to a point.
        size: (u32, u32),
        scale: f32,
        background: u32,
    },
}

impl Drawing {
    /// `scene` at `scale` pixels to a point: packed for the card when the card draws the view (`on_card`), else drawn
    /// now by the processor, in the window's kind of picture.
    pub(crate) fn new(scene: &Scene, scale: f64, on_card: bool) -> Drawing {
        let scale = shapes::usable_scale(scale);
        if on_card {
            Drawing::Shapes {
                bytes: scene.pack(scale),
                size: scene.pixel_size(scale),
                scale: scale as f32,
                background: scene.background,
            }
        } else {
            let pixmap = scene.paint(scale);
            Drawing::Pixels(SharedPixelBuffer::clone_from_slice(pixmap.data(), pixmap.width(), pixmap.height()))
        }
    }
}

/// What a view's steps on the window's thread are called in the freeze lines (`watchdog::doing`).
pub(crate) struct Steps {
    /// Setting its painter up on the card, which can take the card a moment: only when that couldn't be done ahead of
    /// time, when the card was handed over (`gpu::prepare`).
    pub(crate) set_up: &'static str,
    /// Drawing its shapes on the card.
    pub(crate) draw: &'static str,
    /// Handing the processor's picture to the window.
    pub(crate) show: &'static str,
}

/// The picture for Slint to show from `drawing`, made on the window's thread: the processor's as it is, or the shapes
/// drawn on the card now by the view's painter in `painter` (made first if it isn't yet), into one of its two pictures
/// in turn. `Ok(None)` when the shapes were packed for a card that has gone since, or won't draw the view any more: the
/// view asks again. `Err` says why the card or Slint said no; the card is then marked as refusing the view, which the
/// processor draws from then on, while that card lasts.
pub(crate) fn show(
    drawing: Drawing,
    painter: &'static LocalKey<RefCell<gpu::Painter<Canvas>>>,
    steps: &Steps,
) -> Result<Option<Image>, String> {
    let (bytes, size, scale, background) = match drawing {
        Drawing::Pixels(pixels) => {
            let _doing = watchdog::doing(steps.show);
            return Ok(Some(Image::from_rgba8_premultiplied(pixels)));
        }
        Drawing::Shapes { bytes, size, scale, background } => (bytes, size, scale, background),
    };
    let Some((gpu, generation)) = painter.with(|p| p.borrow().card()) else { return Ok(None) };
    let image = painter.with(|p| -> Result<_, String> {
        let mut p = p.borrow_mut();
        let canvas = p.on(&gpu, generation, |gpu| {
            let _doing = watchdog::doing(steps.set_up);
            Canvas::new(&gpu.device, &gpu.queue, Kind::Picture)
        })?;
        let _doing = watchdog::doing(steps.draw);
        let texture = gpu::checked(&gpu.device, || canvas.draw(size, scale, background, &bytes, 0.0))?;
        Image::try_from(texture).map_err(|e| format!("Slint wouldn't show the picture: {e}"))
    });
    image.map(Some).inspect_err(|_| painter.with(|p| p.borrow_mut().refused(generation)))
}

/// A colour of the window's (its theme's, say) as the painters take it.
pub(crate) fn colour(c: slint::Color) -> Colour {
    Colour { r: c.red(), g: c.green(), b: c.blue(), a: c.alpha() }
}

/// A colour of the window's as 0xRRGGBB, for the colour under a picture (which is solid, so how much shows is left out).
pub(crate) fn rgb(c: slint::Color) -> u32 {
    u32::from(c.red()) << 16 | u32::from(c.green()) << 8 | u32::from(c.blue())
}

/// Puts `rows` in `model` in place of what's there, changing only the lines that differ, so the window keeps the words
/// it has laid on a view instead of making them all again.
pub(crate) fn refill<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    let old = model.row_count();
    let keep = old.min(rows.len());
    let mut rows = rows.into_iter();
    for (i, row) in rows.by_ref().take(keep).enumerate() {
        if model.row_data(i).as_ref() != Some(&row) {
            model.set_row_data(i, row);
        }
    }
    model.extend(rows);
    for i in (keep..old).rev() {
        model.remove(i);
    }
}

/// For the tests that draw on a graphics card: a card made the way Slint makes the window's, and a picture read back.
#[cfg(test)]
pub(crate) mod testing {
    use crate::gpu;
    use slint::wgpu_30::wgpu;

    /// A card to draw on, made the way Slint makes the window's (WebGL 2's limits, raised only for the size of a
    /// picture), so a shape the card on Solomon's PC would refuse fails here too. A software card will do.
    pub(crate) fn card() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = gpu::wait(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        eprintln!("{:?}", adapter.get_info());
        let descriptor = wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..wgpu::DeviceDescriptor::default()
        };
        gpu::wait(adapter.request_device(&descriptor)).ok()
    }

    /// The card's picture, read back.
    pub(crate) fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, picture: &wgpu::Texture) -> image::RgbaImage {
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
        let data = buffer.slice(..).get_mapped_range().expect("mapped");
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let at = (y * row + x * 4) as usize;
                img.put_pixel(x, y, image::Rgba([data[at], data[at + 1], data[at + 2], data[at + 3]]));
            }
        }
        img
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{card, read_back};
    use super::*;
    use wgpu::naga;

    /// A view with more shapes than the card takes at once is drawn a part at a time, and comes out the same as in one
    /// go: each part goes over the one before, see-through shapes and all. Needs a graphics card (a software one will
    /// do), so it's run by hand.
    #[test]
    #[ignore]
    fn a_big_view_is_drawn_a_part_at_a_time() {
        let Some((device, queue)) = card() else {
            eprintln!("No graphics card here, not even a software one: nothing to draw on");
            return;
        };
        let mut scene = Scene::new(300.0, 120.0, 0x1D1D1D);
        let edge = Some(Colour::rgb(0xFFFFFF, 0.5));
        for i in 0..400u32 {
            let at = f64::from(i);
            let tint = Colour::rgb(0x3060FF ^ (i * 0x0A0B0C), 0.35);
            scene.rect(((at * 7.3) % 280.0, (at * 3.1) % 100.0, 18.0, 14.0), tint);
            scene.rounded(((at * 5.7) % 270.0, (at * 2.3) % 90.0, 26.0, 20.0), 4.0, tint, edge);
            scene.line(((at * 4.1) % 300.0, 0.0), ((at * 6.7) % 300.0, 120.0), 1.5, tint);
        }
        assert_eq!(scene.shapes.len(), 1200);
        let bytes = scene.pack(1.5);
        let size = scene.pixel_size(1.5);
        let mut canvas = Canvas::new(&device, &queue, Kind::Picture);
        let whole = read_back(&device, &queue, &canvas.draw(size, 1.5, scene.background, &bytes, 0.0));
        // 7 at a time: 172 parts.
        let mut small = Canvas::new(&device, &queue, Kind::Picture);
        small.most = 7;
        let parts = read_back(&device, &queue, &small.draw(size, 1.5, scene.background, &bytes, 0.0));
        assert!(whole == parts, "drawn a part at a time, the picture came out different");
        assert_eq!(small.shapes.as_ref().map(wgpu::Buffer::size), Some(7 * SHAPE_BYTES as u64), "room for one part");
    }

    #[test]
    fn the_words_change_line_by_line() {
        let words = |list: &[&str]| list.iter().map(|w| slint::SharedString::from(*w)).collect::<Vec<_>>();
        let model = VecModel::from(words(&["a", "b", "c"]));
        refill(&model, words(&["a", "x"]));
        assert_eq!(model.iter().collect::<Vec<_>>(), words(&["a", "x"]));
        refill(&model, words(&["a", "x", "y", "z"]));
        assert_eq!(model.iter().collect::<Vec<_>>(), words(&["a", "x", "y", "z"]));
        refill(&model, Vec::new());
        assert_eq!(model.row_count(), 0);
    }

    /// The card only builds the shader when the window opens, so a mistake in it would only show on Solomon's PC. This
    /// checks it here the way wgpu does.
    #[test]
    fn the_shapes_shader_is_valid() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("the shader parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("the shader is valid");
    }

    #[test]
    fn the_shader_knows_the_kinds_of_shape_by_the_same_numbers() {
        for (name, value) in [("RECT", shapes::RECT), ("ROUNDED", shapes::ROUNDED), ("LINE", shapes::LINE)] {
            assert!(SHADER.contains(&format!("const {name}: u32 = {value}u;")), "{name} isn't {value} in the shader");
        }
    }

    /// Slint makes its card with WebGL 2's limits (raised only for the size of a picture), so what a shape takes must
    /// fit them, or the card would refuse to draw on Solomon's PC.
    #[test]
    fn a_shape_fits_the_cards_slint_makes() {
        let limits = wgpu::Limits::downlevel_webgl2_defaults();
        assert!(SHAPE_BYTES as u32 <= limits.max_vertex_buffer_array_stride);
        assert!(SHAPE_IN.attributes.len() as u32 <= limits.max_vertex_attributes);
        let last = SHAPE_IN.attributes.iter().map(|a| a.offset + a.format.size()).max().unwrap_or(0);
        assert_eq!(last, SHAPE_BYTES as u64, "the attributes fill the shape exactly");
        // The shader hands five things from corner to pixel.
        assert!(5 <= limits.max_inter_stage_shader_variables);
    }
}
