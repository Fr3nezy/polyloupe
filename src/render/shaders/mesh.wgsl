// Prepended with common.wgsl.

struct Object {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    // Linear RGB + alpha, already resolved from the active color mode.
    color: vec4<f32>,
    // x: object id + 1, y: flags (1 uv, 2 tangent, 4 color, 8 selected, 16 active),
    // z: first joint matrix (skinning), w: texture pass override + 1 (0 = none).
    info: vec4<u32>,
};

struct MaterialU {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    // x: metallic, y: roughness, z: normal scale, w: occlusion strength.
    pbr: vec4<f32>,
    // x: metallic channel, y: roughness channel, z: occlusion channel,
    // w: flags (1 base, 2 normal, 4 metallic, 8 roughness, 16 occlusion, 32 emissive).
    channels: vec4<u32>,
    // x: alpha cutoff, y: mode (0 opaque, 1 mask, 2 blend), z: clearcoat, w: clearcoat roughness.
    alpha: vec4<f32>,
    // x: transmission, y: ior, z: unused, w: unused.
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var matcap_tex: texture_2d<f32>;
@group(0) @binding(2) var clamp_samp: sampler;
@group(0) @binding(3) var env_src: texture_2d<f32>;
@group(0) @binding(4) var env_spec: texture_2d<f32>;
@group(0) @binding(5) var env_irr: texture_2d<f32>;
@group(0) @binding(6) var brdf_lut: texture_2d<f32>;
@group(0) @binding(7) var env_samp: sampler;
// Surface wear maps (layer 0 grain, 1 scratches), see `SURFACE_MAPS`.
@group(0) @binding(8) var surface_tex: texture_2d_array<f32>;
@group(0) @binding(9) var surface_samp: sampler;
// Shadow maps (layer 0 key light, 1 straight down), see `shadow.wgsl`.
@group(0) @binding(10) var shadow_tex: texture_depth_2d_array;
@group(0) @binding(11) var shadow_samp: sampler_comparison;

const SHADOW_SIZE: f32 = 2048.0;

// Interleaved gradient noise for jittered sampling.
fn interleaved_gradient_noise(pos: vec2<f32>) -> f32 {
    let magic = vec3<f32>(0.06711056, 0.00583715, 52.9829189);
    return fract(magic.z * fract(dot(pos, magic.xy)));
}

// 32 Vogel disk samples distributed uniformly on the unit disk.
const VOGEL_SAMPLES = array<vec2<f32>, 32>(
    vec2<f32>(0.125000, 0.000000),
    vec2<f32>(-0.159645, 0.146248),
    vec2<f32>(0.024436, -0.278438),
    vec2<f32>(0.201222, 0.262459),
    vec2<f32>(-0.369268, -0.065318),
    vec2<f32>(0.349802, -0.222516),
    vec2<f32>(-0.117002, 0.435242),
    vec2<f32>(-0.223136, -0.429634),
    vec2<f32>(0.484115, 0.176798),
    vec2<f32>(-0.503641, 0.207896),
    vec2<f32>(0.242788, -0.518824),
    vec2<f32>(0.179414, 0.572001),
    vec2<f32>(-0.540757, -0.313380),
    vec2<f32>(0.634370, -0.139464),
    vec2<f32>(-0.387146, 0.550675),
    vec2<f32>(-0.089440, -0.690200),
    vec2<f32>(0.549072, 0.462758),
    vec2<f32>(-0.738878, 0.030555),
    vec2<f32>(0.538955, -0.536332),
    vec2<f32>(-0.036058, 0.779792),
    vec2<f32>(-0.512818, -0.614527),
    vec2<f32>(0.812360, 0.109302),
    vec2<f32>(-0.688311, 0.478909),
    vec2<f32>(0.188086, -0.836061),
    vec2<f32>(0.435033, 0.759191),
    vec2<f32>(-0.850448, -0.271316),
    vec2<f32>(0.826102, -0.381680),
    vec2<f32>(-0.357888, 0.855156),
    vec2<f32>(-0.319407, -0.888034),
    vec2<f32>(0.849909, 0.446688),
    vec2<f32>(-0.944035, 0.248845),
    vec2<f32>(0.536596, -0.834530),
);

// How lit `p` is from shadow map `layer` (1 = lit), filtered over a Vogel disk of taps `radius`
// texels apart, rotated per screen pixel with interleaved gradient noise to eliminate banding.
fn shadow_at(p: vec3<f32>, layer: i32, m: mat4x4<f32>, radius: f32, screen_pos: vec2<f32>) -> f32 {
    let c = m * vec4<f32>(p, 1.0);
    let uv = vec2<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || c.z > 1.0 {
        return 1.0;
    }
    let texel = radius / SHADOW_SIZE;
    let phi = interleaved_gradient_noise(screen_pos) * 6.2831853;
    let cos_phi = cos(phi);
    let sin_phi = sin(phi);

    var sum = 0.0;
    for (var i = 0; i < 32; i++) {
        let s = VOGEL_SAMPLES[i];
        let o = vec2<f32>(s.x * cos_phi - s.y * sin_phi, s.x * sin_phi + s.y * cos_phi) * texel;
        sum += textureSampleCompareLevel(shadow_tex, shadow_samp, uv + o, layer, c.z - 0.0015);
    }
    return sum / 32.0;
}

struct PbrOut {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
};

// GGX specular and Lambert diffuse from the key light, shadowed.
fn key_light_pbr(p: vec3<f32>, n: vec3<f32>, v: vec3<f32>, albedo: vec3<f32>, metallic: f32, rough: f32, screen_pos: vec2<f32>) -> PbrOut {
    var out: PbrOut;
    out.diffuse = vec3<f32>(0.0);
    out.specular = vec3<f32>(0.0);
    let l = g.light.xyz;
    let nl = dot(n, l);
    if nl <= 0.0 {
        return out;
    }
    let lit = shadow_at(p + n * g.shadow.w, 0, g.light0, 1.0 + g.shadow.z, screen_pos);
    if lit <= 0.0 {
        return out;
    }
    let h = normalize(l + v);
    let nv = max(dot(n, v), 1e-4);
    let nh = max(dot(n, h), 0.0);
    let a = rough * rough;
    let a2 = a * a;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    let ndf = a2 / (PI * d * d);
    let k = (rough + 1.0) * (rough + 1.0) * 0.125;
    let vis = 0.25 / ((nl * (1.0 - k) + k) * (nv * (1.0 - k) + k));
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - max(dot(v, h), 0.0), 5.0);
    let diffuse = (vec3<f32>(1.0) - f) * (1.0 - metallic) * albedo / PI;
    let spec = f * ndf * vis;
    out.diffuse = diffuse * nl * g.light.w * lit;
    out.specular = spec * nl * g.light.w * lit;
    return out;
}

