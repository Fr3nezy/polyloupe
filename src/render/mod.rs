//! wgpu viewport renderer. Draws into an offscreen MSAA target that egui shows as an image,
//! so the 3D view composes with the UI without owning the window surface.

pub mod environment;
pub mod ibl;
pub mod matcap;

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::loader::EnvImage;
use crate::scene::{AlphaMode, Scene};
use crate::settings::{ColorMode, Lighting, MetalFinish, PartMaterial, Settings, ShadingMode, TexturePass, ViewTransform};

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// egui expects gamma-encoded texels, so it samples the same memory through a non-sRGB view.
const EGUI_VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
const PICK_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const SAMPLES: u32 = 4;

/// Viewport background, sRGB.
pub const BACKGROUND: [u8; 3] = [0x26, 0x26, 0x26];
/// Blender's selection colors: active object is lighter than the rest of the selection.
const SELECTED: [f32; 4] = [0.93, 0.34, 0.0, 1.0];
const ACTIVE: [f32; 4] = [1.0, 0.67, 0.25, 1.0];

const HAS_UV: u32 = 1;
const HAS_TANGENT: u32 = 2;
const HAS_COLOR: u32 = 4;
const FLAG_SELECTED: u32 = 8;
const FLAG_ACTIVE: u32 = 16;
const FLAG_SKINNED: u32 = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightDataUniform {
    dir: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlobalsUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    eye: [f32; 4],
    cam_back: [f32; 4],
    shading: [u32; 4],
    extra: [u32; 4],
    params: [f32; 4],
    env: [f32; 4],
    grid: [f32; 4],
    wire_color: [f32; 4],
    selected_color: [f32; 4],
    active_color: [f32; 4],
    viewport: [f32; 4],
    markers: [u32; 4],
    section: [f32; 4],
    normals: [f32; 4],
    display: [f32; 4],
    finish: [f32; 4],
    finish_color: [f32; 4],
    surface: [f32; 4],
    light0: [[f32; 4]; 4],
    light1: [[f32; 4]; 4],
    lights: [LightDataUniform; 6],
    shadow: [f32; 4],
    floor: [f32; 4],
}

/// Shadow map size and format; two layers: the key light and a straight-down view.
const SHADOW_SIZE: u32 = 2048;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Where the scene is, for fitting the shadow maps and placing the floor: bounds center and
/// radius, and the lowest point (the floor height).
#[derive(Clone, Copy)]
pub struct SceneFrame {
    pub center: Vec3,
    pub radius: f32,
    pub floor: f32,
}

struct Shadows {
    /// One depth view per layer to render into.
    layers: [wgpu::TextureView; 2],
    /// Per layer: the light matrix, and its bind group for the shadow pass.
    matrices: [wgpu::Buffer; 2],
    bind_groups: [wgpu::BindGroup; 2],
    pipeline: wgpu::RenderPipeline,
}

/// Surface wear maps (CC0, ambientCG; see assets/surface/LICENSE.txt), packed by channel:
/// grain = normal XY, roughness, dust; scratches = normal XY, scratch mask, brushed mask.
const SURFACE_MAPS: [&[u8]; 2] = [include_bytes!("../../assets/surface/grain.png"), include_bytes!("../../assets/surface/scratches.png")];
const SURFACE_SIZE: u32 = 1024;
const SURFACE_MIPS: u32 = 11;

/// The surface maps are decoded on first use, on a worker thread, so they never slow startup.
enum SurfaceMaps {
    Unused,
    Decoding(std::sync::mpsc::Receiver<Vec<Vec<Vec<u8>>>>),
    Ready,
}

/// One mesh analysis marker (see `qa`): an edge from `a` to `b`, or a vertex at `a`.
/// Skinning data travels along so markers follow the animation.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MarkerInstance {
    a: [f32; 3],
    /// 0 non-manifold edge, 1 open edge, 2 overlapping vertex.
    kind: u32,
    b: [f32; 3],
    _pad: u32,
    joints_a: [u16; 4],
    joints_b: [u16; 4],
    weights_a: [f32; 4],
    weights_b: [f32; 4],
}

/// Marker instances for one mesh.
pub fn marker_instances(mesh: &crate::scene::Mesh, marks: &crate::qa::MeshMarks) -> Vec<MarkerInstance> {
    let rig = &mesh.rig;
    let skin = |v: u32| -> ([u16; 4], [f32; 4]) {
        match (&rig.joints, &rig.weights) {
            (Some(j), Some(w)) if rig.skin.is_some() => (j[v as usize], w[v as usize]),
            _ => ([0; 4], [0.0; 4]),
        }
    };
    let make = |kind: u32, a: u32, b: u32| {
        let (joints_a, weights_a) = skin(a);
        let (joints_b, weights_b) = skin(b);
        MarkerInstance {
            a: mesh.positions[a as usize],
            kind,
            b: mesh.positions[b as usize],
            _pad: 0,
            joints_a,
            joints_b,
            weights_a,
            weights_b,
        }
    };
    let mut out = Vec::with_capacity(marks.non_manifold.len() + marks.open.len() + marks.overlapping.len());
    out.extend(marks.open.iter().map(|&[a, b]| make(1, a, b)));
    out.extend(marks.non_manifold.iter().map(|&[a, b]| make(0, a, b)));
    out.extend(marks.overlapping.iter().map(|&v| make(2, v, v)));
    out
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    normal_mat: [[f32; 4]; 4],
    color: [f32; 4],
    info: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    base_color: [f32; 4],
    emissive: [f32; 4],
    pbr: [f32; 4],
    channels: [u32; 4],
    alpha: [f32; 4],
    extra: [f32; 4],
}

const OBJECT_SIZE: u64 = std::mem::size_of::<ObjectUniform>() as u64;

/// Everything in a `FrameInput` that shows in the image, kept to tell an unchanged frame.
#[derive(PartialEq)]
struct FrameKey {
    size: [u32; 2],
    view: Mat4,
    proj: Mat4,
    eye: Vec3,
    cam_back: Vec3,
    ortho: bool,
    grid: [f32; 2],
    grid_axis: u32,
    settings: Settings,
    transparent: bool,
    section: Option<[f32; 4]>,
    normal_length: f32,
    print_scale: f32,
    scene: Option<[f32; 5]>,
}

impl FrameKey {
    fn new(size: [u32; 2], input: &FrameInput) -> Self {
        Self {
            size,
            view: input.view,
            proj: input.proj,
            eye: input.eye,
            cam_back: input.cam_back,
            ortho: input.ortho,
            grid: [input.grid_cell, input.grid_fade],
            grid_axis: input.grid_axis,
            settings: input.settings.clone(),
            transparent: input.transparent,
            section: input.section,
            normal_length: input.normal_length,
            print_scale: input.print_scale,
            scene: input.scene.map(|f| [f.center.x, f.center.y, f.center.z, f.radius, f.floor]),
        }
    }
}

pub struct FrameInput<'a> {
    pub view: Mat4,
    pub proj: Mat4,
    pub eye: Vec3,
    pub cam_back: Vec3,
    pub ortho: bool,
    pub grid_cell: f32,
    pub grid_fade: f32,
    /// Normal axis of the floor plane: 2 = XY floor, 1 = XZ (front views), 0 = YZ (side views).
    pub grid_axis: u32,
    pub settings: &'a Settings,
    /// Pixel to pick an object at, in viewport pixels.
    pub pick: Option<[u32; 2]>,
    /// Clear to transparent instead of the viewport color (thumbnails).
    pub transparent: bool,
    /// Section plane (normal, offset): geometry on the normal's side is cut away.
    pub section: Option<[f32; 4]>,
    /// Length of the normal lines in world units (when the overlay is on).
    pub normal_length: f32,
    /// World units per millimeter when the print finish applies (Manufacturing workspace),
    /// 0 otherwise.
    pub print_scale: f32,
    /// Shadows and the shadow floor (Rendered mode) need to know where the scene is.
    pub scene: Option<SceneFrame>,
}

struct GpuMesh {
    positions: wgpu::Buffer,
    normals: wgpu::Buffer,
    uvs: Option<wgpu::Buffer>,
    tangents: Option<wgpu::Buffer>,
    colors: Option<wgpu::Buffer>,
    joints: Option<wgpu::Buffer>,
    weights: Option<wgpu::Buffer>,
    indices: wgpu::Buffer,
    index_count: u32,
    vertex_count: u32,
    edges: Option<(wgpu::Buffer, u32)>,
    /// Mesh analysis markers, filled in when the background analysis finishes.
    markers: Option<(wgpu::Buffer, u32)>,
    material: usize,
    center: Vec3,
}

struct ObjectInfo {
    model: Mat4,
    material_color: [f32; 4],
    random_color: [f32; 3],
    flags: u32,
    joint_base: u32,
}

