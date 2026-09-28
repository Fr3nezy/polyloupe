//! Mesh checks for artists: non-manifold edges, open (boundary) edges, overlapping vertices and
//! degenerate faces, the same problems Blender's "Select Non-Manifold" and "Merge by Distance"
//! find.
//!
//! Loaders split vertices along UV seams and hard edges, so topology is rebuilt first by welding
//! vertices with identical positions. Primitives of the same node (one per material in glTF) are
//! analyzed together, so the border between two materials isn't reported as an open edge.

use std::collections::HashMap;

use glam::Mat4;

use crate::scene::Mesh;

/// Blender's default "Merge by Distance" threshold, in meters.
pub const MERGE_DISTANCE: f32 = 1e-4;

/// Problems found in one mesh, as indices into its own vertex buffer.
#[derive(Clone, Default)]
pub struct MeshMarks {
    /// Edges shared by more than two faces, or by two faces with opposite winding.
    pub non_manifold: Vec<[u32; 2]>,
    /// Edges used by a single face: holes and open borders.
    pub open: Vec<[u32; 2]>,
    /// One vertex per position that has another vertex within `MERGE_DISTANCE`.
    pub overlapping: Vec<u32>,
}

/// Totals for the whole scene, counted on the welded topology (what Blender would show).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Report {
    pub non_manifold_edges: usize,
    pub open_edges: usize,
    pub overlapping_vertices: usize,
    pub degenerate_faces: usize,
    /// Faces whose vertex normals point against their winding (inside out when lit).
    pub inverted_normals: usize,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        *self == Report::default()
    }
}