@group(1) @binding(0) var<uniform> obj: Object;
@group(1) @binding(1) var<storage, read> joint_mats: array<mat4x4<f32>>;

@group(2) @binding(0) var<uniform> mat: MaterialU;
@group(2) @binding(1) var base_tex: texture_2d<f32>;
@group(2) @binding(2) var normal_tex: texture_2d<f32>;
@group(2) @binding(3) var metallic_tex: texture_2d<f32>;
@group(2) @binding(4) var roughness_tex: texture_2d<f32>;
@group(2) @binding(5) var occlusion_tex: texture_2d<f32>;
@group(2) @binding(6) var emissive_tex: texture_2d<f32>;
@group(2) @binding(7) var mat_samp: sampler;

const HAS_UV: u32 = 1u;
const HAS_TANGENT: u32 = 2u;
const HAS_COLOR: u32 = 4u;
const SELECTED: u32 = 8u;
const ACTIVE: u32 = 16u;
const SKINNED: u32 = 32u;

// Object-to-world matrix for this vertex: the object's, or its skin blend.
fn vertex_matrix(joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    if (obj.info.y & SKINNED) == 0u {
        return obj.model;
    }
    let b = obj.info.z;
    return joint_mats[b + joints.x] * weights.x
        + joint_mats[b + joints.y] * weights.y
        + joint_mats[b + joints.z] * weights.z
        + joint_mats[b + joints.w] * weights.w;
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
};

@vertex
fn vs_mesh(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    let m = vertex_matrix(joints, weights);
    let wp = m * vec4<f32>(pos, 1.0);
    out.clip = g.view_proj * wp;
    out.world_pos = wp.xyz;
    if (obj.info.y & SKINNED) != 0u {
        // Joints carry rotation and (mostly uniform) scale: good enough for normals.
        out.normal = (m * vec4<f32>(normal, 0.0)).xyz;
    } else {
        out.normal = (obj.normal_mat * vec4<f32>(normal, 0.0)).xyz;
    }
    out.uv = uv;
    out.tangent = vec4<f32>((m * vec4<f32>(tangent.xyz, 0.0)).xyz, tangent.w);
    out.color = color;
    return out;
}

fn has(flag: u32) -> bool {
    return (obj.info.y & flag) != 0u;
}

fn mat_has(flag: u32) -> bool {
    return (mat.channels.w & flag) != 0u;
}

fn view_dir(world_pos: vec3<f32>) -> vec3<f32> {
    if g.eye.w > 0.5 {
        return g.cam_back.xyz;
    }
    return normalize(g.eye.xyz - world_pos);
}

// Blender-like studio setup: lights are fixed to the camera, not the world.
fn studio(n_view: vec3<f32>, base: vec3<f32>) -> vec3<f32> {
    let key = normalize(vec3<f32>(-0.45, 0.65, 0.62));
    let fill = normalize(vec3<f32>(0.75, -0.15, 0.45));
    let rim = normalize(vec3<f32>(0.1, 0.45, -0.9));
    let wrap_key = clamp((dot(n_view, key) + 0.25) / 1.25, 0.0, 1.0);
    let diffuse = 0.68 * wrap_key + 0.24 * max(dot(n_view, fill), 0.0) + 0.18 * max(dot(n_view, rim), 0.0);
    let ambient = mix(0.10, 0.24, n_view.y * 0.5 + 0.5);
    let h = normalize(key + vec3<f32>(0.0, 0.0, 1.0));
    let spec = pow(max(dot(n_view, h), 0.0), 48.0) * 0.18;
    return base * (ambient + diffuse) + vec3<f32>(spec);
}

