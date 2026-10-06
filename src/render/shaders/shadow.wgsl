// Shadow maps: depth only, seen from a light. Not prepended with common.wgsl: this pass binds a
// light matrix of its own in group 0, since the globals group samples these very maps.

struct Object {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    color: vec4<f32>,
    info: vec4<u32>,
};

@group(0) @binding(0) var<uniform> light: mat4x4<f32>;
@group(1) @binding(0) var<uniform> obj: Object;
@group(1) @binding(1) var<storage, read> joint_mats: array<mat4x4<f32>>;

const SKINNED: u32 = 32u;

@vertex
fn vs_shadow(
    @location(0) pos: vec3<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
) -> @builtin(position) vec4<f32> {
    var m = obj.model;
    if (obj.info.y & SKINNED) != 0u {
        let b = obj.info.z;
        m = joint_mats[b + joints.x] * weights.x
            + joint_mats[b + joints.y] * weights.y
            + joint_mats[b + joints.z] * weights.z
            + joint_mats[b + joints.w] * weights.w;
    }
    return light * (m * vec4<f32>(pos, 1.0));
}
