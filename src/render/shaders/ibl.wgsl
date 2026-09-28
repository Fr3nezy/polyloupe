// Image-based lighting preprocessing: equirect mip chain, GGX prefiltered specular,
// diffuse irradiance and the split-sum BRDF lookup table.

const PI: f32 = 3.14159265;

struct Params {
    // x: roughness, y: source width, z: source height, w: source mip count.
    v: vec4<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct FullOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: FullOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn dir_from_uv(uv: vec2<f32>) -> vec3<f32> {
    let phi = (0.5 - uv.x) * 2.0 * PI;
    let theta = uv.y * PI;
    return vec3<f32>(sin(theta) * cos(phi), sin(theta) * sin(phi), cos(theta));
}

fn uv_from_dir(d: vec3<f32>) -> vec2<f32> {
    // Same layout as Blender (Cycles/EEVEE): u grows clockwise seen from above.
    return vec2<f32>(0.5 - atan2(d.y, d.x) / (2.0 * PI), acos(clamp(d.z, -1.0, 1.0)) / PI);
}

fn radical_inverse(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}

fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), radical_inverse(i));
}

fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.999);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    return mat3x3<f32>(t, b, n);
}

fn importance_ggx(xi: vec2<f32>, a: f32) -> vec3<f32> {
    let phi = 2.0 * PI * xi.x;
    let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_t = sqrt(1.0 - cos_t * cos_t);
    return vec3<f32>(cos(phi) * sin_t, sin(phi) * sin_t, cos_t);
}

fn d_ggx(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

@fragment
fn fs_downsample(in: FullOut) -> @location(0) vec4<f32> {
    // Source is bound as the single previous mip; linear filtering averages 2x2 texels.
    return textureSampleLevel(src, samp, in.uv, 0.0);
}

@fragment
fn fs_copy(in: FullOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, samp, in.uv, 0.0);
}

const SPEC_SAMPLES: u32 = 96u;

@fragment
fn fs_prefilter(in: FullOut) -> @location(0) vec4<f32> {
    let n = dir_from_uv(in.uv);
    let rough = params.v.x;
    let a = rough * rough;
    let tbn = basis(n);
    // Solid angle of one source texel (equirect average).
    let texel_sa = 4.0 * PI / (params.v.y * params.v.z);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0u; i < SPEC_SAMPLES; i++) {
        let h = tbn * importance_ggx(hammersley(i, SPEC_SAMPLES), a);
        let l = normalize(2.0 * dot(n, h) * h - n);
        let n_l = dot(n, l);
        if n_l > 0.0 {
            // Filtered importance sampling: fetch from a blurrier mip where samples are sparse.
            let n_h = max(dot(n, h), 0.0);
            let pdf = d_ggx(n_h, a) * 0.25 + 1e-4;
            let sample_sa = 1.0 / (f32(SPEC_SAMPLES) * pdf);
            let lod = clamp(0.5 * log2(sample_sa / texel_sa) + 1.0, 0.0, params.v.w - 1.0);
            sum += textureSampleLevel(src, samp, uv_from_dir(l), lod).rgb * n_l;
            weight += n_l;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-4), 1.0);
}

const IRR_SAMPLES: u32 = 512u;

@fragment
fn fs_irradiance(in: FullOut) -> @location(0) vec4<f32> {
    let n = dir_from_uv(in.uv);
    let tbn = basis(n);
    // A low mip already averages the environment; cosine-weighted samples do the rest.
    let lod = max(params.v.w - 6.0, 0.0);
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < IRR_SAMPLES; i++) {
        let xi = hammersley(i, IRR_SAMPLES);
        let phi = 2.0 * PI * xi.x;
        let r = sqrt(xi.y);
        let local = vec3<f32>(cos(phi) * r, sin(phi) * r, sqrt(max(1.0 - xi.y, 0.0)));
        sum += textureSampleLevel(src, samp, uv_from_dir(tbn * local), lod).rgb;
    }
    return vec4<f32>(sum / f32(IRR_SAMPLES), 1.0);
}

const BRDF_SAMPLES: u32 = 256u;

@fragment
fn fs_brdf(in: FullOut) -> @location(0) vec4<f32> {
    let n_v = max(in.uv.x, 1e-3);
    let rough = 1.0 - in.uv.y;
    let a = rough * rough;
    let v = vec3<f32>(sqrt(1.0 - n_v * n_v), 0.0, n_v);
    var scale = 0.0;
    var bias = 0.0;
    for (var i = 0u; i < BRDF_SAMPLES; i++) {
        let h = importance_ggx(hammersley(i, BRDF_SAMPLES), a);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let n_l = max(l.z, 0.0);
        let n_h = max(h.z, 0.0);
        let v_h = max(dot(v, h), 0.0);
        if n_l > 0.0 {
            let k = a * 0.5;
            let g = (n_v / (n_v * (1.0 - k) + k)) * (n_l / (n_l * (1.0 - k) + k));
            let g_vis = g * v_h / (n_h * n_v + 1e-5);
            let fc = pow(1.0 - v_h, 5.0);
            scale += (1.0 - fc) * g_vis;
            bias += fc * g_vis;
        }
    }
    return vec4<f32>(scale / f32(BRDF_SAMPLES), bias / f32(BRDF_SAMPLES), 0.0, 1.0);
}