struct GpuImage {
    linear: wgpu::TextureView,
    srgb: wgpu::TextureView,
}

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve_srgb: wgpu::TextureView,
    resolve_tex: wgpu::Texture,
    id_tex: wgpu::Texture,
    id: wgpu::TextureView,
    id_depth: wgpu::TextureView,
    pick_depth_tex: wgpu::Texture,
    pick_depth: wgpu::TextureView,
    outline_bg: Option<wgpu::BindGroup>,
}

/// Surface pipelines, built once without and once with backface culling.
struct SurfacePipelines {
    opaque: wgpu::RenderPipeline,
    depth_only: wgpu::RenderPipeline,
    xray: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
}

struct Pipelines {
    surfaces: [SurfacePipelines; 2],
    wire: wgpu::RenderPipeline,
    wire_xray: wgpu::RenderPipeline,
    marker: wgpu::RenderPipeline,
    marker_hidden: wgpu::RenderPipeline,
    normals: wgpu::RenderPipeline,
    grid: wgpu::RenderPipeline,
    background: wgpu::RenderPipeline,
    floor: wgpu::RenderPipeline,
    id: wgpu::RenderPipeline,
    outline: wgpu::RenderPipeline,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    surface_tex: wgpu::Texture,
    surface_maps: SurfaceMaps,
    shadows: Shadows,
    /// Direction of the environment's brightest spot (environment space, before rotation).
    key_light: std::cell::Cell<Vec3>,
    object_bgl: wgpu::BindGroupLayout,
    object_buf: wgpu::Buffer,
    object_bg: wgpu::BindGroup,
    object_capacity: usize,
    object_stride: u64,
    joints_buf: wgpu::Buffer,
    joints_capacity: usize,
    material_bgl: wgpu::BindGroupLayout,
    material_sampler: wgpu::Sampler,
    materials: Vec<wgpu::BindGroup>,
    material_blend: Vec<bool>,
    white: GpuImage,
    flat_normal: GpuImage,
    images: Vec<GpuImage>,
    matcap_tex: wgpu::Texture,
    matcap_loaded: usize,
    ibl: ibl::Ibl,
    pipes: Pipelines,
    outline_bgl: wgpu::BindGroupLayout,
    selection_buf: wgpu::Buffer,
    selection_capacity: usize,
    pick_buf: wgpu::Buffer,
    targets: Option<Targets>,
    texture_id: Option<egui::TextureId>,
    meshes: Vec<GpuMesh>,
    objects: Vec<ObjectInfo>,
    zero_buf: Option<wgpu::Buffer>,
    visible: Vec<bool>,
    selection: Vec<u32>,
    pass_overrides: Vec<u32>,
    /// The user's move per object (Move tool), applied on top of the file's placement.
    user_transforms: Vec<Mat4>,
    objects_key: Option<(ColorMode, [u8; 3], Option<[u8; 3]>)>,
    objects_dirty: bool,
    /// Something the image depends on changed outside `render` (scene, pose, environment,
    /// selection...). Together with `last_frame` it lets an unchanged viewport skip the GPU.
    dirty: std::cell::Cell<bool>,
    /// Inputs of the last viewport frame still held by the egui texture.
    last_frame: Option<FrameKey>,
    /// Light matrices the shadow maps were last drawn with; geometry changes clear it.
    shadow_key: Option<[Mat4; 2]>,
    /// Result of the last pick request: `Some(None)` means "clicked empty space".
    pub picked: Option<Option<usize>>,
    /// World position under the last pick pixel, when it hit a surface.
    pub picked_point: Option<Option<Vec3>>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let tex_entry = |binding, filterable: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding, dynamic: bool, min: Option<NonZeroU64>| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: min,
            },
            count: None,
        };

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                uniform_entry(0, false, None),
                tex_entry(1, true),
                sampler_entry(2),
                tex_entry(3, true),
                tex_entry(4, true),
                tex_entry(5, true),
                tex_entry(6, true),
                sampler_entry(7),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                sampler_entry(9),
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let object_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[
                uniform_entry(0, true, NonZeroU64::new(OBJECT_SIZE)),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                uniform_entry(0, false, None),
                tex_entry(1, true),
                tex_entry(2, true),
                tex_entry(3, true),
                tex_entry(4, true),
                tex_entry(5, true),
                tex_entry(6, true),
                sampler_entry(7),
            ],
        });
        let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline"),
            entries: &[
                uniform_entry(0, false, None),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<GlobalsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let matcap_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("matcap"),
            size: wgpu::Extent3d {
                width: matcap::SIZE as u32 * 2,
                height: matcap::SIZE as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_matcap(queue, &matcap_tex, 0);
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        // Allocated now (GPU memory only), filled when a material first needs it.
        let surface_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface maps"),
            size: wgpu::Extent3d { width: SURFACE_SIZE, height: SURFACE_SIZE, depth_or_array_layers: SURFACE_MAPS.len() as u32 },
            mip_level_count: SURFACE_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let surface_view = surface_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 2 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let shadow_view = shadow_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let t = std::time::Instant::now();
        let shadows = create_shadows(device, &shadow_tex, &object_bgl);
        log::info!("shadow pipeline created in {:?}", t.elapsed());

        let ibl = ibl::Ibl::new(device, queue);
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&matcap_tex.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&clamp_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&ibl.src_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&ibl.specular_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&ibl.irradiance_view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&ibl.brdf_view) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&ibl.sampler) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&surface_view) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(&material_sampler) },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
            ],
        });

        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let object_stride = OBJECT_SIZE.div_ceil(align) * align;
        let joints_buf = create_joints_buffer(device, 1);
        let (object_buf, object_bg) = create_object_buffer(device, &object_bgl, 1, object_stride, &joints_buf);

        let t = std::time::Instant::now();
        let pipes = create_pipelines(device, &globals_bgl, &object_bgl, &material_bgl, &outline_bgl);
        log::info!("pipelines created in {:?}", t.elapsed());
        let white = upload_image(device, queue, 1, 1, &[vec![255, 255, 255, 255]]);
        let flat_normal = upload_image(device, queue, 1, 1, &[vec![128, 128, 255, 255]]);
        let selection_buf = create_selection_buffer(device, 1);
        let pick_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick"),
            // Object id at 0, depth at 256 (copies need 256-byte aligned rows).
            size: 512,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            device: device.clone(),
            queue: queue.clone(),
            globals_buf,
            globals_bg,
            surface_tex,
            surface_maps: SurfaceMaps::Unused,
            shadows,
            key_light: std::cell::Cell::new(DEFAULT_KEY_LIGHT),
            object_bgl,
            object_buf,
            object_bg,
            object_capacity: 1,
            object_stride,
            joints_buf,
            joints_capacity: 1,
            material_bgl,
            material_sampler,
            materials: Vec::new(),
            material_blend: Vec::new(),
            white,
            flat_normal,
            images: Vec::new(),
            matcap_tex,
            matcap_loaded: 0,
            ibl,
            pipes,
            outline_bgl,
            selection_buf,
            selection_capacity: 1,
            pick_buf,
            targets: None,
            texture_id: None,
            picked_point: None,
            meshes: Vec::new(),
            objects: Vec::new(),
            zero_buf: None,
            visible: Vec::new(),
            selection: Vec::new(),
            pass_overrides: Vec::new(),
            user_transforms: Vec::new(),
            objects_key: None,
            objects_dirty: true,
            dirty: std::cell::Cell::new(true),
            last_frame: None,
            shadow_key: None,
            picked: None,
        }
    }

    pub fn set_environment(&self, env: &EnvImage) {
        self.dirty.set(true);
        self.ibl.set_environment(&self.device, &self.queue, env);
        self.key_light.set(brightest_direction(env));
    }

    pub fn key_light(&self) -> Vec3 {
        self.key_light.get()
    }

    /// Whether every mesh buffer fits this GPU's largest buffer: wgpu treats an oversized buffer
    /// as a fatal error, so a model too big for the GPU must be refused before uploading.
    pub fn check_fits(&self, scene: &Scene) -> Result<(), String> {
        let limit = self.device.limits().max_buffer_size;
        // 16 bytes is the widest per-vertex attribute (tangents, colors, the zero buffer).
        let largest = scene
            .meshes
            .iter()
            .map(|m| (m.positions.len() as u64 * 16).max(m.indices.len() as u64 * 4).max(m.edges.len() as u64 * 4))
            .max()
            .unwrap_or(0);
        if largest <= limit {
            return Ok(());
        }
        let mb = |b: u64| (b / (1024 * 1024)).to_string();
        Err(crate::i18n::trf(
            "the model needs {needed} MB GPU buffers, this GPU allows {limit} MB",
            &[("needed", &mb(largest)), ("limit", &mb(limit))],
        ))
    }

    pub fn upload_scene(&mut self, scene: &Scene) {
        self.dirty.set(true);
        self.shadow_key = None;
        self.meshes.clear();
        self.objects.clear();

        let max_dim = self.device.limits().max_texture_dimension_2d;
        self.images = scene
            .images
            .iter()
            .map(|img| {
                // Skip mips larger than the GPU supports (8K+ textures on older hardware).
                let mut skip = 0;
                while (img.width >> skip) > max_dim || (img.height >> skip) > max_dim {
                    skip += 1;
                }
                let w = (img.width >> skip).max(1);
                let h = (img.height >> skip).max(1);
                upload_image(&self.device, &self.queue, w, h, &img.mips[skip.min(img.mips.len() - 1)..])
            })
            .collect();

        self.materials.clear();
        self.material_blend.clear();
        for m in &scene.materials {
            let mut flags = 0u32;
            let base = self.texture_view(m.base_color_tex, 1, true, &mut flags);
            let normal = self.texture_view(m.normal_tex, 2, false, &mut flags);
            let metallic = self.texture_view(m.metallic_tex.map(|t| t.image), 4, false, &mut flags);
            let rough = self.texture_view(m.roughness_tex.map(|t| t.image), 8, false, &mut flags);
            let occ = self.texture_view(m.occlusion_tex.map(|t| t.image), 16, false, &mut flags);
            let emissive = self.texture_view(m.emissive_tex, 32, true, &mut flags);
            let (alpha_cutoff, alpha_mode) = match m.alpha_mode {
                AlphaMode::Opaque => (0.0, 0.0),
                AlphaMode::Mask(c) => (c, 1.0),
                AlphaMode::Blend => (0.0, 2.0),
            };
            let uniform = MaterialUniform {
                base_color: m.base_color,
                emissive: [m.emissive[0], m.emissive[1], m.emissive[2], 0.0],
                pbr: [m.metallic, m.roughness, m.normal_scale, m.occlusion_strength],
                channels: [
                    m.metallic_tex.map_or(0, |t| t.channel as u32),
                    m.roughness_tex.map_or(0, |t| t.channel as u32),
                    m.occlusion_tex.map_or(0, |t| t.channel as u32),
                    flags,
                ],
                alpha: [alpha_cutoff, alpha_mode, m.clearcoat, m.clearcoat_roughness],
                extra: [m.transmission, m.ior, 0.0, 0.0],
            };
            let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("material"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let tv = wgpu::BindingResource::TextureView;
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("material"),
                layout: &self.material_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: tv(base) },
                    wgpu::BindGroupEntry { binding: 2, resource: tv(normal) },
                    wgpu::BindGroupEntry { binding: 3, resource: tv(metallic) },
                    wgpu::BindGroupEntry { binding: 4, resource: tv(rough) },
                    wgpu::BindGroupEntry { binding: 5, resource: tv(occ) },
                    wgpu::BindGroupEntry { binding: 6, resource: tv(emissive) },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(&self.material_sampler),
                    },
                ],
            });
            self.materials.push(bg);
            self.material_blend.push(m.alpha_mode == AlphaMode::Blend);
        }

        let init = |label: &str, contents: &[u8], usage| {
            // Zero-sized buffers aren't bindable: pad empty meshes.
            let padded;
            let contents = if contents.is_empty() {
                padded = [0u8; 16];
                &padded[..]
            } else {
                contents
            };
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        // Every skin's joints live in one storage buffer; start with the rest pose.
        let anim = &scene.animation;
        let mut joint_bases = Vec::with_capacity(anim.skins.len());
        let mut rest_joints = Vec::new();
        let rest = anim.rest_world();
        for skin in &anim.skins {
            joint_bases.push(rest_joints.len() as u32);
            for (j, ib) in skin.joints.iter().zip(&skin.inverse_bind) {
                rest_joints.push(rest[*j] * *ib);
            }
        }

        let mut zero_needed = 0usize;
        for mesh in &scene.meshes {
            let vertex = wgpu::BufferUsages::VERTEX;
            let index = wgpu::BufferUsages::INDEX;
            let uvs = mesh.uvs.as_ref().map(|v| init("uvs", bytemuck::cast_slice(v), vertex));
            let tangents = mesh.tangents.as_ref().map(|v| init("tangents", bytemuck::cast_slice(v), vertex));
            let colors = mesh.colors.as_ref().map(|v| init("colors", bytemuck::cast_slice(v), vertex));
            let skinned = mesh.rig.skin.is_some();
            let joints = mesh.rig.joints.as_ref().filter(|_| skinned).map(|v| init("joints", bytemuck::cast_slice(v), vertex));
            let weights = mesh.rig.weights.as_ref().filter(|_| skinned).map(|v| init("weights", bytemuck::cast_slice(v), vertex));
            if uvs.is_none() || tangents.is_none() || colors.is_none() || joints.is_none() || weights.is_none() {
                zero_needed = zero_needed.max(mesh.positions.len());
            }
            let skinned = joints.is_some() && weights.is_some();
            let flags = if uvs.is_some() { HAS_UV } else { 0 }
                | if tangents.is_some() { HAS_TANGENT } else { 0 }
                | if colors.is_some() { HAS_COLOR } else { 0 }
                | if skinned { FLAG_SKINNED } else { 0 };
            let edges = (!mesh.edges.is_empty())
                .then(|| (init("edges", bytemuck::cast_slice(&mesh.edges), index), mesh.edges.len() as u32));
            let material = mesh.material.min(scene.materials.len() - 1);
            self.meshes.push(GpuMesh {
                // Morph targets rewrite positions and normals on the CPU.
                positions: init("positions", bytemuck::cast_slice(&mesh.positions), vertex | wgpu::BufferUsages::COPY_DST),
                normals: init("normals", bytemuck::cast_slice(&mesh.normals), vertex | wgpu::BufferUsages::COPY_DST),
                uvs,
                tangents,
                colors,
                joints,
                weights,
                indices: init("indices", bytemuck::cast_slice(&mesh.indices), index),
                index_count: mesh.indices.len() as u32,
                vertex_count: mesh.positions.len() as u32,
                edges,
                markers: None,
                material,
                center: mesh.bounds.transformed(&mesh.transform).center(),
            });
            self.objects.push(ObjectInfo {
                model: mesh.transform,
                material_color: scene.materials[material].base_color,
                random_color: random_color(&mesh.name),
                flags,
                joint_base: mesh.rig.skin.and_then(|s| joint_bases.get(s).copied()).unwrap_or(0),
            });
        }
        // One zero-filled buffer stands in for every missing attribute (16 bytes covers the
        // largest attribute stride), instead of allocating defaults per mesh.
        self.zero_buf = (zero_needed > 0).then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zero attributes"),
                size: (zero_needed as u64 * 16).max(16),
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            })
        });

        self.set_joints(&rest_joints);

        let n = self.objects.len();
        self.visible = vec![true; n];
        self.selection = vec![0; n];
        self.pass_overrides = vec![0; n];
        self.user_transforms = vec![Mat4::IDENTITY; n];
        if n > self.object_capacity {
            let capacity = n.next_power_of_two();
            let (buf, bg) = create_object_buffer(&self.device, &self.object_bgl, capacity, self.object_stride, &self.joints_buf);
            self.object_buf = buf;
            self.object_bg = bg;
            self.object_capacity = capacity;
        }
        if n > self.selection_capacity {
            self.selection_capacity = n.next_power_of_two();
            self.selection_buf = create_selection_buffer(&self.device, self.selection_capacity);
            if let Some(t) = &mut self.targets {
                t.outline_bg = None;
            }
        }
        self.objects_dirty = true;
    }

    /// The view for a material slot, or a neutral default; sets `bit` in `flags` when textured.
    fn texture_view(&self, image: Option<usize>, bit: u32, srgb: bool, flags: &mut u32) -> &wgpu::TextureView {
        match image.and_then(|i| self.images.get(i)) {
            Some(img) => {
                *flags |= bit;
                if srgb { &img.srgb } else { &img.linear }
            }
            None if bit == 2 => &self.flat_normal.linear,
            None if srgb => &self.white.srgb,
            None => &self.white.linear,
        }
    }

    /// Joint matrices for every skin (see `AnimPlayer::pose`).
    pub fn set_joints(&mut self, joints: &[Mat4]) {
        self.dirty.set(true);
        self.shadow_key = None;
        if joints.len() > self.joints_capacity {
            self.joints_capacity = joints.len().next_power_of_two();
            self.joints_buf = create_joints_buffer(&self.device, self.joints_capacity);
            let (buf, bg) = create_object_buffer(
                &self.device,
                &self.object_bgl,
                self.object_capacity,
                self.object_stride,
                &self.joints_buf,
            );
            self.object_buf = buf;
            self.object_bg = bg;
            self.objects_dirty = true;
        }
        if !joints.is_empty() {
            let data: Vec<[[f32; 4]; 4]> = joints.iter().map(|m| m.to_cols_array_2d()).collect();
            self.queue.write_buffer(&self.joints_buf, 0, bytemuck::cast_slice(&data));
        }
    }

    /// Animated object-to-world matrices (one per mesh).
    pub fn set_object_transforms(&mut self, transforms: &[Mat4]) {
        for (o, t) in self.objects.iter_mut().zip(transforms) {
            if o.model != *t {
                o.model = *t;
                self.objects_dirty = true;
                self.dirty.set(true);
            }
        }
    }

    /// New vertex positions/normals for a morphing mesh (same vertex count).
    pub fn update_mesh_geometry(&mut self, index: usize, positions: &[[f32; 3]], normals: &[[f32; 3]]) {
        self.dirty.set(true);
        self.shadow_key = None;
        if let Some(m) = self.meshes.get(index) {
            self.queue.write_buffer(&m.positions, 0, bytemuck::cast_slice(positions));
            self.queue.write_buffer(&m.normals, 0, bytemuck::cast_slice(normals));
        }
    }

    /// Uploads the mesh analysis markers; `per_mesh[i]` belongs to mesh `i`.
    pub fn set_markers(&mut self, per_mesh: &[Vec<MarkerInstance>]) {
        self.dirty.set(true);
        for (mesh, instances) in self.meshes.iter_mut().zip(per_mesh) {
            mesh.markers = (!instances.is_empty()).then(|| {
                let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("markers"),
                    contents: bytemuck::cast_slice(instances),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                (buffer, instances.len() as u32)
            });
        }
    }

    /// Normal lines: every vertex is an instance of a two-point line.
    fn draw_normals(&self, pass: &mut wgpu::RenderPass<'_>) {
        for (i, m) in self.meshes.iter().enumerate() {
            if m.vertex_count == 0 || !self.visible.get(i).copied().unwrap_or(true) {
                continue;
            }
            pass.set_bind_group(1, &self.object_bg, &[(i as u64 * self.object_stride) as u32]);
            pass.set_vertex_buffer(0, m.positions.slice(..));
            pass.set_vertex_buffer(1, m.normals.slice(..));
            for (slot, buf) in [(2, &m.joints), (3, &m.weights)] {
                if let Some(b) = buf.as_ref().or(self.zero_buf.as_ref()) {
                    pass.set_vertex_buffer(slot, b.slice(..));
                }
            }
            pass.draw(0..2, 0..m.vertex_count);
        }
    }

    fn draw_markers(&self, pass: &mut wgpu::RenderPass<'_>) {
        for (i, m) in self.meshes.iter().enumerate() {
            let Some((buffer, count)) = &m.markers else { continue };
            if !self.visible.get(i).copied().unwrap_or(true) {
                continue;
            }
            pass.set_bind_group(1, &self.object_bg, &[(i as u64 * self.object_stride) as u32]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..*count);
        }
    }

    pub fn set_visibility(&mut self, visible: &[bool]) {
        if visible != self.visible.as_slice() {
            self.visible.clear();
            self.visible.extend_from_slice(visible);
            self.dirty.set(true);
            self.shadow_key = None;
        }
    }

    /// Per object: 0 = unselected, 1 = selected, 2 = active.
    pub fn set_selection(&mut self, states: &[u8]) {
        let states: Vec<u32> = states.iter().map(|&s| s as u32).collect();
        if states != self.selection {
            self.selection = states;
            self.objects_dirty = true;
            self.dirty.set(true);
        }
    }

    /// The user's move per object, applied on top of the file's (or the animation's) placement.
    pub fn set_user_transforms(&mut self, transforms: &[Mat4]) {
        if transforms != self.user_transforms.as_slice() {
            self.user_transforms = transforms.to_vec();
            self.objects_dirty = true;
            self.dirty.set(true);
        }
    }

    /// Per object: 0 = follow the global color mode, otherwise texture pass index + 1.
    pub fn set_pass_overrides(&mut self, passes: &[u32]) {
        if passes != self.pass_overrides.as_slice() {
            self.pass_overrides = passes.to_vec();
            self.objects_dirty = true;
            self.dirty.set(true);
        }
    }

    fn write_objects(&mut self, settings: &Settings, neutral: bool) {
        let key = (settings.color, settings.single_color, neutral.then_some(settings.plastic_color));
        if (self.objects_key == Some(key) && !self.objects_dirty) || self.objects.is_empty() {
            return;
        }
        self.objects_key = Some(key);
        self.objects_dirty = false;
        self.shadow_key = None;
        let single = srgb_to_linear(settings.single_color);
        let plastic = srgb_to_linear(settings.plastic_color);
        let mut bytes = vec![0u8; self.objects.len() * self.object_stride as usize];
        for (i, o) in self.objects.iter().enumerate() {
            let color = match settings.color {
                ColorMode::Single => [single[0], single[1], single[2], 1.0],
                ColorMode::Random => [o.random_color[0], o.random_color[1], o.random_color[2], 1.0],
                _ if neutral => [plastic[0], plastic[1], plastic[2], 1.0],
                _ => o.material_color,
            };
            let sel = match self.selection.get(i) {
                Some(2) => FLAG_ACTIVE | FLAG_SELECTED,
                Some(1) => FLAG_SELECTED,
                _ => 0,
            };
            let model = self.user_transforms.get(i).copied().unwrap_or(Mat4::IDENTITY) * o.model;
            let u = ObjectUniform {
                model: model.to_cols_array_2d(),
                normal_mat: model.inverse().transpose().to_cols_array_2d(),
                color,
                info: [i as u32 + 1, o.flags | sel, o.joint_base, self.pass_overrides.get(i).copied().unwrap_or(0)],
            };
            let start = i * self.object_stride as usize;
            bytes[start..start + OBJECT_SIZE as usize].copy_from_slice(bytemuck::bytes_of(&u));
        }
        self.queue.write_buffer(&self.object_buf, 0, &bytes);
        let mut sel = self.selection.clone();
        if sel.is_empty() {
            sel.push(0);
        }
        self.queue.write_buffer(&self.selection_buf, 0, bytemuck::cast_slice(&sel));
    }

    fn ensure_targets(&mut self, size: [u32; 2], egui_renderer: Option<&mut egui_wgpu::Renderer>) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let make = |label, format, samples, usage, view_formats: &[wgpu::TextureFormat]| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats,
            })
        };
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let msaa = make("msaa", COLOR_FORMAT, SAMPLES, attach, &[]);
        let depth = make("depth", DEPTH_FORMAT, SAMPLES, attach, &[]);
        let resolve = make(
            "resolve",
            COLOR_FORMAT,
            1,
            attach | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            &[EGUI_VIEW_FORMAT],
        );
        let id_tex = make(
            "ids",
            ID_FORMAT,
            1,
            attach | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            &[],
        );
        let id_depth = make("id depth", DEPTH_FORMAT, 1, attach, &[]);
        // Depth textures can't be copied one pixel at a time: the id pass writes depth here too.
        let pick_depth = make("pick depth", PICK_DEPTH_FORMAT, 1, attach | wgpu::TextureUsages::COPY_SRC, &[]);
        let resolve_egui = resolve.create_view(&wgpu::TextureViewDescriptor {
            format: Some(EGUI_VIEW_FORMAT),
            ..Default::default()
        });
        if let Some(egui_renderer) = egui_renderer {
            match self.texture_id {
                Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                    &self.device,
                    &resolve_egui,
                    wgpu::FilterMode::Nearest,
                    id,
                ),
                None => {
                    self.texture_id = Some(egui_renderer.register_native_texture(
                        &self.device,
                        &resolve_egui,
                        wgpu::FilterMode::Nearest,
                    ))
                }
            }
        }
        self.targets = Some(Targets {
            size,
            msaa: msaa.create_view(&Default::default()),
            depth: depth.create_view(&Default::default()),
            resolve_srgb: resolve.create_view(&Default::default()),
            resolve_tex: resolve,
            id: id_tex.create_view(&Default::default()),
            id_tex,
            id_depth: id_depth.create_view(&Default::default()),
            pick_depth: pick_depth.create_view(&Default::default()),
            pick_depth_tex: pick_depth,
            outline_bg: None,
        });
    }

    fn draw_surfaces(&self, pass: &mut wgpu::RenderPass<'_>, indices: impl Iterator<Item = usize>) {
        for i in indices {
            let m = &self.meshes[i];
            if m.index_count == 0 || !self.visible.get(i).copied().unwrap_or(true) {
                continue;
            }
            let zero = self.zero_buf.as_ref();
            pass.set_bind_group(1, &self.object_bg, &[(i as u64 * self.object_stride) as u32]);
            pass.set_bind_group(2, &self.materials[m.material], &[]);
            pass.set_vertex_buffer(0, m.positions.slice(..));
            pass.set_vertex_buffer(1, m.normals.slice(..));
            for (slot, buf) in [(2, &m.uvs), (3, &m.tangents), (4, &m.colors), (5, &m.joints), (6, &m.weights)] {
                if let Some(b) = buf.as_ref().or(zero) {
                    pass.set_vertex_buffer(slot, b.slice(..));
                }
            }
            pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.index_count, 0, 0..1);
        }
    }

    fn draw_positions_only(&self, pass: &mut wgpu::RenderPass<'_>, edges: bool) {
        for (i, m) in self.meshes.iter().enumerate() {
            if !self.visible.get(i).copied().unwrap_or(true) {
                continue;
            }
            let (buffer, count) = if edges {
                match &m.edges {
                    Some((b, c)) => (b, *c),
                    None => continue,
                }
            } else {
                (&m.indices, m.index_count)
            };
            if count == 0 {
                continue;
            }
            pass.set_bind_group(1, &self.object_bg, &[(i as u64 * self.object_stride) as u32]);
            pass.set_vertex_buffer(0, m.positions.slice(..));
            for (slot, buf) in [(5, &m.joints), (6, &m.weights)] {
                if let Some(b) = buf.as_ref().or(self.zero_buf.as_ref()) {
                    pass.set_vertex_buffer(slot, b.slice(..));
                }
            }
            pass.set_index_buffer(buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..count, 0, 0..1);
        }
    }

    /// Renders the viewport and returns the egui texture holding the result (when an egui
    /// renderer is given; headless callers read the pixels back instead).
    pub fn render(
        &mut self,
        egui_renderer: Option<&mut egui_wgpu::Renderer>,
        size: [u32; 2],
        input: &FrameInput,
    ) -> Option<egui::TextureId> {
        let size = [size[0].max(1), size[1].max(1)];
        // An unchanged viewport (the UI repainting for a hover, a tooltip...) keeps last frame's
        // image instead of drawing the whole scene again. Headless renders reuse the targets at
        // another size, so they always draw and drop the cache.
        let to_egui = egui_renderer.is_some();
        let key = FrameKey::new(size, input);
        if to_egui
            && input.pick.is_none()
            && !self.dirty.get()
            && self.texture_id.is_some()
            && !matches!(self.surface_maps, SurfaceMaps::Decoding(_))
            && self.last_frame.as_ref() == Some(&key)
        {
            return self.texture_id;
        }
        self.dirty.set(false);
        self.last_frame = to_egui.then_some(key);
        self.ensure_targets(size, egui_renderer);
        let s = input.settings;

        if self.matcap_loaded != s.matcap {
            write_matcap(&self.queue, &self.matcap_tex, s.matcap);
            self.matcap_loaded = s.matcap;
        }
        // Manufacturing (the print scale applies) shows plain plastic unless the file's own
        // materials are asked for.
        let neutral = input.print_scale > 0.0 && !s.file_materials;
        self.write_objects(s, neutral);
        let wants_surface = input.print_scale > 0.0
            && (s.grain > 0.0 || s.scratches > 0.0 || (neutral && s.material == PartMaterial::Metal));
        let surface_ready = self.surface_maps_ready(wants_surface);

        let xray = s.xray();
        let wire_mode = s.shading == ShadingMode::Wireframe;
        let rendered = s.shading == ShadingMode::Rendered;
        let object_outline = s.show_outline && s.shading == ShadingMode::Solid;
        let any_selected = self.selection.iter().any(|&v| v > 0);
        let need_ids = !self.meshes.is_empty() && (object_outline || any_selected || input.pick.is_some());

        let view_proj = input.proj * input.view;
        // Shadows: the key light from the environment's brightest spot (turned with the
        // environment), plus a straight-down view for contact shading on the floor.
        let shadow_frame = input.scene.filter(|_| rendered && !xray && (s.shadows || s.floor_shadow));
        let mut uniform_lights = [LightDataUniform { dir: [0.0; 4], color: [0.0; 4] }; 6];
        let key_dir = if let Some(l) = s.lights.first() {
            if l.follow_env {
                let d = self.key_light.get();
                let (sin, cos) = s.env_rotation.to_radians().sin_cos();
                let d = Vec3::new(cos * d.x - sin * d.y, sin * d.x + cos * d.y, d.z);
                let flat = Vec3::new(d.x, d.y, 0.0).normalize_or(Vec3::X);
                let elevation = d.z.clamp(-1.0, 1.0).asin().max(25f32.to_radians());
                (flat * elevation.cos() + Vec3::Z * elevation.sin()).normalize()
            } else {
                let yaw_rad = l.yaw.to_radians();
                let pitch_rad = l.pitch.to_radians().clamp(5f32.to_radians(), 89f32.to_radians());
                let (sy, cy) = yaw_rad.sin_cos();
                let (sp, cp) = pitch_rad.sin_cos();
                Vec3::new(cp * cy, cp * sy, sp).normalize()
            }
        } else {
            Vec3::new(0.5, 0.5, 0.7).normalize()
        };

        if rendered {
            if let Some(l) = s.lights.first() {
                if l.enabled && l.strength > 0.0 {
                    let c = srgb_to_linear(l.color);
                    uniform_lights[0] = LightDataUniform {
                        dir: [key_dir.x, key_dir.y, key_dir.z, l.strength * s.env_strength * 2.0],
                        color: [c[0], c[1], c[2], 1.0],
                    };
                }
            }

            for (idx, l) in s.lights.iter().skip(1).take(5).enumerate() {
                if l.enabled && l.strength > 0.0 {
                    let yaw_rad = l.yaw.to_radians();
                    let pitch_rad = l.pitch.to_radians().clamp(-89f32.to_radians(), 89f32.to_radians());
                    let (sy, cy) = yaw_rad.sin_cos();
                    let (sp, cp) = pitch_rad.sin_cos();
                    let dir = Vec3::new(cp * cy, cp * sy, sp).normalize();
                    let c = srgb_to_linear(l.color);
                    uniform_lights[idx + 1] = LightDataUniform {
                        dir: [dir.x, dir.y, dir.z, l.strength * s.env_strength * 2.0],
                        color: [c[0], c[1], c[2], 1.0],
                    };
                }
            }
        };
        let (light0, light1) = match shadow_frame {
            Some(f) => {
                let c = Vec3::new(f.center.x, f.center.y, f.center.z);
                let r = f.radius.max(1e-6);
                // Shadows fall up to height / tan(elevation) away from the model.
                let reach = r * (1.0 + 1.0 / key_dir.z.asin().tan()).min(4.0);
                let view = |dir: Vec3, half: f32| {
                    let up = if dir.z.abs() > 0.99 { Vec3::Y } else { Vec3::Z };
                    glam::camera::rh::proj::directx::orthographic(-half, half, -half, half, 0.0, 6.0 * r)
                        * glam::camera::rh::view::look_at_mat4(c + dir * 3.0 * r, c, up)
                };
                (view(key_dir, reach), view(Vec3::Z, r * 1.5))
            }
            None => (Mat4::IDENTITY, Mat4::IDENTITY),
        };
        let globals = GlobalsUniform {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            view: input.view.to_cols_array_2d(),
            eye: input.eye.extend(if input.ortho { 1.0 } else { 0.0 }).to_array(),
            cam_back: input.cam_back.extend(0.0).to_array(),
            shading: [
                match s.shading {
                    ShadingMode::Wireframe => 0,
                    ShadingMode::Solid => 1,
                    ShadingMode::Rendered => 2,
                },
                match s.lighting {
                    Lighting::Studio => 0,
                    Lighting::MatCap => 1,
                    Lighting::Flat => 2,
                },
                s.backface_culling as u32,
                match s.color {
                    ColorMode::Material => 0,
                    ColorMode::Single => 1,
                    ColorMode::Random => 2,
                    ColorMode::Texture => 3,
                    ColorMode::Attribute => 4,
                },
            ],
            extra: [
                TexturePass::ALL.iter().position(|p| *p == s.texture_pass).unwrap_or(0) as u32,
                (s.view_transform == ViewTransform::AgX) as u32,
                if rendered && s.transparent_background { 3 } else if rendered && s.env_background { 1 } else if rendered && s.studio_backdrop { 2 } else { 0 },
                object_outline as u32,
            ],
            params: [
                if xray { s.xray_alpha.clamp(0.05, 1.0) } else { 1.0 },
                s.exposure,
                if wire_mode { if xray { 0.75 } else { 1.0 } } else { s.wire_opacity.clamp(0.05, 1.0) },
                s.env_rotation.to_radians(),
            ],
            env: [s.env_strength, s.env_blur, (ibl::SPEC_MIPS - 1) as f32, 0.0],
            grid: [
                input.grid_cell,
                input.grid_fade,
                input.grid_axis as f32,
                if s.show_axes { 1.0 } else { 0.0 },
            ],
            wire_color: {
                let wire_random = (wire_mode && s.color == ColorMode::Random)
                    || (!wire_mode && s.show_wire_overlay && s.wire_color_mode == crate::settings::WireColorMode::Random);
                let wire_rgb = if !wire_mode && s.wire_color_mode == crate::settings::WireColorMode::Custom {
                    let c = srgb_to_linear(s.wire_color);
                    [c[0], c[1], c[2]]
                } else if wire_mode {
                    [0.62, 0.62, 0.62]
                } else {
                    [0.05, 0.05, 0.05]
                };
                [wire_rgb[0], wire_rgb[1], wire_rgb[2], if wire_random { 1.0 } else { 0.0 }]
            },
            selected_color: SELECTED,
            active_color: ACTIVE,
            viewport: [size[0] as f32, size[1] as f32, 1.0 / size[0] as f32, 1.0 / size[1] as f32],
            markers: [s.show_non_manifold as u32, s.show_open_edges as u32, s.show_overlapping as u32, 0],
            section: input.section.unwrap_or([0.0; 4]),
            normals: [if s.show_normals { input.normal_length } else { 0.0 }, s.show_face_orientation as u32 as f32, 0.0, 0.0],
            display: [(s.up_axis == crate::axes::UpAxis::Y) as u32 as f32, neutral as u32 as f32, 0.0, 0.0],
            // The material only shows in Rendered; layer lines need a height.
            finish: if input.print_scale > 0.0 {
                let id = if rendered && neutral { s.material.shader_id() } else { 0 };
                let layers = s.layer_lines && s.material.layer_height() > 0.0;
                let height = if layers { s.layer_height.max(0.01) * input.print_scale } else { 0.0 };
                let metal = MetalFinish::ALL.iter().position(|f| *f == s.metal_finish).unwrap_or(0);
                [id as f32, height, metal as f32, input.print_scale]
            } else {
                [0.0; 4]
            },
            finish_color: {
                let c = srgb_to_linear(s.plastic_color);
                [c[0], c[1], c[2], 1.0]
            },
            // Wear in Manufacturing, once its maps are on the GPU: grain and scratch strength,
            // then the size of one map tile in world units.
            surface: if input.print_scale > 0.0 && surface_ready {
                let size = s.grain_size.clamp(0.2, 5.0) * input.print_scale;
                [s.grain.clamp(0.0, 1.0), s.scratches.clamp(0.0, 1.0), 30.0 * size, 50.0 * size]
            } else {
                [0.0; 4]
            },
            light0: light0.to_cols_array_2d(),
            light1: light1.to_cols_array_2d(),
            lights: uniform_lights,
            shadow: match shadow_frame {
                Some(f) => [s.floor_shadow as u32 as f32, f.floor, 1.0 + s.shadow_softness.clamp(0.0, 1.0) * 5.0, f.radius * 0.004],
                None => [0.0; 4],
            },
            floor: match shadow_frame {
                Some(f) => [f.center.x, f.center.y, f.radius * 4.0, if s.shadows { 1.0 } else { 0.0 }],
                None => [0.0; 4],
            },
        };
        self.queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        // Shadow maps only change with the lights or the geometry, not with the camera.
        let draw_shadows = shadow_frame.is_some() && self.shadow_key != Some([light0, light1]);
        if draw_shadows {
            for (buf, m) in self.shadows.matrices.iter().zip([light0, light1]) {
                self.queue.write_buffer(buf, 0, bytemuck::bytes_of(&m.to_cols_array_2d()));
            }
            self.shadow_key = Some([light0, light1]);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewport") });
        let targets = self.targets.as_ref().expect("targets were just created");

        // 0. Object ids, for picking and outlines.
        if need_ids {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ids"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.id,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }), Some(wgpu::RenderPassColorAttachment {
                    view: &targets.pick_depth,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.id_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipes.id);
            pass.set_bind_group(0, &self.globals_bg, &[]);
            self.draw_positions_only(&mut pass, false);
        }

        if draw_shadows {
            for layer in 0..2 {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow map"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.shadows.layers[layer],
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.shadows.pipeline);
                pass.set_bind_group(0, &self.shadows.bind_groups[layer], &[]);
                self.draw_positions_only(&mut pass, false);
            }
        }

        {
            let bg = srgb_to_linear(BACKGROUND);
            let clear = if input.transparent {
                wgpu::Color::TRANSPARENT
            } else {
                wgpu::Color { r: bg[0] as f64, g: bg[1] as f64, b: bg[2] as f64, a: 1.0 }
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa,
                    depth_slice: None,
                    resolve_target: Some(&targets.resolve_srgb),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);

            // 1. World background or studio backdrop (Rendered).
            if !input.transparent && rendered && (s.env_background || s.studio_backdrop || s.transparent_background) {
                pass.set_pipeline(&self.pipes.background);
                pass.draw(0..3, 0..1);
            }

            // 2. Surfaces (or depth only, for hidden-line wireframe). Blended materials wait.
            let uses_blend = !wire_mode && !xray;
            let opaque = (0..self.meshes.len())
                .filter(|&i| !(uses_blend && self.material_blend[self.meshes[i].material]));
            // A section shows the inside through back faces, so it turns culling off.
            let cull = s.backface_culling && !wire_mode && input.section.is_none();
            let surf = &self.pipes.surfaces[cull as usize];
            match (wire_mode, xray) {
                (true, true) => {}
                (true, false) => {
                    pass.set_pipeline(&surf.depth_only);
                    self.draw_surfaces(&mut pass, opaque);
                }
                (false, true) => {
                    pass.set_pipeline(&surf.xray);
                    self.draw_surfaces(&mut pass, opaque);
                }
                (false, false) => {
                    pass.set_pipeline(&surf.opaque);
                    self.draw_surfaces(&mut pass, opaque);
                }
            }

            // 3. Floor grid, depth-tested against the surfaces, and the shadow floor. The studio
            // backdrop is for clean shots: no grid.
            if !input.transparent && s.show_grid && !(rendered && (s.studio_backdrop || s.transparent_background) && !s.env_background) {
                pass.set_pipeline(&self.pipes.grid);
                pass.draw(0..3, 0..1);
            }
            if shadow_frame.is_some() && s.floor_shadow {
                pass.set_pipeline(&self.pipes.floor);
                pass.draw(0..6, 0..1);
            }

            // 4. Transparent materials, back to front.
            if uses_blend {
                let mut blended: Vec<usize> = (0..self.meshes.len())
                    .filter(|&i| self.material_blend[self.meshes[i].material])
                    .collect();
                if !blended.is_empty() {
                    blended.sort_by(|&a, &b| {
                        let da = self.meshes[a].center.distance_squared(input.eye);
                        let db = self.meshes[b].center.distance_squared(input.eye);
                        db.total_cmp(&da)
                    });
                    // Triangles inside one mesh aren't sorted, so a double-sided or two-layer
                    // glass would blend its far layer over its near one in patches. Each mesh
                    // writes its depth first and is then colored only where it's the nearest.
                    for i in blended {
                        pass.set_pipeline(&surf.depth_only);
                        self.draw_surfaces(&mut pass, std::iter::once(i));
                        pass.set_pipeline(&surf.blend);
                        self.draw_surfaces(&mut pass, std::iter::once(i));
                    }
                }
            }

            // 5. Edges.
            if wire_mode || s.show_wire_overlay {
                pass.set_pipeline(if xray { &self.pipes.wire_xray } else { &self.pipes.wire });
                self.draw_positions_only(&mut pass, true);
            }

            // 6. Normal lines.
            if s.show_normals && input.normal_length > 0.0 {
                pass.set_pipeline(&self.pipes.normals);
                self.draw_normals(&mut pass);
            }

            // 7. Mesh analysis markers: faint where hidden, solid where visible.
            if s.show_non_manifold || s.show_open_edges || s.show_overlapping {
                pass.set_pipeline(&self.pipes.marker_hidden);
                self.draw_markers(&mut pass);
                if !xray {
                    pass.set_pipeline(&self.pipes.marker);
                    self.draw_markers(&mut pass);
                }
            }
        }

        // 8. Outlines over the resolved image.
        if need_ids && (object_outline || any_selected) {
            let targets = self.targets.as_mut().expect("targets exist");
            if targets.outline_bg.is_none() {
                targets.outline_bg = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("outline"),
                    layout: &self.outline_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: self.globals_buf.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&targets.id) },
                        wgpu::BindGroupEntry { binding: 2, resource: self.selection_buf.as_entire_binding() },
                    ],
                }));
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.resolve_srgb,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipes.outline);
            pass.set_bind_group(0, targets.outline_bg.as_ref(), &[]);
            pass.draw(0..3, 0..1);
        }

        let targets = self.targets.as_ref().expect("targets exist");
        let pick = input.pick.filter(|p| p[0] < size[0] && p[1] < size[1]);
        if let Some(p) = pick {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &targets.id_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: p[0], y: p[1], z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.pick_buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &targets.pick_depth_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: p[0], y: p[1], z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.pick_buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 256,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
        }
        self.queue.submit(Some(encoder.finish()));

        if let Some(p) = pick {
            let (object, depth) = self.read_pick();
            self.picked = Some(object);
            // Reverse-Z: 0 is the far plane, i.e. nothing under the cursor.
            self.picked_point = Some((object.is_some() && depth > 0.0).then(|| {
                let ndc = glam::Vec4::new(
                    (p[0] as f32 + 0.5) / size[0] as f32 * 2.0 - 1.0,
                    1.0 - (p[1] as f32 + 0.5) / size[1] as f32 * 2.0,
                    depth,
                    1.0,
                );
                let world = view_proj.inverse() * ndc;
                world.truncate() / world.w
            }));
        }
        self.texture_id
    }

    /// Reads the last rendered image back as tightly packed RGBA8 (sRGB).
    pub fn read_pixels(&self) -> Option<([u32; 2], Vec<u8>)> {
        let targets = self.targets.as_ref()?;
        let [w, h] = targets.size;
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.resolve_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let start = y * row as usize;
            out.extend_from_slice(&data[start..start + w as usize * 4]);
        }
        Some(([w, h], out))
    }

    /// Blocks briefly for the one-pixel readback. Only happens on click.
    /// Object under the pick pixel and its depth (0 when nothing was hit).
    fn read_pick(&self) -> (Option<usize>, f32) {
        let slice = self.pick_buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let (id, depth) = match slice.get_mapped_range() {
            Ok(data) => (
                u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
                f32::from_le_bytes([data[256], data[257], data[258], data[259]]),
            ),
            Err(_) => (0, 0.0),
        };
        self.pick_buf.unmap();
        ((id > 0).then(|| id as usize - 1), depth)
    }
}

