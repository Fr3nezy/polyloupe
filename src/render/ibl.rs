//! Image-based lighting: turns an equirectangular HDR environment into the textures the
//! Rendered mode samples (prefiltered specular mips, diffuse irradiance, BRDF LUT).

use eframe::egui_wgpu::wgpu;

use crate::loader::EnvImage;

pub const SRC_WIDTH: u32 = 2048;
pub const SRC_HEIGHT: u32 = 1024;
const SRC_MIPS: u32 = 11; // 2048 -> 1
const SPEC_WIDTH: u32 = 512;
const SPEC_HEIGHT: u32 = 256;
pub const SPEC_MIPS: u32 = 6;
const IRR_WIDTH: u32 = 64;
const IRR_HEIGHT: u32 = 32;
const LUT_SIZE: u32 = 128;
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub struct Ibl {
    src: wgpu::Texture,
    specular: wgpu::Texture,
    pub src_view: wgpu::TextureView,
    pub specular_view: wgpu::TextureView,
    pub irradiance_view: wgpu::TextureView,
    pub brdf_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    downsample: wgpu::RenderPipeline,
    prefilter: wgpu::RenderPipeline,
    irradiance_pipe: wgpu::RenderPipeline,
}

impl Ibl {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let tex = |label, w, h, mips, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let src = tex("env src", SRC_WIDTH, SRC_HEIGHT, SRC_MIPS, HDR_FORMAT);
        let specular = tex("env specular", SPEC_WIDTH, SPEC_HEIGHT, SPEC_MIPS, HDR_FORMAT);
        let irradiance = tex("env irradiance", IRR_WIDTH, IRR_HEIGHT, 1, HDR_FORMAT);
        let brdf = tex("brdf lut", LUT_SIZE, LUT_SIZE, 1, wgpu::TextureFormat::Rg16Float);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("env"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ibl"),
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
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ibl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/ibl.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ibl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipe = |fs: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let downsample = pipe("fs_downsample", HDR_FORMAT);
        let prefilter = pipe("fs_prefilter", HDR_FORMAT);
        let irradiance_pipe = pipe("fs_irradiance", HDR_FORMAT);
        let brdf_pipe = pipe("fs_brdf", wgpu::TextureFormat::Rg16Float);

        let ibl = Self {
            src_view: src.create_view(&Default::default()),
            specular_view: specular.create_view(&Default::default()),
            irradiance_view: irradiance.create_view(&Default::default()),
            brdf_view: brdf.create_view(&Default::default()),
            src,
            specular,
            sampler,
            layout,
            downsample,
            prefilter,
            irradiance_pipe,
        };

        // The BRDF LUT doesn't depend on the environment: compute it once.
        let mut encoder = device.create_command_encoder(&Default::default());
        let dummy = ibl.bind(device, &ibl.src_view, [0.0; 4]);
        ibl.pass(&mut encoder, &brdf_pipe, &ibl.brdf_view, &dummy);
        queue.submit(Some(encoder.finish()));
        ibl
    }

    fn bind(&self, device: &wgpu::Device, view: &wgpu::TextureView, params: [f32; 4]) -> wgpu::BindGroup {
        let buf = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("ibl params"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ibl"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: buf.as_entire_binding() },
            ],
        })
    }

    fn pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        target: &wgpu::TextureView,
        bind: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ibl"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }

    fn mip_view(tex: &wgpu::Texture, mip: u32) -> wgpu::TextureView {
        tex.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: mip,
            mip_level_count: Some(1),
            ..Default::default()
        })
    }

    /// Uploads a new environment and rebuilds every derived texture on the GPU.
    pub fn set_environment(&self, device: &wgpu::Device, queue: &wgpu::Queue, env: &EnvImage) {
        let env = fit(env, SRC_WIDTH, SRC_HEIGHT);
        let halfs: Vec<u16> = env.iter().flat_map(|p| p.map(f32_to_f16)).collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.src,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&halfs),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SRC_WIDTH * 8),
                rows_per_image: Some(SRC_HEIGHT),
            },
            wgpu::Extent3d { width: SRC_WIDTH, height: SRC_HEIGHT, depth_or_array_layers: 1 },
        );

        let mut encoder = device.create_command_encoder(&Default::default());
        for mip in 1..SRC_MIPS {
            let source = Self::mip_view(&self.src, mip - 1);
            let target = Self::mip_view(&self.src, mip);
            let bind = self.bind(device, &source, [0.0; 4]);
            self.pass(&mut encoder, &self.downsample, &target, &bind);
        }
        let src_params = |rough: f32| [rough, SRC_WIDTH as f32, SRC_HEIGHT as f32, SRC_MIPS as f32];
        for mip in 0..SPEC_MIPS {
            let rough = mip as f32 / (SPEC_MIPS - 1) as f32;
            let target = Self::mip_view(&self.specular, mip);
            let bind = self.bind(device, &self.src_view, src_params(rough.max(0.02)));
            self.pass(&mut encoder, &self.prefilter, &target, &bind);
        }
        let bind = self.bind(device, &self.src_view, src_params(1.0));
        self.pass(&mut encoder, &self.irradiance_pipe, &self.irradiance_view, &bind);
        queue.submit(Some(encoder.finish()));
    }
}

/// Resamples an environment to the fixed source resolution (bilinear).
fn fit(env: &EnvImage, width: u32, height: u32) -> Vec<[f32; 4]> {
    if env.width == width && env.height == height {
        return env.pixels.clone();
    }
    let mut out = Vec::with_capacity((width * height) as usize);
    let (sw, sh) = (env.width as usize, env.height as usize);
    let at = |x: usize, y: usize| env.pixels[y.min(sh - 1) * sw + (x % sw)];
    for y in 0..height {
        let fy = ((y as f32 + 0.5) / height as f32 * sh as f32 - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        for x in 0..width {
            let fx = ((x as f32 + 0.5) / width as f32 * sw as f32 - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            let mut p = [0.0; 4];
            for i in 0..4 {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bottom = c[i] + (d[i] - c[i]) * tx;
                p[i] = top + (bottom - top) * ty;
            }
            out.push(p);
        }
    }
    out
}

/// IEEE 754 half-float conversion with rounding and clamping to the finite range.
pub fn f32_to_f16(v: f32) -> u16 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(-65000.0, 65000.0) };
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = bits & 0x7f_ffff;
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    let half = sign as u32 | ((exp as u32) << 10) | (mant >> 13);
    // Round to nearest.
    (half + ((mant >> 12) & 1)) as u16
}
