//! Draws the stars on the graphics card with wgpu, into a picture Slint shows like any other image. Each star is a
//! small soft dot; all 300,000 go to the card once, and each frame only sends the camera (16 bytes).

use slint::wgpu_30::wgpu;

use crate::sky::{Star, View};

const SHADER: &str = r#"
struct Camera {
    centre: vec2<f32>,
    // Sky units to clip space, x and y.
    scale: vec2<f32>,
    // One pixel in clip space, x and y.
    pixel: vec2<f32>,
    // How much bigger stars get as the camera goes in.
    grow: f32,
    _pad: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

@vertex
fn star_vertex(@builtin(vertex_index) v: u32, @location(0) at: vec2<f32>, @location(1) colour: u32, @location(2) size: f32) -> Out {
    var corners = array<vec2<f32>, 6>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let corner = corners[v];
    let centre = (at - camera.centre) * camera.scale;
    let radius = min(size * camera.grow, 24.0);
    var out: Out;
    out.position = vec4(centre + corner * radius * camera.pixel, 0.0, 1.0);
    out.corner = corner;
    out.colour = unpack4x8unorm(colour);
    return out;
}

@fragment
fn star_fragment(in: Out) -> @location(0) vec4<f32> {
    let d = length(in.corner);
    let glow = clamp(1.0 - d, 0.0, 1.0);
    let a = in.colour.a * glow * glow;
    return vec4(in.colour.rgb * a, a);
}
"#;

pub struct StarPainter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    stars: wgpu::Buffer,
    count: u32,
    camera: wgpu::Buffer,
    bind: wgpu::BindGroup,
    /// Two pictures, so one is drawn into while Slint shows the other.
    pictures: [Option<wgpu::Texture>; 2],
    next: usize,
}

impl StarPainter {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, stars: &[Star]) -> StarPainter {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stars"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
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
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("stars"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stars"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("star_vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32, 2 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("star_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    // Stars add up where they crowd, so dense clusters glow.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let star_bytes = Star::bytes(stars);
        let star_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stars"),
            size: star_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&star_buffer, 0, &star_bytes);
        StarPainter {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            stars: star_buffer,
            count: stars.len() as u32,
            camera,
            bind,
            pictures: [None, None],
            next: 0,
        }
    }

    /// Draws the sky from `view` into a picture `width` by `height` pixels, and returns it for Slint to show.
    pub fn draw(&mut self, view: View, width: u32, height: u32, background: [f64; 3]) -> wgpu::Texture {
        let (width, height) = (width.max(1), height.max(1));
        let slot = &mut self.pictures[self.next];
        if slot.as_ref().is_none_or(|t| t.width() != width || t.height() != height) {
            *slot = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sky"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
        }
        let texture = slot.clone().expect("made just above");
        self.next = 1 - self.next;

        // The whole sky (-1..1) fits the shorter side when zoom is 1.
        let fit = width.min(height) as f32;
        let scale = [view.zoom * fit / width as f32, view.zoom * fit / height as f32];
        let pixel = [2.0 / width as f32, 2.0 / height as f32];
        let grow = 0.6 + 0.4 * (view.zoom + 1.0).log2();
        let camera: [f32; 8] = [view.x, view.y, scale[0], scale[1], pixel[0], pixel[1], grow, 0.0];
        let bytes: Vec<u8> = camera.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.queue.write_buffer(&self.camera, 0, &bytes);

        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sky") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: background[0],
                            g: background[1],
                            b: background[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.stars.slice(..));
            pass.draw(0..6, 0..self.count);
        }
        self.queue.submit(Some(encoder.finish()));
        texture
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::naga;

    /// The shader is only compiled by the graphics card when the window opens, so a mistake in it would only show on
    /// Solomon's PC. This checks it here, the same way wgpu does.
    #[test]
    fn the_star_shader_is_valid() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("the shader parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("the shader is valid");
    }
}