impl Renderer {
    /// Whether the surface maps are on the GPU. The first time they're wanted, a worker thread
    /// decodes them; until it's done materials render without wear.
    fn surface_maps_ready(&mut self, wanted: bool) -> bool {
        match &self.surface_maps {
            SurfaceMaps::Ready => return true,
            SurfaceMaps::Unused if wanted => {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let layers = SURFACE_MAPS
                        .iter()
                        .map(|png| {
                            let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png)
                                .map(|i| i.to_rgba8().into_raw())
                                .unwrap_or_else(|_| vec![128; (SURFACE_SIZE * SURFACE_SIZE * 4) as usize]);
                            polyloupe_core::loader::build_mips(SURFACE_SIZE, SURFACE_SIZE, rgba)
                        })
                        .collect();
                    let _ = tx.send(layers);
                });
                self.surface_maps = SurfaceMaps::Decoding(rx);
            }
            SurfaceMaps::Decoding(rx) => {
                if let Ok(layers) = rx.try_recv() {
                    for (layer, mips) in layers.iter().enumerate() {
                        for (level, data) in mips.iter().enumerate().take(SURFACE_MIPS as usize) {
                            let w = (SURFACE_SIZE >> level).max(1);
                            self.queue.write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture: &self.surface_tex,
                                    mip_level: level as u32,
                                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                                    aspect: wgpu::TextureAspect::All,
                                },
                                data,
                                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(w) },
                                wgpu::Extent3d { width: w, height: w, depth_or_array_layers: 1 },
                            );
                        }
                    }
                    self.surface_maps = SurfaceMaps::Ready;
                    return true;
                }
            }
            SurfaceMaps::Unused => {}
        }
        false
    }
}