fn matcap(n_view: vec3<f32>, base: vec3<f32>) -> vec3<f32> {
    let uv = vec2<f32>(n_view.x, -n_view.y) * 0.495 + vec2<f32>(0.5);
    return textureSampleLevel(matcap_tex, clamp_samp, uv, 0.0).rgb * base;
}

fn lit(n: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    let n_view = normalize((g.view * vec4<f32>(n, 0.0)).xyz);
    switch g.shading.y {
        case 1u: { return matcap(n_view, albedo); }
        case 2u: { return albedo; }
        default: { return studio(n_view, albedo); }
    }
}

fn hsv(h: f32, s: f32, v: f32) -> vec3<f32> {
    let k = vec3<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0);
    let p = abs(fract(vec3<f32>(h) + k) * 6.0 - 3.0);
    return v * mix(vec3<f32>(1.0), clamp(p - 1.0, vec3<f32>(0.0), vec3<f32>(1.0)), s);
}

// Colored grid like Blender's generated "Color Grid": spots stretching and seams at a glance.
fn uv_checker(uv: vec2<f32>) -> vec3<f32> {
    let cell = floor(uv * 8.0);
    let hue = fract((cell.x + cell.y * 3.0) / 8.0);
    let color = hsv(hue, 0.55, 0.9);
    let fine = floor(uv * 64.0);
    let check = (i32(fine.x) + i32(fine.y)) & 1;
    return srgb_to_linear(color * select(1.0, 0.72, check == 1));
}

fn pick(v: vec4<f32>, c: u32) -> f32 {
    var arr = v;
    return arr[min(c, 3u)];
}

// Displays data (non-color) values as-is, like Blender's "Non-Color" images.
fn raw(v: vec3<f32>) -> vec3<f32> {
    return srgb_to_linear(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)));
}

fn ibl_pbr(n: vec3<f32>, v: vec3<f32>, base: vec3<f32>, metallic: f32, rough: f32) -> PbrOut {
    let f0 = mix(vec3<f32>(0.04), base, metallic);
    let n_v = max(dot(n, v), 1e-4);
    let r = reflect(-v, n);
    let rot = -g.params.w;
    let lut = textureSampleLevel(brdf_lut, clamp_samp, vec2<f32>(n_v, 1.0 - rough), 0.0).rg;
    let spec = textureSampleLevel(env_spec, env_samp, equirect_uv(rotate_z(r, rot)), rough * g.env.z).rgb;
    let irr = textureSampleLevel(env_irr, env_samp, equirect_uv(rotate_z(n, rot)), 0.0).rgb;
    let fr = f0 + (max(vec3<f32>(1.0 - rough), f0) - f0) * pow(1.0 - n_v, 5.0);
    let kd = (vec3<f32>(1.0) - fr) * (1.0 - metallic);
    var out: PbrOut;
    out.diffuse = kd * base * irr * g.env.x;
    out.specular = spec * (f0 * lut.x + lut.y) * g.env.x;
    return out;
}

// Clearcoat specular from IBL and key light
fn clearcoat_pbr(n: vec3<f32>, v: vec3<f32>, p: vec3<f32>, screen_pos: vec2<f32>) -> vec3<f32> {
    let cc_factor = mat.alpha.z;
    if cc_factor <= 0.0 {
        return vec3<f32>(0.0);
    }
    let cc_rough = clamp(mat.alpha.w, 0.02, 1.0);
    let n_v = max(dot(n, v), 1e-4);
    let r = reflect(-v, n);
    let rot = -g.params.w;
    let cc_spec = textureSampleLevel(env_spec, env_samp, equirect_uv(rotate_z(r, rot)), cc_rough * g.env.z).rgb;
    let cc_f_ibl = 0.04 + 0.96 * pow(1.0 - n_v, 5.0);
    var spec = cc_factor * cc_spec * cc_f_ibl * g.env.x;

    let l = g.light.xyz;
    let nl = dot(n, l);
    if nl > 0.0 && g.light.w > 0.0 {
        let lit = shadow_at(p + n * g.shadow.w, 0, g.light0, 1.0 + g.shadow.z, screen_pos);
        if lit > 0.0 {
            let h = normalize(l + v);
            let nh = max(dot(n, h), 0.0);
            let a = cc_rough * cc_rough;
            let a2 = a * a;
            let d = nh * nh * (a2 - 1.0) + 1.0;
            let ndf = a2 / (PI * d * d);
            let k = (cc_rough + 1.0) * (cc_rough + 1.0) * 0.125;
            let vis = 0.25 / ((nl * (1.0 - k) + k) * (n_v * (1.0 - k) + k));
            let cc_f = 0.04 + 0.96 * pow(1.0 - max(dot(v, h), 0.0), 5.0);
            spec += cc_factor * (cc_f * ndf * vis) * nl * g.light.w * lit;
        }
    }
    return spec;
}

// --- Print finishes (Manufacturing workspace) ---
// Procedural and in world space, so they need no UVs: FDM layer lines run along world Z, the
// bed's normal, and follow the part when it's laid on another face.

