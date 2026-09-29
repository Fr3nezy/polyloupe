//! CPU-side scene representation produced by the loaders and consumed by the renderer.

use glam::{Mat4, Quat, Vec2, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    pub fn is_valid(&self) -> bool {
        self.min.x <= self.max.x && self.min.y <= self.max.y && self.min.z <= self.max.z
    }

    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn union(&mut self, other: &Aabb) {
        if other.is_valid() {
            self.grow(other.min);
            self.grow(other.max);
        }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn radius(&self) -> f32 {
        self.size().length() * 0.5
    }

    pub fn transformed(&self, m: &Mat4) -> Aabb {
        let mut out = Aabb::EMPTY;
        if !self.is_valid() {
            return out;
        }
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { self.min.x } else { self.max.x },
                if i & 2 == 0 { self.min.y } else { self.max.y },
                if i & 4 == 0 { self.min.z } else { self.max.z },
            );
            out.grow(m.transform_point3(corner));
        }
        out
    }
}

/// Decoded RGBA8 image with its full mip chain (level 0 first).
pub struct Image {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

/// A texture reference that reads a single channel (0 = R … 3 = A).
#[derive(Clone, Copy, Debug)]
pub struct ChannelTex {
    pub image: usize,
    pub channel: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask(f32),
    Blend,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    /// Linear RGBA factor, multiplied with the base color texture.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub alpha_mode: AlphaMode,
    pub base_color_tex: Option<usize>,
    pub normal_tex: Option<usize>,
    pub emissive_tex: Option<usize>,
    pub metallic_tex: Option<ChannelTex>,
    pub roughness_tex: Option<ChannelTex>,
    pub occlusion_tex: Option<ChannelTex>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "Default".into(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_mode: AlphaMode::Opaque,
            base_color_tex: None,
            normal_tex: None,
            emissive_tex: None,
            metallic_tex: None,
            roughness_tex: None,
            occlusion_tex: None,
        }
    }
}

impl Material {
    /// Texture maps in display order, for the sidebar.
    pub fn maps(&self) -> Vec<MapRef> {
        let mut out = Vec::new();
        if let Some(image) = self.base_color_tex {
            out.push(MapRef { label: "Base Color", image, channel: None });
        }
        // Packed maps (glTF's metallic-roughness texture) name the channel they use.
        let shared = |t: ChannelTex| {
            [self.metallic_tex, self.roughness_tex, self.occlusion_tex]
                .iter()
                .flatten()
                .filter(|o| o.image == t.image)
                .count()
                > 1
        };
        let packed = |t: Option<ChannelTex>, names: [&'static str; 5]| {
            t.map(|t| {
                let label = if shared(t) { names[1 + t.channel.min(3) as usize] } else { names[0] };
                MapRef { label, image: t.image, channel: Some(t.channel) }
            })
        };
        out.extend(packed(self.roughness_tex, ["Roughness", "Roughness · R", "Roughness · G", "Roughness · B", "Roughness · A"]));
        out.extend(packed(self.metallic_tex, ["Metallic", "Metallic · R", "Metallic · G", "Metallic · B", "Metallic · A"]));
        if let Some(image) = self.normal_tex {
            out.push(MapRef { label: "Normal", image, channel: None });
        }
        out.extend(packed(self.occlusion_tex, ["Occlusion", "Occlusion · R", "Occlusion · G", "Occlusion · B", "Occlusion · A"]));
        if let Some(image) = self.emissive_tex {
            out.push(MapRef { label: "Emission", image, channel: None });
        }
        out
    }
}

/// One texture map of a material, as shown in the sidebar.
#[derive(Clone, Copy, Debug)]
pub struct MapRef {
    pub label: &'static str,
    pub image: usize,
    /// The single channel the material reads, for data maps.
    pub channel: Option<u8>,
}

impl Image {
    /// Small preview (longest side <= `max`) from the mip chain. Single-channel maps are shown
    /// as grayscale, so packed textures don't look like strange pink images.
    pub fn thumbnail(&self, max: u32, channel: Option<u8>) -> ([usize; 2], Vec<u8>) {
        let mut level = 0;
        while level + 1 < self.mips.len() && (self.width >> level).max(self.height >> level) > max {
            level += 1;
        }
        let w = (self.width >> level).max(1) as usize;
        let h = (self.height >> level).max(1) as usize;
        let src = &self.mips[level];
        let pixels = match channel {
            Some(c) => src
                .chunks_exact(4)
                .flat_map(|p| {
                    let v = p[c.min(3) as usize];
                    [v, v, v, 255]
                })
                .collect(),
            None => src.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        };
        ([w, h], pixels)
    }
}