fn upload_image(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, mips: &[Vec<u8>]) -> GpuImage {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("image"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    for (level, data) in mips.iter().enumerate() {
        let w = (width >> level).max(1);
        let h = (height >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }
    GpuImage {
        linear: texture.create_view(&Default::default()),
        srgb: texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            ..Default::default()
        }),
    }
}

/// Diffuse in the texture's left half, specular in its right half.
fn write_matcap(queue: &wgpu::Queue, tex: &wgpu::Texture, index: usize) {
    let m = matcap::generate(index);
    let size = matcap::SIZE as u32;
    for (half, pixels) in [m.diffuse, m.specular].iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: half as u32 * size, y: 0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
    }
}

fn create_joints_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("joints"),
        size: (capacity.max(1) * 64) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_object_buffer(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    capacity: usize,
    stride: u64,
    joints: &wgpu::Buffer,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("objects"),
        size: capacity as u64 * stride,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("objects"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buf,
                offset: 0,
                size: NonZeroU64::new(OBJECT_SIZE),
            }),
        }, wgpu::BindGroupEntry {
            binding: 1,
            resource: joints.as_entire_binding(),
        }],
    });
    (buf, bg)
}

fn create_selection_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("selection"),
        size: (capacity.max(1) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_pipelines(
    device: &wgpu::Device,
    globals_bgl: &wgpu::BindGroupLayout,
    object_bgl: &wgpu::BindGroupLayout,
    material_bgl: &wgpu::BindGroupLayout,
    outline_bgl: &wgpu::BindGroupLayout,
) -> Pipelines {
    let common = include_str!("shaders/common.wgsl");
    let module = |label, src: &str| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(format!("{common}\n{src}").into()),
        })
    };
    let mesh_module = module("mesh", include_str!("shaders/mesh.wgsl"));
    let grid_module = module("grid", include_str!("shaders/grid.wgsl"));
    let outline_module = module("outline", include_str!("shaders/outline.wgsl"));

    let layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: groups,
            immediate_size: 0,
        })
    };
    let mesh_layout = layout("mesh", &[Some(globals_bgl), Some(object_bgl), Some(material_bgl)]);
    let object_layout = layout("object", &[Some(globals_bgl), Some(object_bgl)]);
    let globals_layout = layout("globals", &[Some(globals_bgl)]);
    let outline_layout = layout("outline", &[Some(outline_bgl)]);

    let attrs: Vec<[wgpu::VertexAttribute; 1]> = [
        wgpu::VertexFormat::Float32x3,
        wgpu::VertexFormat::Float32x3,
        wgpu::VertexFormat::Float32x2,
        wgpu::VertexFormat::Float32x4,
        wgpu::VertexFormat::Float32x4,
        wgpu::VertexFormat::Uint16x4,
        wgpu::VertexFormat::Float32x4,
    ]
    .iter()
    .enumerate()
    .map(|(i, &format)| [wgpu::VertexAttribute { format, offset: 0, shader_location: i as u32 }])
    .collect();
    let buffer_layout = |i: usize| wgpu::VertexBufferLayout {
        array_stride: attrs[i][0].format.size(),
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attrs[i],
    };
    let mesh_buffers: Vec<Option<wgpu::VertexBufferLayout>> = (0..7).map(|i| Some(buffer_layout(i))).collect();
    // Wire and id passes only need positions plus skinning.
    let pos_buffers = [Some(buffer_layout(0)), None, None, None, None, Some(buffer_layout(5)), Some(buffer_layout(6))];

    struct Desc<'a> {
        label: &'a str,
        layout: &'a wgpu::PipelineLayout,
        module: &'a wgpu::ShaderModule,
        vs: &'a str,
        fs: &'a str,
        buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
        topology: wgpu::PrimitiveTopology,
        depth: Option<(bool, wgpu::CompareFunction)>,
        cull: Option<wgpu::Face>,
        samples: u32,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
        write_mask: wgpu::ColorWrites,
        /// Extra color target (the id pass also writes depth for picking).
        second: Option<wgpu::TextureFormat>,
    }
    let build = |d: Desc| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(d.label),
            layout: Some(d.layout),
            vertex: wgpu::VertexState {
                module: d.module,
                entry_point: Some(d.vs),
                compilation_options: Default::default(),
                buffers: d.buffers,
            },
            primitive: wgpu::PrimitiveState { topology: d.topology, cull_mode: d.cull, ..Default::default() },
            depth_stencil: d.depth.map(|(write, compare)| wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: d.samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: d.module,
                entry_point: Some(d.fs),
                compilation_options: Default::default(),
                targets: &[
                    Some(wgpu::ColorTargetState { format: d.format, blend: d.blend, write_mask: d.write_mask }),
                    d.second.map(|format| wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                ][..1 + d.second.is_some() as usize],
            }),
            multiview_mask: None,
            cache: None,
        })
    };

    use wgpu::CompareFunction as Cmp;
    use wgpu::PrimitiveTopology as Topo;
    let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
    let all = wgpu::ColorWrites::ALL;
    let mesh = |label, depth, blend, write_mask, cull| {
        build(Desc {
            label,
            layout: &mesh_layout,
            module: &mesh_module,
            vs: "vs_mesh",
            fs: "fs_mesh",
            buffers: &mesh_buffers,
            topology: Topo::TriangleList,
            depth: Some(depth),
            cull,
            samples: SAMPLES,
            format: COLOR_FORMAT,
            blend,
            write_mask,
            second: None,
        })
    };
    let wire = |label, compare| {
        build(Desc {
            label,
            layout: &object_layout,
            module: &mesh_module,
            vs: "vs_wire",
            fs: "fs_wire",
            buffers: &pos_buffers,
            topology: Topo::LineList,
            depth: Some((false, compare)),
            cull: None,
            samples: SAMPLES,
            format: COLOR_FORMAT,
            blend: alpha,
            write_mask: all,
            second: None,
        })
    };

    // Markers read one instance per quad: endpoints, kind and skinning of both ends.
    let marker_attrs = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Uint32, 2 => Float32x3, 3 => Uint16x4, 4 => Uint16x4, 5 => Float32x4, 6 => Float32x4
    ];
    let marker_buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MarkerInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &marker_attrs,
    })];
    let marker = |label, fs, compare| {
        build(Desc {
            label,
            layout: &object_layout,
            module: &mesh_module,
            vs: "vs_marker",
            fs,
            buffers: &marker_buffers,
            topology: Topo::TriangleList,
            depth: Some((false, compare)),
            cull: None,
            samples: SAMPLES,
            format: COLOR_FORMAT,
            blend: alpha,
            write_mask: all,
            second: None,
        })
    };

    // Normal lines read positions, normals and skinning once per instance (vertex).
    let instance = |i: usize| wgpu::VertexBufferLayout {
        array_stride: attrs[i][0].format.size(),
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &attrs[i],
    };
    let normal_buffers = [Some(instance(0)), Some(instance(1)), Some(instance(5)), Some(instance(6))];
    let normals = build(Desc {
        label: "normals",
        layout: &object_layout,
        module: &mesh_module,
        vs: "vs_normal",
        fs: "fs_normal",
        buffers: &normal_buffers,
        topology: Topo::LineList,
        depth: Some((false, Cmp::GreaterEqual)),
        cull: None,
        samples: SAMPLES,
        format: COLOR_FORMAT,
        blend: alpha,
        write_mask: all,
        second: None,
    });

    Pipelines {
        normals,
        surfaces: [None, Some(wgpu::Face::Back)].map(|cull| SurfacePipelines {
            opaque: mesh("mesh", (true, Cmp::Greater), None, all, cull),
            depth_only: mesh("mesh depth", (true, Cmp::Greater), None, wgpu::ColorWrites::empty(), cull),
            xray: mesh("mesh xray", (false, Cmp::Always), alpha, all, cull),
            // Equal passes too: a depth-only draw of the same mesh comes first (see the blend pass).
            blend: mesh("mesh blend", (false, Cmp::GreaterEqual), alpha, all, cull),
        }),
        wire: wire("wire", Cmp::GreaterEqual),
        wire_xray: wire("wire xray", Cmp::Always),
        marker: marker("markers", "fs_marker", Cmp::GreaterEqual),
        marker_hidden: marker("markers hidden", "fs_marker_hidden", Cmp::Always),
        grid: build(Desc {
            label: "grid",
            layout: &globals_layout,
            module: &grid_module,
            vs: "vs_grid",
            fs: "fs_grid",
            buffers: &[],
            topology: Topo::TriangleList,
            depth: Some((false, Cmp::Greater)),
            cull: None,
            samples: SAMPLES,
            format: COLOR_FORMAT,
            blend: alpha,
            write_mask: all,
            second: None,
        }),
        floor: build(Desc {
            label: "shadow floor",
            layout: &globals_layout,
            module: &mesh_module,
            vs: "vs_floor",
            fs: "fs_floor",
            buffers: &[],
            topology: Topo::TriangleList,
            depth: Some((false, Cmp::Greater)),
            cull: None,
            samples: SAMPLES,
            format: COLOR_FORMAT,
            blend: alpha,
            write_mask: all,
            second: None,
        }),
        background: build(Desc {
            label: "background",
            layout: &globals_layout,
            module: &mesh_module,
            vs: "vs_full",
            fs: "fs_background",
            buffers: &[],
            topology: Topo::TriangleList,
            depth: Some((false, Cmp::Always)),
            cull: None,
            samples: SAMPLES,
            format: COLOR_FORMAT,
            blend: None,
            write_mask: all,
            second: None,
        }),
        id: build(Desc {
            label: "ids",
            layout: &object_layout,
            module: &mesh_module,
            vs: "vs_id",
            fs: "fs_id",
            buffers: &pos_buffers,
            topology: Topo::TriangleList,
            depth: Some((true, Cmp::Greater)),
            cull: None,
            samples: 1,
            format: ID_FORMAT,
            blend: None,
            write_mask: all,
            second: Some(PICK_DEPTH_FORMAT),
        }),
        outline: build(Desc {
            label: "outline",
            layout: &outline_layout,
            module: &outline_module,
            vs: "vs_full",
            fs: "fs_outline",
            buffers: &[],
            topology: Topo::TriangleList,
            depth: None,
            cull: None,
            samples: 1,
            format: COLOR_FORMAT,
            blend: alpha,
            write_mask: all,
            second: None,
        }),
    }
}

