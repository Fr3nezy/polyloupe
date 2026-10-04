//! CPU copy of each object's geometry, kept after loading for the tools that need it: vertex
//! snapping for the Measure tool (the vertex of the clicked object nearest to the cursor on
//! screen, among those close to the surface point under it, so vertices hidden behind the
//! surface don't grab the cursor), laying an object on a face, and model export. Rest pose.

use glam::{Mat4, Vec2, Vec3};

use crate::scene::Aabb;

/// Snap radius around the cursor, in physical pixels at 1x scale.
pub const RADIUS_PX: f32 = 12.0;

/// One object's vertices and triangles in its own space, plus its placement.
pub struct SnapMesh {
    /// The file's placement; `model` adds the user's move on top of it.
    base: Mat4,
    pub model: Mat4,
    inverse: Mat4,
    /// Smallest axis scale of `model`, to turn a world radius into a safe local one.
    min_scale: f32,
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
}

impl SnapMesh {
    pub fn new(model: Mat4, positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Self {
        let mut mesh = Self {
            base: model,
            model,
            inverse: model.inverse(),
            min_scale: 1.0,
            positions: positions.into_iter().map(Vec3::from).collect(),
            indices,
        };
        mesh.place(Mat4::IDENTITY);
        mesh
    }

    /// Puts the object where the user moved it: `user` applies on top of the file's placement.
    pub fn place(&mut self, user: Mat4) {
        self.model = user * self.base;
        self.inverse = self.model.inverse();
        self.min_scale = [self.model.x_axis, self.model.y_axis, self.model.z_axis]
            .iter()
            .map(|a| a.truncate().length())
            .fold(f32::INFINITY, f32::min)
            .max(1e-12);
    }

    /// Exact world bounds at the current placement.
    pub fn world_bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for &p in &self.positions {
            b.grow(self.model.transform_point3(p));
        }
        b
    }

    fn triangles(&self) -> impl Iterator<Item = [Vec3; 3]> + '_ {
        self.indices.chunks_exact(3).filter_map(|t| {
            Some([
                *self.positions.get(t[0] as usize)?,
                *self.positions.get(t[1] as usize)?,
                *self.positions.get(t[2] as usize)?,
            ])
        })
    }

    /// World normal of the triangle under the surface point `hit` (the one nearest to it).
    pub fn face_normal(&self, hit: Vec3) -> Option<Vec3> {
        let local = self.inverse.transform_point3(hit);
        let mut best: Option<(f32, Vec3)> = None;
        for [a, b, c] in self.triangles() {
            let n = (b - a).cross(c - a);
            if n.length_squared() < 1e-24 {
                continue;
            }
            let d = distance_to_triangle(local, a, b, c);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, n));
            }
        }
        let n = best?.1;
        Some(self.inverse.transpose().transform_vector3(n).normalize())
    }

    /// The side to lay the object on for printing: the world direction whose flat faces on the
    /// bed add up to the largest area. Faces count only when they lie on the object's outermost
    /// plane along that direction, so a face inside a recess never wins.
    pub fn best_down_direction(&self) -> Option<Vec3> {
        use std::collections::HashMap;
        let tris: Vec<(Vec3, f32, Vec3)> = self
            .triangles()
            .map(|t| t.map(|p| self.model.transform_point3(p)))
            .filter_map(|[a, b, c]| {
                let n = (b - a).cross(c - a);
                let area = n.length() * 0.5;
                (area > 0.0).then(|| (n / (area * 2.0), area, a))
            })
            .collect();
        // Normals quantized to about one degree, ranked by total area.
        let key = |n: Vec3| (n * 64.0).round().as_ivec3();
        let mut by_dir: HashMap<glam::IVec3, (Vec3, f32)> = HashMap::new();
        for &(n, area, _) in &tris {
            let e = by_dir.entry(key(n)).or_insert((Vec3::ZERO, 0.0));
            e.0 += n * area;
            e.1 += area;
        }
        let mut candidates: Vec<(Vec3, f32)> = by_dir.into_values().map(|(n, a)| (n.normalize_or_zero(), a)).collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        candidates.truncate(24);
        let world: Vec<Vec3> = self.positions.iter().map(|&p| self.model.transform_point3(p)).collect();
        let mut best: Option<(f32, Vec3)> = None;
        for (dir, _) in candidates {
            if dir == Vec3::ZERO {
                continue;
            }
            // Outermost plane along `dir`: the bed once `dir` points down.
            let (lo, hi) = world.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                let d = p.dot(dir);
                (lo.min(d), hi.max(d))
            });
            let tolerance = (hi - lo).max(1e-6) * 1e-3;
            let contact: f32 = tris
                .iter()
                .filter(|(n, _, p)| n.dot(dir) > 0.999 && (hi - p.dot(dir)) <= tolerance)
                .map(|(_, a, _)| a)
                .sum();
            if best.is_none_or(|(ba, _)| contact > ba) {
                best = Some((contact, dir));
            }
        }
        best.filter(|(a, _)| *a > 0.0).map(|(_, d)| d)
    }

    /// The vertex within `radius` pixels of `cursor` (viewport pixels, y down), near `hit`.
    pub fn nearest(&self, hit: Vec3, cursor: Vec2, view_proj: Mat4, size: Vec2, radius: f32) -> Option<Vec3> {
        let to_px = |w: Vec3| {
            let c = view_proj * w.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                Vec2::new((n.x * 0.5 + 0.5) * size.x, (0.5 - n.y * 0.5) * size.y)
            })
        };
        // World size of the snap radius at the hit's depth: unproject a point `radius` pixels
        // to the side. Candidates may sit a few radii away in depth on slanted surfaces.
        let clip = view_proj * hit.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let side = view_proj.inverse() * (ndc + Vec3::new(radius * 2.0 / size.x, 0.0, 0.0)).extend(1.0);
        let world_radius = (side.truncate() / side.w - hit).length() * 3.0;
        let local_hit = self.inverse.transform_point3(hit);
        let local_radius = world_radius / self.min_scale;
        let r2 = local_radius * local_radius;

        let mut best: Option<(f32, Vec3)> = None;
        for &p in &self.positions {
            if p.distance_squared(local_hit) > r2 {
                continue;
            }
            let world = self.model.transform_point3(p);
            let Some(px) = to_px(world) else { continue };
            let d = px.distance(cursor);
            if d <= radius && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, world));
            }
        }
        best.map(|(_, w)| w)
    }
}