pub struct Mesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Option<Vec<[f32; 2]>>,
    /// xyz tangent, w bitangent sign.
    pub tangents: Option<Vec<[f32; 4]>>,
    /// Linear RGBA vertex colors.
    pub colors: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    /// Unique edges as index pairs, used by the wireframe pass.
    pub edges: Vec<u32>,
    /// Object-to-world transform in the rest pose (Z-up world, like Blender).
    pub transform: Mat4,
    pub material: usize,
    /// Local-space bounds.
    pub bounds: Aabb,
    pub rig: MeshRig,
}

/// How a mesh follows the scene's node hierarchy and animation.
#[derive(Clone, Default)]
pub struct MeshRig {
    /// Node whose world transform places the mesh (None = static `transform`).
    pub node: Option<usize>,
    /// Extra transform between the node and the geometry (FBX geometric transforms).
    pub offset: Mat4,
    pub skin: Option<usize>,
    /// Up to four joints (indices into the skin's joint list) and weights per vertex.
    pub joints: Option<Vec<[u16; 4]>>,
    pub weights: Option<Vec<[f32; 4]>>,
    pub morph_targets: Vec<MorphTarget>,
}

#[derive(Clone)]
pub struct MorphTarget {
    #[allow(dead_code)] // For a shape key list in the sidebar.
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
}

pub struct MeshData {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub transform: Mat4,
    pub material: usize,
    pub rig: MeshRig,
}

impl Mesh {
    /// Finishes a mesh: fills missing normals (and tangents when needed), builds edges and bounds.
    pub fn build(data: MeshData, needs_tangents: bool) -> Self {
        let MeshData {
            name,
            positions,
            normals,
            uvs,
            tangents,
            colors,
            indices,
            transform,
            material,
            rig,
        } = data;
        let n = positions.len();
        let normals = match normals {
            Some(v) if v.len() == n => v,
            _ => smooth_normals(&positions, &indices),
        };
        let uvs = uvs.filter(|v| v.len() == n);
        let colors = colors.filter(|v| v.len() == n);
        let tangents = match (tangents.filter(|v| v.len() == n), &uvs) {
            (Some(t), _) => Some(t),
            (None, Some(uv)) if needs_tangents => Some(generate_tangents(&positions, &normals, uv, &indices)),
            _ => None,
        };
        let mut bounds = Aabb::EMPTY;
        for p in &positions {
            bounds.grow(Vec3::from(*p));
        }
        let edges = unique_edges(&indices);
        Self {
            name,
            positions,
            normals,
            uvs,
            tangents,
            colors,
            indices,
            edges,
            transform,
            material,
            bounds,
            rig,
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

pub struct Scene {
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub bounds: Aabb,
    /// Number of vertices as authored (before any splitting done for shading or UV seams).
    pub source_vertex_count: usize,
    /// Non-fatal problems worth telling the user about (e.g. missing textures).
    pub warnings: Vec<String>,
    pub animation: Animation,
    /// Units the file declares (the scene itself is always in meters).
    pub units: Units,
}

/// What a file says about its units.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Units {
    /// The format doesn't store units (OBJ, STL): read as meters.
    #[default]
    Undeclared,
    /// Meters by definition (glTF).
    Meters,
    /// Declared by the file: meters per file unit (FBX: 0.01 = centimeters).
    Declared(f64),
}

/// Node hierarchy, skins and clips. Empty for static files.
#[derive(Clone, Default)]
pub struct Animation {
    /// Parents always come before their children.
    pub nodes: Vec<Node>,
    pub skins: Vec<Skin>,
    pub clips: Vec<Clip>,
    /// Frame rate for the timeline display.
    pub fps: f32,
}

impl Animation {
    pub fn is_animated(&self) -> bool {
        !self.clips.is_empty()
    }

    /// World matrices of the rest pose.
    pub fn rest_world(&self) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let local = n.local();
            world.push(match n.parent {
                Some(p) => world[p] * local,
                None => local,
            });
        }
        world
    }
}

#[derive(Clone)]
pub struct Node {
    #[allow(dead_code)] // For a bone/hierarchy view in the outliner.
    pub name: String,
    pub parent: Option<usize>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    /// Morph target weights driven by this node.
    pub weights: Vec<f32>,
}

impl Node {
    pub fn local(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Clone)]