use polyloupe_core::color::srgb_to_linear;

/// Stable pastel color per object name, like Blender's "Random" viewport color.
fn random_color(name: &str) -> [f32; 3] {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let hue = (h & 0xffff) as f32 / 65535.0;
    let sat = 0.38 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.2;
    let val = 0.72 + ((h >> 24) & 0xff) as f32 / 255.0 * 0.18;
    let rgb = egui::ecolor::Hsva::new(hue, sat, val, 1.0).to_rgb();
    [rgb[0], rgb[1], rgb[2]]
}

#[cfg(test)]
mod tests {
    use eframe::egui_wgpu::wgpu::naga;

    /// Parses and validates every shader, so WGSL mistakes fail `cargo test` instead of the app.
    #[test]
    fn shaders_validate() {
        let common = include_str!("shaders/common.wgsl");
        let shaders = [
            ("mesh", format!("{common}\n{}", include_str!("shaders/mesh.wgsl"))),
            ("grid", format!("{common}\n{}", include_str!("shaders/grid.wgsl"))),
            ("outline", format!("{common}\n{}", include_str!("shaders/outline.wgsl"))),
            ("ibl", include_str!("shaders/ibl.wgsl").to_string()),
        ];
        for (name, src) in shaders {
            let module = naga::front::wgsl::parse_str(&src)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}

/// Key light when the environment has no clear sun: high, from the front left, like a studio.
const DEFAULT_KEY_LIGHT: Vec3 = Vec3::new(-0.42, -0.55, 0.72);

/// Direction of the environment's brightest spot (its sun or main softbox), or the default
/// studio light when nothing stands out from the rest of the sky.
fn brightest_direction(env: &EnvImage) -> Vec3 {
    let (w, h) = (env.width as usize, env.height as usize);
    if w == 0 || h == 0 {
        return DEFAULT_KEY_LIGHT.normalize();
    }
    let step = (w / 256).max(1);
    let (mut best, mut best_at, mut sum, mut n) = (0.0f32, (0, 0), 0.0f32, 0usize);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let p = env.pixels[y * w + x];
            let lum = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
            sum += lum;
            n += 1;
            if lum > best {
                best = lum;
                best_at = (x, y);
            }
        }
    }
    if n == 0 || best < 4.0 * sum / n as f32 {
        return DEFAULT_KEY_LIGHT.normalize();
    }
    // Inverse of the shaders' equirect_uv: u = 0.5 - atan2(y, x) / 2pi, v = acos(z) / pi.
    let u = (best_at.0 as f32 + 0.5) / w as f32;
    let v = (best_at.1 as f32 + 0.5) / h as f32;
    let phi = (0.5 - u) * std::f32::consts::TAU;
    let theta = v * std::f32::consts::PI;
    Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos())
}