struct Finish {
    n: vec3<f32>,
    albedo: vec3<f32>,
    metallic: f32,
    rough: f32,
    ao: f32,
};

fn hash13(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.zyx + 31.32);
    return fract((q.x + q.y) * q.z);
}

fn noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(hash13(i), hash13(i + vec3<f32>(1.0, 0.0, 0.0)), u.x);
    let b = mix(hash13(i + vec3<f32>(0.0, 1.0, 0.0)), hash13(i + vec3<f32>(1.0, 1.0, 0.0)), u.x);
    let c = mix(hash13(i + vec3<f32>(0.0, 0.0, 1.0)), hash13(i + vec3<f32>(1.0, 0.0, 1.0)), u.x);
    let d = mix(hash13(i + vec3<f32>(0.0, 1.0, 1.0)), hash13(i + vec3<f32>(1.0, 1.0, 1.0)), u.x);
    return mix(mix(a, b, u.y), mix(c, d, u.y), u.z);
}

// A surface map layer projected on the three world planes and blended by the surface's
// orientation (triplanar, so no UVs): the map's normal as a world-space offset, and its B and A
// channels. Gradients come from the caller, where control flow is still uniform; mipmaps keep
// the pattern from turning to noise in the distance.
struct Tri {
    offset: vec3<f32>,
    data: vec2<f32>,
};

fn triplanar(p: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, n: vec3<f32>, tile: f32, layer: i32) -> Tri {
    var w = pow(abs(n), vec3<f32>(4.0));
    w /= w.x + w.y + w.z;
    let k = 1.0 / tile;
    let sx = textureSampleGrad(surface_tex, surface_samp, p.zy * k, layer, dpx.zy * k, dpy.zy * k);
    let sy = textureSampleGrad(surface_tex, surface_samp, p.xz * k, layer, dpx.xz * k, dpy.xz * k);
    let sz = textureSampleGrad(surface_tex, surface_samp, p.xy * k, layer, dpx.xy * k, dpy.xy * k);
    let tx = sx.xy * 2.0 - 1.0;
    let ty = sy.xy * 2.0 - 1.0;
    let tz = sz.xy * 2.0 - 1.0;
    var out: Tri;
    out.offset = vec3<f32>(0.0, tx.y, tx.x) * w.x + vec3<f32>(ty.x, 0.0, ty.y) * w.y + vec3<f32>(tz.x, tz.y, 0.0) * w.z;
    out.data = sx.zw * w.x + sy.zw * w.y + sz.zw * w.z;
    return out;
}

// Tilts `n` by a world-space offset, keeping only the part along the surface.
fn bump(n: vec3<f32>, offset: vec3<f32>, k: f32) -> vec3<f32> {
    return normalize(n + (offset - n * dot(offset, n)) * k);
}

// The part material (Manufacturing): its sheen, FDM/resin layer lines, then surface wear.
// `rendered` is false in Solid, where only the relief counts. `pos_fw`, `dpx`, `dpy`: screen
// derivatives of the world position, taken where control flow is still uniform.
fn part_surface(p: vec3<f32>, pos_fw: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, n_in: vec3<f32>, base: vec3<f32>, metallic_in: f32, rough_in: f32) -> Finish {
    var out: Finish;
    out.albedo = base;
    out.metallic = metallic_in;
    out.rough = rough_in;
    out.ao = 1.0;
    var n = n_in;
    let id = u32(g.finish.x + 0.5);
    let metal = id == 7u;
    var lines = 0.0;
    var grain_k = 1.0;
    switch id {
        case 1u: { out.rough = 0.55; out.metallic = 0.0; lines = 0.55; }
        case 2u: { out.rough = 0.26; out.metallic = 0.55; lines = 0.35; }
        case 3u: { out.rough = 0.16; out.metallic = 0.0; lines = 0.5; }
        case 4u: { out.rough = 0.42; out.metallic = 0.0; lines = 0.5; }
        case 5u: { out.rough = 0.3; out.metallic = 0.0; lines = 0.12; }
        case 6u: { out.rough = 0.88; out.metallic = 0.0; grain_k = 2.2; }
        case 7u: {
            out.metallic = 1.0;
            switch u32(g.finish.z + 0.5) {
                case 0u: { out.rough = 0.07; }
                case 1u: { out.rough = 0.28; }
                case 2u: { out.rough = 0.3; }
                default: { out.rough = 0.55; grain_k = 1.8; }
            }
        }
        default: {}
    }
    // Layer lines along world Z (the bed's normal), on the walls only.
    let side = sqrt(max(1.0 - n_in.z * n_in.z, 0.0));
    if lines > 0.0 && g.finish.y > 0.0 && side > 0.0 {
        let t = p.z / g.finish.y;
        // Layers thinner than a pixel fade out instead of shimmering.
        let fade = 1.0 - smoothstep(0.3, 0.8, pos_fw.z / g.finish.y);
        if fade > 0.0 {
            // Each layer is a rounded bead: its normal tilts up above the bead's middle and down
            // below it, with a crease where two layers meet.
            let f = fract(t) - 0.5;
            let up = normalize(vec3<f32>(0.0, 0.0, 1.0) - n_in * n_in.z);
            n = normalize(n + up * (2.0 * f * lines * fade * side));
            out.ao *= 1.0 - 0.35 * pow(abs(f) * 2.0, 6.0) * side * fade * lines;
            out.albedo *= 1.0 + (hash13(vec3<f32>(floor(t), 7.0, 3.0)) - 0.5) * 0.06 * fade * min(lines * 2.0, 1.0);
        }
    }
    // Brushed metal: fine lines in the scratch map's A channel, stretched along one axis.
    if metal && u32(g.finish.z + 0.5) == 2u && g.surface.w > 0.0 {
        let b = triplanar(p, dpx, dpy, n_in, g.surface.w * 0.25, 1);
        out.rough = clamp(out.rough + (b.data.y - 0.25) * 0.35, 0.05, 1.0);
        out.albedo *= 1.0 + (b.data.y - 0.25) * 0.15;
    }
    // Grain and dust: relief, uneven gloss, faint specks.
    if g.surface.x > 0.0 {
        let k = g.surface.x * grain_k;
        let s = triplanar(p, dpx, dpy, n_in, g.surface.z, 0);
        n = bump(n, s.offset, 0.35 * k);
        out.rough = clamp(out.rough + (s.data.x - 0.5) * 0.45 * k + s.data.y * 0.25 * k, 0.03, 1.0);
        out.albedo *= 1.0 - s.data.y * 0.12 * k;
    }
    // Scratches: dents in the normal; they catch the light differently from the surface.
    if g.surface.y > 0.0 {
        let k = g.surface.y;
        let s = triplanar(p, dpx, dpy, n_in, g.surface.w, 1);
        let m = clamp(s.data.x * k * 1.5, 0.0, 1.0);
        n = bump(n, s.offset, 0.3 * k);
        if metal || out.metallic > 0.5 {
            out.rough = mix(out.rough, max(out.rough, 0.4), m);
        } else {
            // Plastic whitens where it's scratched.
            out.rough = mix(out.rough, 0.65, m);
            out.albedo = mix(out.albedo, out.albedo * 1.2 + vec3<f32>(0.04), m * 0.6);
        }
    }
    out.n = n;
    return out;
}