/// Analyzes every mesh; `marks[i]` belongs to `meshes[i]`.
pub fn analyze(meshes: &[Mesh]) -> (Report, Vec<MeshMarks>) {
    let mut marks = vec![MeshMarks::default(); meshes.len()];
    let mut report = Report::default();
    // Group the primitives of one node; a mesh without a node stands alone.
    let mut groups: HashMap<(bool, usize), Vec<usize>> = HashMap::new();
    for (i, m) in meshes.iter().enumerate() {
        let key = m.rig.node.map_or((false, i), |n| (true, n));
        groups.entry(key).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    groups.sort();
    for group in groups {
        analyze_group(meshes, &group, &mut report, &mut marks);
    }
    (report, marks)
}

fn analyze_group(meshes: &[Mesh], group: &[usize], report: &mut Report, marks: &mut [MeshMarks]) {
    // Global vertex ids: offset of each mesh in the group.
    let mut offsets = Vec::with_capacity(group.len() + 1);
    let mut total = 0usize;
    for &mi in group {
        offsets.push(total);
        total += meshes[mi].positions.len();
    }
    offsets.push(total);
    let locate = |g: usize| -> (usize, u32) {
        let k = offsets.partition_point(|&o| o <= g) - 1;
        (group[k], (g - offsets[k]) as u32)
    };
    let positions: Vec<[f32; 3]> = group.iter().flat_map(|&mi| meshes[mi].positions.iter().copied()).collect();

    // 1. Weld identical positions: weld[g] = first global id at that position.
    let key = |p: [f32; 3]| p.map(|c| if c == 0.0 { 0 } else { c.to_bits() });
    let mut order: Vec<([u32; 3], u32)> = positions.iter().enumerate().map(|(g, &p)| (key(p), g as u32)).collect();
    order.sort_unstable();
    let mut weld = vec![0u32; total];
    let mut unique = Vec::new();
    let mut run_start = 0usize;
    for k in 0..order.len() {
        if k == 0 || order[k].0 != order[run_start].0 {
            run_start = k;
            unique.push(order[k].1);
        }
        weld[order[k].1 as usize] = order[run_start].1;
    }
    drop(order);
    let position = |g: usize| positions[g];

    // 2. Edges of the welded triangles, as (edge key, direction bit | triangle corner). Corners
    // count through the group's index buffers; the corner starts the edge that produced it.
    let mut corner_offsets = Vec::with_capacity(group.len() + 1);
    let mut corners = 0usize;
    for &mi in group {
        corner_offsets.push(corners);
        corners += meshes[mi].indices.len();
    }
    corner_offsets.push(corners);
    let mut edges: Vec<(u64, u32)> = Vec::with_capacity(corners);
    for (k, &mi) in group.iter().enumerate() {
        let base = offsets[k] as u32;
        let corner_base = corner_offsets[k] as u32;
        for (t, tri) in meshes[mi].indices.chunks_exact(3).enumerate() {
            let w = [0, 1, 2].map(|j| weld[(base + tri[j]) as usize]);
            if w[0] == w[1] || w[1] == w[2] || w[0] == w[2] {
                report.degenerate_faces += 1;
                continue;
            }
            // Shading normals against the face's winding.
            let m = &meshes[mi];
            let p = [0, 1, 2].map(|j| glam::Vec3::from(m.positions[tri[j] as usize]));
            let face = (p[1] - p[0]).cross(p[2] - p[0]);
            let shading: glam::Vec3 = (0..3).map(|j| glam::Vec3::from(m.normals[tri[j] as usize])).sum();
            if face.dot(shading) < 0.0 {
                report.inverted_normals += 1;
            }
            for j in 0..3 {
                let (a, b) = (w[j], w[(j + 1) % 3]);
                let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                let corner = corner_base + (t * 3 + j) as u32;
                edges.push(((lo as u64) << 32 | hi as u64, corner | ((a < b) as u32) << 31));
            }
        }
    }
    edges.sort_unstable_by_key(|e| e.0);
    // The edge's two vertices, in the mesh that owns the corner.
    let edge_of = |corner: u32| -> (usize, [u32; 2]) {
        let c = (corner & !(1 << 31)) as usize;
        let k = corner_offsets.partition_point(|&o| o <= c) - 1;
        let mi = group[k];
        let local = c - corner_offsets[k];
        let tri = &meshes[mi].indices[local - local % 3..][..3];
        (mi, [tri[local % 3], tri[(local % 3 + 1) % 3]])
    };
    let mut i = 0;
    while i < edges.len() {
        let mut j = i + 1;
        while j < edges.len() && edges[j].0 == edges[i].0 {
            j += 1;
        }
        let run = &edges[i..j];
        match run.len() {
            1 => {
                report.open_edges += 1;
                let (mi, e) = edge_of(run[0].1);
                marks[mi].open.push(e);
            }
            2 if (run[0].1 >> 31) != (run[1].1 >> 31) => {}
            _ => {
                report.non_manifold_edges += 1;
                let (mi, e) = edge_of(run[0].1);
                marks[mi].non_manifold.push(e);
            }
        }
        i = j;
    }
    drop(edges);

    // 3. Distinct positions closer than the merge distance (in world units). Cells are twice
    // the distance, so a point's neighbors lie in its own cell or the one on its nearer side
    // along each axis: 8 cells to look at.
    let eps = MERGE_DISTANCE / max_scale(&meshes[group[0]].transform).max(1e-12);
    let size = eps * 2.0;
    let cell = |p: [f32; 3]| p.map(|c| (c / size).floor() as i64);
    let mut by_cell: Vec<([i64; 3], u32)> = unique.iter().map(|&g| (cell(position(g as usize)), g)).collect();
    by_cell.sort_unstable();
    let mut grid: HashMap<[i64; 3], (u32, u32), FastHash> = HashMap::with_capacity_and_hasher(by_cell.len(), FastHash);
    let mut start = 0;
    for k in 1..=by_cell.len() {
        if k == by_cell.len() || by_cell[k].0 != by_cell[start].0 {
            grid.insert(by_cell[start].0, (start as u32, k as u32));
            start = k;
        }
    }
    // Read-only lookups into a large table: memory bound, so spread over the cores.
    let is_close = |g: u32| {
        let p = position(g as usize);
        let c = cell(p);
        let side = [0, 1, 2].map(|a| if p[a] / size - c[a] as f32 >= 0.5 { 1 } else { -1 });
        (0..8).any(|n| {
            let key = [0, 1, 2].map(|a| c[a] + if n >> a & 1 == 1 { side[a] } else { 0 });
            grid.get(&key).is_some_and(|&(from, to)| {
                by_cell[from as usize..to as usize].iter().any(|&(_, other)| {
                    let q = position(other as usize);
                    other != g && (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2) <= eps * eps
                })
            })
        })
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = unique.len().div_ceil(threads).max(4096);
    let flagged: Vec<bool> = std::thread::scope(|scope| {
        let handles: Vec<_> = unique
            .chunks(chunk)
            .map(|part| scope.spawn(|| part.iter().map(|&g| is_close(g)).collect::<Vec<bool>>()))
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("mesh analysis thread")).collect()
    });
    for (u, &g) in unique.iter().enumerate() {
        if flagged[u] {
            report.overlapping_vertices += 1;
            let (mi, v) = locate(g as usize);
            marks[mi].overlapping.push(v);
        }
    }
}

/// Multiply-rotate hasher for the grid lookup: integer keys, no DoS concerns, and SipHash
/// would dominate the analysis time on million-vertex meshes.
#[derive(Clone, Copy, Default)]
struct FastHash;

impl std::hash::BuildHasher for FastHash {
    type Hasher = FastHasher;
    fn build_hasher(&self) -> FastHasher {
        FastHasher(0)
    }
}

struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        // Final avalanche (splitmix64), so every input bit reaches the bucket bits.
        let mut h = self.0;
        h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        h ^ (h >> 31)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
}

