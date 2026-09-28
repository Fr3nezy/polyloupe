//! Animation playback: samples clips, walks the node hierarchy and produces what the renderer
//! needs each frame (object matrices, joint matrices, morphed vertices).

use glam::{Mat4, Quat, Vec3};

use crate::scene::{Animation, Channel, Interpolation, Mesh, MeshRig, MorphTarget, Property};

/// Per-mesh animation data kept on the CPU after the scene is uploaded.
pub struct MeshAnim {
    rig: MeshRig,
    /// Rest positions/normals, only for meshes with morph targets.
    base: Option<(Vec<[f32; 3]>, Vec<[f32; 3]>)>,
    static_transform: Mat4,
}

pub struct AnimPlayer {
    pub data: Animation,
    meshes: Vec<MeshAnim>,
    pub clip: Option<usize>,
    pub time: f32,
    pub playing: bool,
    pub speed: f32,
    pub looping: bool,
    /// Last applied morph weights per mesh, to skip unchanged uploads.
    last_weights: Vec<Vec<f32>>,
}

pub struct Pose {
    pub object_transforms: Vec<Mat4>,
    pub joints: Vec<Mat4>,
    /// Meshes whose morphed geometry changed: (mesh index, positions, normals).
    pub morphed: Vec<(usize, Vec<[f32; 3]>, Vec<[f32; 3]>)>,
}

impl AnimPlayer {
    pub fn new(data: Animation, meshes: &[Mesh]) -> Self {
        let meshes: Vec<MeshAnim> = meshes
            .iter()
            .map(|m| MeshAnim {
                base: (!m.rig.morph_targets.is_empty()).then(|| (m.positions.clone(), m.normals.clone())),
                rig: m.rig.clone(),
                static_transform: m.transform,
            })
            .collect();
        let last_weights = vec![Vec::new(); meshes.len()];
        Self {
            clip: (!data.clips.is_empty()).then_some(0),
            data,
            meshes,
            time: 0.0,
            playing: false,
            speed: 1.0,
            looping: true,
            last_weights,
        }
    }

    pub fn has_clips(&self) -> bool {
        !self.data.clips.is_empty()
    }

    pub fn duration(&self) -> f32 {
        self.clip.and_then(|c| self.data.clips.get(c)).map_or(0.0, |c| c.duration)
    }

    pub fn fps(&self) -> f32 {
        self.data.fps.max(1.0)
    }

    pub fn frame(&self) -> i32 {
        (self.time * self.fps()).round() as i32
    }

    pub fn last_frame(&self) -> i32 {
        (self.duration() * self.fps()).round() as i32
    }

    pub fn set_frame(&mut self, frame: i32) {
        self.time = (frame as f32 / self.fps()).clamp(0.0, self.duration());
    }

    /// Advances playback. Returns true when time moved.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.playing || self.duration() <= 0.0 {
            return false;
        }
        self.time += dt * self.speed;
        let d = self.duration();
        if self.time > d {
            if self.looping {
                self.time %= d;
            } else {
                self.time = d;
                self.playing = false;
            }
        } else if self.time < 0.0 {
            self.time = if self.looping { d + self.time % d } else { 0.0 };
        }
        true
    }

    /// Evaluates the current clip at the current time.
    pub fn pose(&mut self) -> Pose {
        let nodes = &self.data.nodes;
        let mut trs: Vec<(Vec3, Quat, Vec3)> = nodes.iter().map(|n| (n.translation, n.rotation, n.scale)).collect();
        let mut weights: Vec<Vec<f32>> = nodes.iter().map(|n| n.weights.clone()).collect();
        if let Some(clip) = self.clip.and_then(|c| self.data.clips.get(c)) {
            let t = clip.start + self.time;
            for ch in &clip.channels {
                let Some(node) = trs.get_mut(ch.node) else { continue };
                match ch.property {
                    Property::Translation => node.0 = Vec3::from_slice(&sample(ch, t)),
                    Property::Scale => node.2 = Vec3::from_slice(&sample(ch, t)),
                    Property::Rotation => node.1 = Quat::from_slice(&sample(ch, t)).normalize(),
                    Property::Weights => weights[ch.node] = sample(ch, t),
                }
            }
        }
        let mut world: Vec<Mat4> = Vec::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            let (t, r, s) = trs[i];
            let local = Mat4::from_scale_rotation_translation(s, r, t);
            world.push(match n.parent {
                Some(p) => world[p] * local,
                None => local,
            });
        }

        let object_transforms = self
            .meshes
            .iter()
            .map(|m| match (m.rig.skin, m.rig.node) {
                (Some(_), _) => Mat4::IDENTITY,
                (None, Some(n)) => world[n] * m.rig.offset,
                (None, None) => m.static_transform,
            })
            .collect();

        let mut joints = Vec::new();
        for skin in &self.data.skins {
            for (j, ib) in skin.joints.iter().zip(&skin.inverse_bind) {
                joints.push(world[*j] * *ib);
            }
        }

        let mut morphed = Vec::new();
        for (i, m) in self.meshes.iter().enumerate() {
            let (Some((base_p, base_n)), Some(node)) = (&m.base, m.rig.node) else { continue };
            let w = &weights[node];
            if *w == self.last_weights[i] {
                continue;
            }
            self.last_weights[i] = w.clone();
            let (p, n) = apply_morph(base_p, base_n, &m.rig.morph_targets, w);
            morphed.push((i, p, n));
        }

        Pose { object_transforms, joints, morphed }
    }
}