@fragment
fn fs_mesh(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Explicit gradients keep texture sampling legal after the non-uniform branches below.
    let dx = dpdx(in.uv);
    let dy = dpdy(in.uv);
    let pos_fw = fwidth(in.world_pos);
    let dpx = dpdx(in.world_pos);
    let dpy = dpdy(in.world_pos);

    if section_cuts(in.world_pos) {
        discard;
    }
    // Inside of a cut model: back faces seen through the section, drawn as a hatched cap.
    if !front && dot(g.section.xyz, g.section.xyz) > 0.0 {
        let stripe = fract((in.clip.x + in.clip.y) / 9.0) < 0.5;
        let cap = select(vec3<f32>(0.55, 0.16, 0.12), vec3<f32>(0.85, 0.3, 0.22), stripe);
        return vec4<f32>(srgb_to_linear(cap), 1.0);
    }

    // Backface culling is pipeline state: a facing-dependent discard here made some drivers
    // drop front-facing triangles as well.
    var n = normalize(in.normal);
    if !front {
        n = -n;
    }

    let mode = g.shading.x;
    let cmode = g.shading.w;
    // A per-object channel override wins over the global color mode.
    let override_pass = obj.info.w;
    let show_pass = override_pass > 0u || (mode == 1u && cmode == 3u);
    let tex_pass = select(g.extra.x, override_pass - 1u, override_pass > 0u);
    // Manufacturing: plain plastic, the file's maps, vertex colors and alpha are left out.
    let neutral = g.display.y > 0.5;
    let material_inputs = (mode == 2u || show_pass) && !neutral;
    let textured = material_inputs && has(HAS_UV);

    var base = mat.base_color;
    if neutral {
        base = vec4<f32>(g.finish_color.rgb, 1.0);
    }
    if textured && mat_has(1u) {
        base *= textureSampleGrad(base_tex, mat_samp, in.uv, dx, dy);
    }
    if material_inputs && has(HAS_COLOR) {
        base *= in.color;
    }
    if material_inputs && mat.alpha.y == 1.0 && base.a < mat.alpha.x {
        discard;
    }

    if textured && has(HAS_TANGENT) && mat_has(2u) {
        var tn = textureSampleGrad(normal_tex, mat_samp, in.uv, dx, dy).xyz * 2.0 - 1.0;
        tn = vec3<f32>(tn.xy * mat.pbr.z, tn.z);
        let t = normalize(in.tangent.xyz - n * dot(n, in.tangent.xyz));
        let b = cross(n, t) * in.tangent.w;
        n = normalize(t * tn.x + b * tn.y + n * tn.z);
    }

    var metallic = select(mat.pbr.x, 0.0, neutral);
    var rough = select(mat.pbr.y, 0.5, neutral);
    var ao = 1.0;
    var emissive = select(mat.emissive.rgb, vec3<f32>(0.0), neutral);
    if textured {
        if mat_has(4u) {
            metallic *= pick(textureSampleGrad(metallic_tex, mat_samp, in.uv, dx, dy), mat.channels.x);
        }
        if mat_has(8u) {
            rough *= pick(textureSampleGrad(roughness_tex, mat_samp, in.uv, dx, dy), mat.channels.y);
        }
        if mat_has(16u) {
            let o = pick(textureSampleGrad(occlusion_tex, mat_samp, in.uv, dx, dy), mat.channels.z);
            ao = mix(1.0, o, mat.pbr.w);
        }
        if mat_has(32u) {
            emissive *= textureSampleGrad(emissive_tex, mat_samp, in.uv, dx, dy).rgb;
        }
    }

    var color: vec3<f32>;
    var alpha = base.a;
    if show_pass {
        switch tex_pass {
            case 1u: { color = raw(vec3<f32>(rough)); }
            case 2u: { color = raw(vec3<f32>(metallic)); }
            case 3u: {
                if textured && mat_has(2u) {
                    color = raw(textureSampleGrad(normal_tex, mat_samp, in.uv, dx, dy).rgb);
                } else {
                    color = raw(vec3<f32>(0.5, 0.5, 1.0));
                }
            }
            case 4u: { color = raw(vec3<f32>(ao)); }
            case 5u: { color = emissive; }
            case 6u: { color = raw(vec3<f32>(base.a)); alpha = 1.0; }
            case 7u: {
                if has(HAS_UV) {
                    color = lit(n, uv_checker(in.uv));
                } else {
                    color = vec3<f32>(1.0, 0.0, 1.0);
                }
            }
            default: { color = lit(n, base.rgb); }
        }
    } else if mode == 2u {
        let v = view_dir(in.world_pos);
        var albedo = base.rgb;
        // Manufacturing: the part material and its wear.
        if g.finish.w > 0.0 {
            let f = part_surface(in.world_pos, pos_fw, dpx, dpy, n, base.rgb, metallic, rough);
            n = f.n;
            albedo = f.albedo;
            metallic = f.metallic;
            rough = f.rough;
            ao *= f.ao;
        }
        let ibl_res = ibl_pbr(n, v, albedo, clamp(metallic, 0.0, 1.0), clamp(rough, 0.02, 1.0));
        var total_diffuse = ibl_res.diffuse * ao;
        var total_specular = ibl_res.specular * ao;
        if g.light.w > 0.0 {
            let dir_res = key_light_pbr(in.world_pos, n, v, albedo, clamp(metallic, 0.0, 1.0), clamp(rough, 0.03, 1.0), in.clip.xy);
            total_diffuse += dir_res.diffuse * ao;
            total_specular += dir_res.specular * ao;
        }

        // Clearcoat specular layer (automotive paint, rims, carbon, lacquer)
        let cc_spec = clearcoat_pbr(n, v, in.world_pos, in.clip.xy);
        let n_v = max(dot(n, v), 1e-4);
        let cc_f = 0.04 + 0.96 * pow(1.0 - n_v, 5.0);
        let cc_factor = mat.alpha.z;
        let cc_atten = 1.0 - cc_factor * cc_f;
        total_diffuse *= cc_atten;
        total_specular = total_specular * cc_atten + cc_spec;

        // Transparent materials & Transmission (Glass, Windows, Headlights)
        let transmission = mat.extra.x;
        let is_transparent = (mat.alpha.y == 2.0 && base.a < 0.999) || transmission > 0.0;
        if is_transparent {
            let trans_amount = clamp(max(transmission, 1.0 - base.a), 0.0, 1.0);
            total_diffuse *= (1.0 - trans_amount);

            let fresnel = pow(1.0 - n_v, 5.0);
            let base_op = select(base.a, (1.0 - transmission) * base.a + 0.1, transmission > 0.0);
            alpha = clamp(base_op + (1.0 - base_op) * fresnel * (1.0 - rough), 0.02, 1.0);

            // In alpha blending, fragment RGB is multiplied by alpha.
            // Preserving specular through the blend so glass reflections gleam with full intensity:
            color = total_diffuse + (total_specular / max(alpha, 0.08)) + emissive;
        } else {
            color = total_diffuse + total_specular + emissive;
        }
        color = view_transform(color * g.params.y, g.extra.y);
    } else if cmode == 4u {
        let c = select(vec3<f32>(0.8), in.color.rgb, has(HAS_COLOR));
        color = lit(n, c);
    } else {
        // Solid: the wear's relief only, at half strength (no gloss to vary).
        if g.finish.w > 0.0 && (g.surface.x > 0.0 || g.surface.y > 0.0) {
            if g.surface.x > 0.0 {
                n = bump(n, triplanar(in.world_pos, dpx, dpy, n, g.surface.z, 0).offset, 0.17 * g.surface.x);
            }
            if g.surface.y > 0.0 {
                n = bump(n, triplanar(in.world_pos, dpx, dpy, n, g.surface.w, 1).offset, 0.15 * g.surface.y);
            }
        }
        color = lit(n, obj.color.rgb);
        alpha = obj.color.a;
    }
    // Face orientation overlay (Blender's colors): front blue, back red.
    if g.normals.y > 0.5 {
        let side = select(vec3<f32>(0.64, 0.02, 0.02), vec3<f32>(0.03, 0.12, 0.72), front);
        color = mix(color, side * (0.35 + 0.65 * max(dot(normalize((g.view * vec4<f32>(n, 0.0)).xyz), vec3<f32>(0.0, 0.0, 1.0)), 0.0)), 0.7);
    }
    // params.x is 1 for opaque passes and the x-ray alpha otherwise.
    return vec4<f32>(color, g.params.x * alpha);
}

