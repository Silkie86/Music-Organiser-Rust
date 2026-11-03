//! The graphics card's painter for the views the app draws itself (the arrangement first): it draws the
//! [`mo_app::shapes::Scene`] a view works out, which the processor draws the same way where there's no card
//! (`Scene::paint`). A picture is one draw on the card: every shape is two triangles, which the card fills with its
//! colour, cutting the corners round and softening the edges where the shape asks for it, each shape over the ones
//! before.

use mo_app::shapes::{self, SHAPE_BYTES};
use slint::wgpu_30::wgpu;

pub(crate) const SHADER: &str = r#"
struct Frame {
    // The picture's size in pixels, and pixels to a point.
    size: vec2<f32>,
    scale: f32,
    _a: f32,
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
    // A rectangle's left, top, right and bottom, or a line's two ends, in pixels.
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
        // Its edges on whole pixels, taking the pixels whose middles are inside (one on the left or top edge in, one on
        // the right or bottom edge out), as mo_app::shapes::crisp works them out for the processor.
        p = mix(ceil(px.xy - vec2(0.5)), ceil(px.zw - vec2(0.5)), c);
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
    let fill = in.fill * clamp(0.5 - out_by, 0.0, 1.0);
    // The edge: a line a point wide along the border, over the fill.
    let edge = in.edge * clamp(0.5 * frame.scale + 0.5 - abs(out_by), 0.0, 1.0);
    return edge + fill * (1.0 - edge.a);
}
"#;

const PICTURE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The frame as the shader reads it (`Frame`): 4 f32.
const FRAME_BYTES: u64 = 16;
/// The most shapes drawn at once (32 MB of them): a view with more is drawn a part at a time, each over the one before,
/// so a huge song doesn't need a huge buffer on the card (or one bigger than the card allows).
const MOST_AT_ONCE: usize = 1 << 20;

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
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Canvas {
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
                    write_mask: wgpu::ColorWrites::ALL,
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
            pipeline,
            frame,
            frame_group,
            shapes: None,
            most,
            pictures: None,
        }
    }

    /// The pictures for `width` by `height` pixels, made again when the size changes.
    fn pictures(&mut self, width: u32, height: u32) -> &mut Pictures {
        if self.pictures.as_ref().is_none_or(|p| p.width != width || p.height != height) {
            let make = || {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("shapes picture"),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: PICTURE,
                    // Shown by Slint, which needs both; copied out by the look test.
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            };
            self.pictures = Some(Pictures { width, height, textures: [make(), make()], next: 0 });
        }
        self.pictures.get_or_insert_with(|| unreachable!("made just above"))
    }

    /// Draws `shapes` (as `mo_app::shapes::Scene::pack` packed them) at `scale` pixels to a point, on a picture
    /// `width` by `height` pixels, over `background` (0xRRGGBB), and gives the picture for Slint to show. A picture
    /// bigger than the card can hold is drawn as big as it can, at a scale to fit, and Slint stretches it.
    pub(crate) fn draw(
        &mut self,
        (width, height): (u32, u32),
        scale: f32,
        background: u32,
        shapes: &[u8],
    ) -> wgpu::Texture {
        let most = self.device.limits().max_texture_dimension_2d.clamp(1, shapes::MOST_PIXELS);
        let fit = (most as f32 / width.max(1) as f32).min(most as f32 / height.max(1) as f32).min(1.0);
        let (width, height) = if fit < 1.0 {
            (
                ((width as f32 * fit).floor() as u32).clamp(1, most),
                ((height as f32 * fit).floor() as u32).clamp(1, most),
            )
        } else {
            (width.max(1), height.max(1))
        };
        let frame = [width as f32, height as f32, scale * fit, 0.0];
        let bytes: Vec<u8> = frame.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.queue.write_buffer(&self.frame, 0, &bytes);
        let shapes = &shapes[..shapes.len() / SHAPE_BYTES * SHAPE_BYTES];
        let pictures = self.pictures(width, height);
        let picture = pictures.textures[pictures.next].clone();
        pictures.next = 1 - pictures.next;
        let target = picture.create_view(&wgpu::TextureViewDescriptor::default());
        let channel = |shift: u32| f64::from((background >> shift) & 0xFF) / 255.0;
        let clear = wgpu::Color { r: channel(16), g: channel(8), b: channel(0), a: 1.0 };
        // The first part goes over the background; with no shapes at all, that's all there is.
        let parts: Vec<&[u8]> =
            if shapes.is_empty() { vec![&[]] } else { shapes.chunks(self.most * SHAPE_BYTES).collect() };
        for (i, part) in parts.into_iter().enumerate() {
            self.draw_part(&target, if i == 0 { wgpu::LoadOp::Clear(clear) } else { wgpu::LoadOp::Load }, part);
        }
        picture
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
    use mo_app::shapes::{Colour, Scene};
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
        let bytes = scene.pack();
        let size = scene.pixel_size(1.5);
        let mut canvas = Canvas::new(&device, &queue);
        let whole = read_back(&device, &queue, &canvas.draw(size, 1.5, scene.background, &bytes));
        // 7 at a time: 172 parts.
        let mut small = Canvas::new(&device, &queue);
        small.most = 7;
        let parts = read_back(&device, &queue, &small.draw(size, 1.5, scene.background, &bytes));
        assert!(whole == parts, "drawn a part at a time, the picture came out different");
        assert_eq!(small.shapes.as_ref().map(wgpu::Buffer::size), Some(7 * SHAPE_BYTES as u64), "room for one part");
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

    /// The card puts a sharp shape's edges on whole pixels by the processor's rule (`mo_app::shapes::crisp`), or the
    /// two pictures would differ by a pixel along the edges. The look test checks the pictures, but needs a card; this
    /// checks the shader still has the rule where there's none (as on the computers that check each pull request).
    #[test]
    fn the_shader_puts_sharp_edges_on_pixels_as_the_processor_does() {
        // crisp: an edge at v points goes to ceil(v * scale - 0.5) pixels.
        assert_eq!(shapes::crisp([0.5, 0.5, 1.5, 2.5], 1.0), Some([0.0, 0.0, 1.0, 2.0]));
        assert!(SHADER.contains("let s = frame.scale;\n    let px = at * s;"), "the shader's edges aren't in pixels");
        assert!(
            SHADER.contains("if (kind == RECT) {")
                && SHADER.contains("p = mix(ceil(px.xy - vec2(0.5)), ceil(px.zw - vec2(0.5)), c);"),
            "the shader's sharp edges aren't on whole pixels as shapes::crisp puts them"
        );
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