/// Distance from `p` to the triangle `abc` (Ericson, Real-Time Collision Detection, 5.1.5).
fn distance_to_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return p.distance(a);
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return p.distance(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return p.distance(a + ab * (d1 / (d1 - d3)));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return p.distance(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return p.distance(a + ac * (d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return p.distance(b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    p.distance(a + ab * (vb * denom) + ac * (vc * denom))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_corner_under_the_cursor() {
        let mesh = SnapMesh::new(Mat4::IDENTITY, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![0, 1, 2]);
        let view = Mat4::look_at_rh(Vec3::new(0.3, 0.3, 5.0), Vec3::new(0.3, 0.3, 0.0), Vec3::Y);
        let proj = Mat4::perspective_rh(0.8, 1.0, 0.1, 100.0);
        let vp = proj * view;
        let size = Vec2::splat(800.0);
        let project = |w: Vec3| {
            let c = vp * w.extend(1.0);
            let n = c.truncate() / c.w;
            Vec2::new((n.x * 0.5 + 0.5) * size.x, (0.5 - n.y * 0.5) * size.y)
        };
        // Cursor 5 px from the (1, 0, 0) corner, hit on the surface next to it.
        let cursor = project(Vec3::X) + Vec2::new(5.0, 0.0);
        let hit = Vec3::new(0.99, 0.0, 0.0);
        assert_eq!(mesh.nearest(hit, cursor, vp, size, 12.0), Some(Vec3::X));
        // Far from every corner: no snap.
        let cursor = project(Vec3::new(0.4, 0.4, 0.0));
        assert_eq!(mesh.nearest(Vec3::new(0.4, 0.4, 0.0), cursor, vp, size, 12.0), None);
    }

    /// A 2 x 1 x 0.2 slab (12 triangles): it should lie on one of its two big faces.
    #[test]
    fn best_down_direction_picks_the_largest_flat_side() {
        let (x, y, z) = (2.0, 1.0, 0.2);
        let p = |i: u32| [if i & 1 != 0 { x } else { 0.0 }, if i & 2 != 0 { y } else { 0.0 }, if i & 4 != 0 { z } else { 0.0 }];
        let positions: Vec<[f32; 3]> = (0..8).map(p).collect();
        let indices = vec![
            0, 2, 1, 1, 2, 3, // -Z
            4, 5, 6, 5, 7, 6, // +Z
            0, 1, 4, 1, 5, 4, // -Y
            2, 6, 3, 3, 6, 7, // +Y
            0, 4, 2, 2, 4, 6, // -X
            1, 3, 5, 3, 7, 5, // +X
        ];
        // Tilt it so the answer isn't an axis by accident of the input.
        let tilt = Mat4::from_rotation_x(0.3);
        let mut mesh = SnapMesh::new(Mat4::IDENTITY, positions, indices);
        mesh.place(tilt);
        let down = mesh.best_down_direction().expect("a side");
        let big = tilt.transform_vector3(Vec3::Z);
        assert!(down.dot(big).abs() > 0.999, "{down}");
        // Clicking the -Z face reports its outward normal.
        let n = mesh.face_normal(tilt.transform_point3(Vec3::new(1.0, 0.5, 0.0))).unwrap();
        assert!(n.dot(-big) > 0.999, "{n}");
    }
}