// --- Background (Rendered mode world) ---

struct FullOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: FullOut;
    out.ndc = uv * 2.0 - 1.0;
    out.clip = vec4<f32>(out.ndc, 0.0, 1.0);
    return out;
}

@fragment
fn fs_background(in: FullOut) -> @location(0) vec4<f32> {
    // Transparent preview checkerboard (extra.z == 3u)
    if g.extra.z == 3u {
        let check_size = 16.0;
        let c = floor(in.clip.xy / check_size);
        let check = (i32(c.x) + i32(c.y)) & 1;
        let col = select(vec3<f32>(0.13, 0.13, 0.14), vec3<f32>(0.18, 0.18, 0.20), check == 1);
        return vec4<f32>(srgb_to_linear(col), 1.0);
    }
    // Studio backdrop: light gray, a little brighter towards the top, like a photo sweep.
    if g.extra.z == 2u {
        let t = clamp(in.ndc.y * 0.5 + 0.5, 0.0, 1.0);
        return vec4<f32>(srgb_to_linear(mix(vec3<f32>(0.80), vec3<f32>(0.93), t)), 1.0);
    }
    let p0 = g.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let p1 = g.inv_view_proj * vec4<f32>(in.ndc, 0.5, 1.0);
    let d = normalize(p1.xyz / p1.w - p0.xyz / p0.w);
    let lod = g.env.y * 9.0;
    let c = textureSampleLevel(env_src, env_samp, equirect_uv(rotate_z(d, -g.params.w)), lod).rgb;
    return vec4<f32>(view_transform(c * g.env.x * g.params.y, g.extra.y), 1.0);
}

