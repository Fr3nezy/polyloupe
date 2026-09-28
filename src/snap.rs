//! Vertex snapping for the Measure tool: the vertex of the clicked object nearest to the
//! cursor on screen, among those close to the surface point under it (so vertices hidden
//! behind the surface don't grab the cursor). Uses the rest pose.

use glam::{Mat4, Vec2, Vec3};

/// Snap radius around the cursor, in physical pixels at 1x scale.
pub const RADIUS_PX: f32 = 12.0;

/// One object's vertices in its own space, plus its placement.
pub struct SnapMesh {
    model: Mat4,
    inverse: Mat4,
    /// Smallest axis scale of `model`, to turn a world radius into a safe local one.
    min_scale: f32,
    positions: Vec<Vec3>,
}

impl SnapMesh {
    pub fn new(model: Mat4, positions: Vec<[f32; 3]>) -> Self {
        let min_scale = [model.x_axis, model.y_axis, model.z_axis]
            .iter()
            .map(|a| a.truncate().length())
            .fold(f32::INFINITY, f32::min)
            .max(1e-12);
        Self { model, inverse: model.inverse(), min_scale, positions: positions.into_iter().map(Vec3::from).collect() }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_corner_under_the_cursor() {
        let mesh = SnapMesh::new(Mat4::IDENTITY, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
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
}