fn apply_morph(
    base_p: &[[f32; 3]],
    base_n: &[[f32; 3]],
    targets: &[MorphTarget],
    weights: &[f32],
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let mut p: Vec<Vec3> = base_p.iter().map(|v| Vec3::from(*v)).collect();
    let mut n: Vec<Vec3> = base_n.iter().map(|v| Vec3::from(*v)).collect();
    for (t, &w) in targets.iter().zip(weights) {
        if w.abs() < 1e-5 {
            continue;
        }
        for (dst, d) in p.iter_mut().zip(&t.positions) {
            *dst += Vec3::from(*d) * w;
        }
        if let Some(normals) = &t.normals {
            for (dst, d) in n.iter_mut().zip(normals) {
                *dst += Vec3::from(*d) * w;
            }
        }
    }
    (
        p.into_iter().map(|v| v.to_array()).collect(),
        n.into_iter().map(|v| v.normalize_or(Vec3::Z).to_array()).collect(),
    )
}

/// Samples a channel at time `t` (seconds, clip time).
fn sample(ch: &Channel, t: f32) -> Vec<f32> {
    let w = ch.width;
    let keys = ch.times.len();
    let cubic = ch.interpolation == Interpolation::CubicSpline;
    // Value k (skipping cubic-spline tangents).
    let value = |k: usize| -> &[f32] {
        let start = if cubic { (k * 3 + 1) * w } else { k * w };
        &ch.values[start..start + w]
    };
    if keys == 1 || t <= ch.times[0] {
        return value(0).to_vec();
    }
    if t >= ch.times[keys - 1] {
        return value(keys - 1).to_vec();
    }
    let next = ch.times.partition_point(|&x| x <= t);
    let k = next - 1;
    let (t0, t1) = (ch.times[k], ch.times[next]);
    let dt = (t1 - t0).max(1e-8);
    let u = ((t - t0) / dt).clamp(0.0, 1.0);
    match ch.interpolation {
        Interpolation::Step => value(k).to_vec(),
        Interpolation::Linear if ch.property == Property::Rotation => {
            let a = Quat::from_slice(value(k)).normalize();
            let b = Quat::from_slice(value(next)).normalize();
            a.slerp(b, u).to_array().to_vec()
        }
        Interpolation::Linear => value(k).iter().zip(value(next)).map(|(a, b)| a + (b - a) * u).collect(),
        Interpolation::CubicSpline => {
            let out_tangent = &ch.values[(k * 3 + 2) * w..(k * 3 + 3) * w];
            let in_tangent = &ch.values[(next * 3) * w..(next * 3 + 1) * w];
            let (p0, p1) = (value(k), value(next));
            let (u2, u3) = (u * u, u * u * u);
            let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
            let h10 = u3 - 2.0 * u2 + u;
            let h01 = -2.0 * u3 + 3.0 * u2;
            let h11 = u3 - u2;
            (0..w)
                .map(|i| h00 * p0[i] + h10 * dt * out_tangent[i] + h01 * p1[i] + h11 * dt * in_tangent[i])
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(interpolation: Interpolation, property: Property, times: Vec<f32>, values: Vec<f32>, width: usize) -> Channel {
        Channel { node: 0, property, interpolation, times, values, width }
    }

    #[test]
    fn linear_translation_interpolates() {
        let ch = channel(Interpolation::Linear, Property::Translation, vec![0.0, 1.0], vec![0.0, 0.0, 0.0, 2.0, 4.0, 6.0], 3);
        assert_eq!(sample(&ch, 0.5), vec![1.0, 2.0, 3.0]);
        assert_eq!(sample(&ch, 5.0), vec![2.0, 4.0, 6.0]);
    }

    #[test]
    fn step_holds_previous_key() {
        let ch = channel(Interpolation::Step, Property::Scale, vec![0.0, 1.0], vec![1.0, 1.0, 1.0, 3.0, 3.0, 3.0], 3);
        assert_eq!(sample(&ch, 0.99), vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn rotation_slerps_halfway() {
        let a = Quat::IDENTITY;
        let b = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mut values = a.to_array().to_vec();
        values.extend_from_slice(&b.to_array());
        let ch = channel(Interpolation::Linear, Property::Rotation, vec![0.0, 1.0], values, 4);
        let q = Quat::from_slice(&sample(&ch, 0.5));
        assert!(q.angle_between(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)) < 1e-4);
    }

    #[test]
    fn cubic_spline_hits_keys() {
        // (in, value, out) per key, width 1.
        let ch = channel(Interpolation::CubicSpline, Property::Weights, vec![0.0, 2.0], vec![0.0, 1.0, 0.0, 0.0, 5.0, 0.0], 1);
        assert!((sample(&ch, 0.0)[0] - 1.0).abs() < 1e-6);
        assert!((sample(&ch, 2.0)[0] - 5.0).abs() < 1e-6);
        assert!((sample(&ch, 1.0)[0] - 3.0).abs() < 1e-6);
    }
}