// --- Wireframe ---

struct WireOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
};

@vertex
fn vs_wire(
    @location(0) pos: vec3<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
) -> WireOut {
    var out: WireOut;
    let wp = vertex_matrix(joints, weights) * vec4<f32>(pos, 1.0);
    out.world_pos = wp.xyz;
    var clip = g.view_proj * wp;
    // Nudge towards the camera (reverse-Z) so edges win the depth test against their own faces.
    clip.z = clip.z * 1.0008 + 1e-7 * clip.w;
    out.clip = clip;
    return out;
}

@fragment
fn fs_wire(in: WireOut) -> @location(0) vec4<f32> {
    if section_cuts(in.world_pos) {
        discard;
    }
    var rgb = g.wire_color.rgb;
    // In wireframe mode the random color mode tints wires per object, like Blender.
    if g.shading.x == 0u && g.shading.w == 2u {
        rgb = obj.color.rgb;
    }
    var a = g.wire_color.a * g.params.z;
    if has(ACTIVE) {
        rgb = g.active_color.rgb;
        a = 1.0;
    } else if has(SELECTED) {
        rgb = g.selected_color.rgb;
        a = 1.0;
    }
    return vec4<f32>(rgb, a);
}

// --- Mesh analysis markers (see qa.rs) ---
// One instance per marker, drawn as a screen-space quad so edges are thick and vertices are
// squares at any zoom. Kind 0: non-manifold edge, 1: open edge, 2: overlapping vertex.

struct MarkerOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) kind: u32,
    @location(1) local: vec2<f32>,
};

@vertex
fn vs_marker(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec3<f32>,
    @location(1) kind: u32,
    @location(2) b: vec3<f32>,
    @location(3) ja: vec4<u32>,
    @location(4) jb: vec4<u32>,
    @location(5) wa: vec4<f32>,
    @location(6) wb: vec4<f32>,
) -> MarkerOut {
    var out: MarkerOut;
    out.kind = kind;
    out.local = vec2<f32>(0.0);
    let shown = (kind == 0u && g.markers.x != 0u) || (kind == 1u && g.markers.y != 0u)
        || (kind == 2u && g.markers.z != 0u);
    let wa4 = vertex_matrix(ja, wa) * vec4<f32>(a, 1.0);
    let wb4 = vertex_matrix(jb, wb) * vec4<f32>(b, 1.0);
    let ca = g.view_proj * wa4;
    let cb = g.view_proj * wb4;
    if !shown || ca.w <= 0.0 || cb.w <= 0.0 || section_cuts(wa4.xyz) || section_cuts(wb4.xyz) {
        // Every corner at the same point outside the view: nothing is drawn.
        out.clip = vec4<f32>(2.0, 2.0, 0.5, 1.0);
        return out;
    }
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    let ndc_per_px = 2.0 * g.viewport.zw;
    var clip: vec4<f32>;
    if kind == 2u {
        let side = vec2<f32>(c.x * 2.0 - 1.0, c.y);
        clip = ca + vec4<f32>(side * 4.0 * ndc_per_px * ca.w, 0.0, 0.0);
        out.local = side;
    } else {
        var dir = (cb.xy / cb.w - ca.xy / ca.w) * g.viewport.xy;
        dir = select(normalize(dir), vec2<f32>(1.0, 0.0), length(dir) < 1e-4);
        let normal = vec2<f32>(-dir.y, dir.x);
        let half = 1.5;
        let base = select(ca, cb, c.x > 0.5);
        let offset = (normal * c.y + dir * (c.x * 2.0 - 1.0)) * half * ndc_per_px;
        clip = base + vec4<f32>(offset * base.w, 0.0, 0.0);
        out.local = vec2<f32>(0.0, c.y);
    }
    clip.z = clip.z * 1.0008 + 1e-7 * clip.w;
    out.clip = clip;
    return out;
}