fn max_scale(m: &Mat4) -> f32 {
    m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{MeshData, MeshRig};

    fn mesh(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Mesh {
        Mesh::build(
            MeshData {
                name: "m".into(),
                positions,
                normals: None,
                uvs: None,
                tangents: None,
                colors: None,
                indices,
                transform: Mat4::IDENTITY,
                material: 0,
                rig: MeshRig::default(),
            },
            false,
        )
    }

    /// Closed tetrahedron, with vertices split per face like a loader does for flat shading.
    fn tetra() -> (Vec<[f32; 3]>, Vec<u32>) {
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let faces = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for f in faces {
            for v in f {
                indices.push(positions.len() as u32);
                positions.push(p[v]);
            }
        }
        (positions, indices)
    }

    #[test]
    fn closed_split_mesh_is_clean() {
        let (p, i) = tetra();
        let (report, _) = analyze(&[mesh(p, i)]);
        assert!(report.is_clean(), "{report:?}");
    }

    #[test]
    fn hole_and_extra_face() {
        let (p, i) = tetra();
        // Drop one face: three open edges.
        let (report, marks) = analyze(&[mesh(p.clone(), i[..9].to_vec())]);
        assert_eq!(report.open_edges, 3);
        assert_eq!(marks[0].open.len(), 3);
        // Duplicate one face: its three edges now have three faces each.
        let mut j = i.clone();
        j.extend_from_slice(&i[..3]);
        let (report, _) = analyze(&[mesh(p, j)]);
        assert_eq!(report.non_manifold_edges, 3);
    }

    #[test]
    fn flipped_face_and_overlap_and_degenerate() {
        let (mut p, mut i) = tetra();
        i.swap(0, 1);
        let (report, _) = analyze(&[mesh(p.clone(), i.clone())]);
        assert_eq!(report.non_manifold_edges, 3, "a flipped face makes its edges non-contiguous");
        i.swap(0, 1);
        // Move one split copy of vertex 1 by 5e-5 m: an overlapping pair and a crack.
        p[2][0] += 5e-5;
        i.extend_from_slice(&[0, 0, 1]);
        let (report, marks) = analyze(&[mesh(p, i)]);
        assert_eq!(report.overlapping_vertices, 2);
        assert_eq!(marks[0].overlapping.len(), 2);
        assert_eq!(report.degenerate_faces, 1);
    }

    #[test]
    fn inverted_normals_are_counted() {
        let (p, i) = tetra();
        let mut m = mesh(p, i);
        assert_eq!(analyze(std::slice::from_ref(&m)).0.inverted_normals, 0);
        for n in &mut m.normals {
            *n = n.map(|c| -c);
        }
        assert_eq!(analyze(&[m]).0.inverted_normals, 4);
    }

    #[test]
    fn material_split_is_not_open() {
        let (p, i) = tetra();
        // Two primitives of the same object, split by material.
        let mut a = mesh(p.clone(), i[..6].to_vec());
        let mut b = mesh(p, i[6..].to_vec());
        a.rig.node = Some(0);
        b.rig.node = Some(0);
        let (report, _) = analyze(&[a, b]);
        assert!(report.is_clean(), "{report:?}");
    }
}
