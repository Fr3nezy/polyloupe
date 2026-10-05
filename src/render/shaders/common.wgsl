// Shared declarations, prepended to every viewport shader.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // xyz: eye position, w: 1 when orthographic.
    eye: vec4<f32>,
    // xyz: unit vector from target towards the eye.
    cam_back: vec4<f32>,
    // x: shading mode (0 wire, 1 solid, 2 rendered), y: lighting (0 studio, 1 matcap, 2 flat),
    // z: backface culling, w: color mode (0 material, 1 single, 2 random, 3 texture, 4 attribute).
    shading: vec4<u32>,
    // x: texture pass, y: view transform (0 standard, 1 AgX), z: env background, w: object outline.
    extra: vec4<u32>,
    // x: x-ray alpha (1 when opaque), y: exposure, z: wire opacity, w: env rotation (radians).
    params: vec4<f32>,
    // x: env strength, y: background blur (0..1), z: specular max lod, w: unused.
    env: vec4<f32>,
    // x: grid cell size, y: level fade, z: plane normal axis (0 x, 1 y, 2 z), w: axes visible.
    grid: vec4<f32>,
    wire_color: vec4<f32>,
    selected_color: vec4<f32>,
    active_color: vec4<f32>,
    // xy: viewport size in pixels, zw: 1 / size.
    viewport: vec4<f32>,
    // Mesh analysis markers shown: x non-manifold edges, y open edges, z overlapping vertices.
    markers: vec4<u32>,
    // Section plane: xyz normal (zero when off), w offset. Points with dot(n, p) > w are cut.
    section: vec4<f32>,
    // x: normal line length (world units, 0 = off), y: 1 for the face orientation overlay.
    normals: vec4<f32>,
    // x: 1 when the interface calls Y up: world Y lines are drawn blue (Z) and world Z green (Y).
    // y: 1 for the neutral plastic material (Manufacturing): the file's materials are ignored.
    // z: strength of the surface imperfection on the neutral plastic (0 = none).
    display: vec4<f32>,
    // Print finish (Rendered mode): x finish id (0 off), y layer height in world units,
    // w world units per millimeter.
    finish: vec4<f32>,
    // Neutral plastic color, linear RGB.
    finish_color: vec4<f32>,
};

fn section_cuts(world_pos: vec3<f32>) -> bool {
    return dot(g.section.xyz, g.section.xyz) > 0.0 && dot(g.section.xyz, world_pos) > g.section.w;
}

const PI: f32 = 3.14159265;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn rotate_z(d: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(c * d.x - s * d.y, s * d.x + c * d.y, d.z);
}

fn equirect_uv(d: vec3<f32>) -> vec2<f32> {
    // Same layout as Blender (Cycles/EEVEE): u grows clockwise seen from above.
    return vec2<f32>(0.5 - atan2(d.y, d.x) / (2.0 * PI), acos(clamp(d.z, -1.0, 1.0)) / PI);
}

// Minimal AgX (Blender's default view transform since 4.0), fitted polynomial version.
fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(color: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104),
    );
    let outset = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116),
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * max(color, vec3<f32>(1e-10));
    v = clamp(log2(v), vec3<f32>(min_ev), vec3<f32>(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    v = agx_contrast(v);
    v = outset * v;
    // Back to linear: the sRGB render target re-encodes on write.
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn view_transform(color: vec3<f32>, mode: u32) -> vec3<f32> {
    if mode == 1u {
        return agx(color);
    }
    return clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
}