fn marker_color(kind: u32) -> vec3<f32> {
    switch kind {
        case 0u: { return srgb_to_linear(vec3<f32>(1.0, 0.18, 0.47)); }
        case 1u: { return srgb_to_linear(vec3<f32>(1.0, 0.76, 0.2)); }
        default: { return srgb_to_linear(vec3<f32>(0.2, 0.86, 1.0)); }
    }
}

fn marker(in: MarkerOut, alpha: f32) -> vec4<f32> {
    var rgb = marker_color(in.kind);
    // Vertices get a dark rim so they read on any surface color.
    if in.kind == 2u && max(abs(in.local.x), abs(in.local.y)) > 0.62 {
        rgb = vec3<f32>(0.0);
    }
    return vec4<f32>(rgb, alpha);
}

@fragment
fn fs_marker(in: MarkerOut) -> @location(0) vec4<f32> {
    return marker(in, 1.0);
}

// Markers behind surfaces stay faintly visible, so hidden problems aren't missed.
@fragment
fn fs_marker_hidden(in: MarkerOut) -> @location(0) vec4<f32> {
    return marker(in, 0.28);
}

// --- Normal lines: one instance per vertex, two line ends ---

@vertex
fn vs_normal(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
) -> WireOut {
    let m = vertex_matrix(joints, weights);
    let wp = m * vec4<f32>(pos, 1.0);
    var wn: vec3<f32>;
    if (obj.info.y & SKINNED) != 0u {
        wn = (m * vec4<f32>(normal, 0.0)).xyz;
    } else {
        wn = (obj.normal_mat * vec4<f32>(normal, 0.0)).xyz;
    }
    let len = length(wn);
    let dir = select(vec3<f32>(0.0), wn / len, len > 1e-8);
    let world = wp.xyz + dir * g.normals.x * f32(vi);
    var clip = g.view_proj * vec4<f32>(world, 1.0);
    clip.z = clip.z * 1.0008 + 1e-7 * clip.w;
    return WireOut(clip, world);
}

@fragment
fn fs_normal(in: WireOut) -> @location(0) vec4<f32> {
    if section_cuts(in.world_pos) {
        discard;
    }
    return vec4<f32>(srgb_to_linear(vec3<f32>(0.35, 0.7, 1.0)), 0.9);
}

// --- Object ids (picking and outlines) ---

@vertex
fn vs_id(
    @location(0) pos: vec3<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
) -> WireOut {
    let wp = vertex_matrix(joints, weights) * vec4<f32>(pos, 1.0);
    return WireOut(g.view_proj * wp, wp.xyz);
}

struct IdOut {
    @location(0) id: u32,
    // Depth for picking a 3D point (the depth buffer itself can't be read one texel at a time).
    @location(1) depth: f32,
};

@fragment
fn fs_id(in: WireOut) -> IdOut {
    // Cut parts can't be picked or outlined.
    if section_cuts(in.world_pos) {
        discard;
    }
    return IdOut(obj.info.x, in.clip.z);
}

// --- Shadow floor (Rendered mode) ---
// A disc under the model that only darkens what's behind it: the key light's shadow and soft
// contact shading where the model is close above. Fades out towards its rim.

struct FloorOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
};

@vertex
fn vs_floor(@builtin(vertex_index) i: u32) -> FloorOut {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let p = vec3<f32>(g.floor.xy + corners[i] * g.floor.z, g.shadow.y);
    var out: FloorOut;
    out.clip = g.view_proj * vec4<f32>(p, 1.0);
    out.world_pos = p;
    return out;
}

@fragment
fn fs_floor(in: FloorOut) -> @location(0) vec4<f32> {
    let p = in.world_pos;
    let r = length(p.xy - g.floor.xy) / g.floor.z;
    let fade = 1.0 - smoothstep(0.45, 1.0, r);
    var dark = 0.0;
    if g.light.w > 0.0 && g.light.z > 0.0 {
        dark += (1.0 - shadow_at(p + vec3<f32>(0.0, 0.0, g.shadow.w), 0, g.light0, 1.5 + g.shadow.z * 1.5, in.clip.xy)) * 0.55;
    }
    // Contact: how much of the model is right above, blurred wide.
    let contact = 1.0 - shadow_at(p + vec3<f32>(0.0, 0.0, g.shadow.w), 1, g.light1, 4.0 + g.shadow.z * 2.0, in.clip.xy + vec2<f32>(13.37, 37.13));
    dark += contact * 0.45;
    return vec4<f32>(0.0, 0.0, 0.0, clamp(dark, 0.0, 0.85) * fade);
}