pub struct Skin {
    /// Node index of each joint.
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Interpolation {
    Step,
    Linear,
    CubicSpline,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
    Weights,
}

#[derive(Clone)]
pub struct Channel {
    pub node: usize,
    pub property: Property,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// Flattened keys; cubic splines store (in-tangent, value, out-tangent) per key.
    pub values: Vec<f32>,
    /// Floats per value (3 translation/scale, 4 rotation, N weights).
    pub width: usize,
}

#[derive(Clone)]
pub struct Clip {
    pub name: String,
    pub start: f32,
    pub duration: f32,
    pub channels: Vec<Channel>,
}

impl Scene {
    pub fn new(
        meshes: Vec<Mesh>,
        mut materials: Vec<Material>,
        images: Vec<Image>,
        source_vertex_count: usize,
    ) -> Self {
        if materials.is_empty() {
            materials.push(Material::default());
        }
        let mut bounds = Aabb::EMPTY;
        for m in &meshes {
            bounds.union(&m.bounds.transformed(&m.transform));
        }
        Self {
            meshes,
            materials,
            images,
            bounds,
            source_vertex_count,
            warnings: Vec::new(),
            animation: Animation::default(),
            units: Units::Undeclared,
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(Mesh::triangle_count).sum()
    }
}

/// Area-weighted smooth normals.
pub fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (pa, pb, pc) = (
            Vec3::from(positions[a]),
            Vec3::from(positions[b]),
            Vec3::from(positions[c]),
        );
        // Unnormalized cross product = area weighting for free.
        let n = (pb - pa).cross(pc - pa);
        acc[a] += n;
        acc[b] += n;
        acc[c] += n;
    }
    acc.into_iter()
        .map(|n| n.try_normalize().unwrap_or(Vec3::Z).to_array())
        .collect()
}

/// MikkTSpace tangents, the standard every baker (Blender, Substance, Marmoset) agrees on.
fn generate_tangents(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Vec<[f32; 4]> {
    struct Geo<'a> {
        positions: &'a [[f32; 3]],
        normals: &'a [[f32; 3]],
        uvs: &'a [[f32; 2]],
        indices: &'a [u32],
        out: Vec<[f32; 4]>,
    }
    impl Geo<'_> {
        fn index(&self, face: usize, vert: usize) -> usize {
            self.indices[face * 3 + vert] as usize
        }
    }
    impl mikktspace::Geometry for Geo<'_> {
        fn num_faces(&self) -> usize {
            self.indices.len() / 3
        }
        fn num_vertices_of_face(&self, _face: usize) -> usize {
            3
        }
        fn position(&self, face: usize, vert: usize) -> [f32; 3] {
            self.positions[self.index(face, vert)]
        }
        fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
            self.normals[self.index(face, vert)]
        }
        fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
            self.uvs[self.index(face, vert)]
        }
        fn set_tangent_encoded(&mut self, tangent: [f32; 4], face: usize, vert: usize) {
            let i = self.index(face, vert);
            self.out[i] = tangent;
        }
    }
    let mut geo = Geo {
        positions,
        normals,
        uvs,
        indices,
        out: vec![[1.0, 0.0, 0.0, 1.0]; positions.len()],
    };
    if !mikktspace::generate_tangents(&mut geo) {
        // Degenerate UVs: fall back to a simple per-triangle basis.
        return fallback_tangents(positions, normals, uvs, indices);
    }
    geo.out
}

fn fallback_tangents(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Vec<[f32; 4]> {
    let mut acc = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let i = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let p = i.map(|k| Vec3::from(positions[k]));
        let t = i.map(|k| Vec2::from(uvs[k]));
        let (e1, e2) = (p[1] - p[0], p[2] - p[0]);
        let (d1, d2) = (t[1] - t[0], t[2] - t[0]);
        let det = d1.x * d2.y - d2.x * d1.y;
        if det.abs() < 1e-12 {
            continue;
        }
        let tangent = (e1 * d2.y - e2 * d1.y) / det;
        for k in i {
            acc[k] += tangent;
        }
    }
    acc.iter()
        .zip(normals)
        .map(|(t, n)| {
            let n = Vec3::from(*n);
            let t = (*t - n * n.dot(*t)).try_normalize().unwrap_or(n.any_orthonormal_vector());
            [t.x, t.y, t.z, 1.0]
        })
        .collect()
}

/// Deduplicated triangle edges as a flat list of index pairs.
pub fn unique_edges(indices: &[u32]) -> Vec<u32> {
    let mut keys: Vec<u64> = Vec::with_capacity(indices.len());
    for tri in indices.chunks_exact(3) {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            keys.push(((lo as u64) << 32) | hi as u64);
        }
    }
    keys.sort_unstable();
    keys.dedup();
    let mut out = Vec::with_capacity(keys.len() * 2);
    for k in keys {
        out.push((k >> 32) as u32);
        out.push(k as u32);
    }
    out
}

/// Converts a Y-up asset (glTF) into the Z-up world, same as Blender's importers.
pub fn y_up_to_z_up() -> Mat4 {
    Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
}