fn create_shadows(device: &wgpu::Device, texture: &wgpu::Texture, object_bgl: &wgpu::BindGroupLayout) -> Shadows {
    let layers = [0, 1].map(|layer| {
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: layer,
            array_layer_count: Some(1),
            ..Default::default()
        })
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadow light"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    });
    let matrices = [0, 1].map(|_| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow light"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    });
    let bind_groups = [0, 1].map(|i| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow light"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: matrices[i].as_entire_binding() }],
        })
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadow"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadow.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadow"),
        bind_group_layouts: &[Some(&bgl), Some(object_bgl)],
        immediate_size: 0,
    });
    let attr = |location, format| [wgpu::VertexAttribute { format, offset: 0, shader_location: location }];
    let (pos, joints, weights) = (
        attr(0, wgpu::VertexFormat::Float32x3),
        attr(5, wgpu::VertexFormat::Uint16x4),
        attr(6, wgpu::VertexFormat::Float32x4),
    );
    let buffer = |attributes: &'static [wgpu::VertexAttribute]| wgpu::VertexBufferLayout {
        array_stride: attributes[0].format.size(),
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes,
    };
    let (pos, joints, weights): (&'static [_; 1], &'static [_; 1], &'static [_; 1]) =
        (Box::leak(Box::new(pos)), Box::leak(Box::new(joints)), Box::leak(Box::new(weights)));
    let buffers = [Some(buffer(pos)), None, None, None, None, Some(buffer(joints)), Some(buffer(weights))];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow map"),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_shadow"), compilation_options: Default::default(), buffers: &buffers },
        // Both sides cast: thin parts and open meshes still throw a shadow.
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: SHADOW_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
        }),
        multisample: Default::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    });
    Shadows { layers, matrices, bind_groups, pipeline }
}
