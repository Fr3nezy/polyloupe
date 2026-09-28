// Prepended with common.wgsl. Infinite floor grid drawn from a fullscreen triangle.

@group(0) @binding(0) var<uniform> g: Globals;

struct GridOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_grid(@builtin(vertex_index) i: u32) -> GridOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    let ndc = uv * 2.0 - 1.0;
    var out: GridOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

struct GridFrag {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

fn unproject(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let p = g.inv_view_proj * vec4<f32>(ndc, z, 1.0);
    return p.xyz / p.w;
}

// Anti-aliased line coverage for a grid of the given cell size.
fn lines(coord: vec2<f32>, cell: f32) -> f32 {
    let c = coord / cell;
    let w = max(fwidth(c), vec2<f32>(1e-6));
    let d = abs(fract(c - 0.5) - 0.5) / w;
    return 1.0 - min(min(d.x, d.y), 1.0);
}

fn axis_line(v: f32) -> f32 {
    let w = max(fwidth(v), 1e-6);
    return 1.0 - min(abs(v) / (w * 1.2), 1.0);
}

@fragment
fn fs_grid(in: GridOut) -> GridFrag {
    // Two points on the pixel's view ray (reverse-Z: 1 = near plane).
    let p0 = unproject(in.ndc, 1.0);
    let p1 = unproject(in.ndc, 0.5);
    let dir = normalize(p1 - p0);

    let axis = u32(g.grid.z);
    var denom: f32;
    var start: f32;
    switch axis {
        case 0u: { denom = dir.x; start = p0.x; }
        case 1u: { denom = dir.y; start = p0.y; }
        default: { denom = dir.z; start = p0.z; }
    }
    if abs(denom) < 1e-7 {
        discard;
    }
    let t = -start / denom;
    if t <= 0.0 {
        discard;
    }
    let hit = p0 + dir * t;

    // In-plane coordinates, and the two world axes that lie on the plane.
    var uv: vec2<f32>;
    var col_u: vec3<f32>;
    var col_v: vec3<f32>;
    let red = vec3<f32>(0.80, 0.16, 0.20);
    let green = vec3<f32>(0.36, 0.62, 0.08);
    let blue = vec3<f32>(0.14, 0.38, 0.80);
    // Colors follow the displayed axis names (Y up shows world Y as Z, world Z as Y).
    let y_up = g.display.x > 0.5;
    let world_y = select(green, blue, y_up);
    let world_z = select(blue, green, y_up);
    switch axis {
        case 0u: { uv = hit.yz; col_u = world_z; col_v = world_y; }
        case 1u: { uv = hit.xz; col_u = world_z; col_v = red; }
        default: { uv = hit.xy; col_u = world_y; col_v = red; }
    }

    let cell = g.grid.x;
    let fade = g.grid.y;
    let minor = lines(uv, cell) * (1.0 - fade);
    let mid = lines(uv, cell * 10.0);
    let major = lines(uv, cell * 100.0);
    var alpha = max(max(minor * 0.06, mid * 0.11), major * 0.18);
    var rgb = vec3<f32>(1.0);

    if g.grid.w > 0.5 {
        // uv.y == 0 is the line along the first in-plane axis, and vice versa.
        let a_u = axis_line(uv.x);
        let a_v = axis_line(uv.y);
        if a_v > 0.0 {
            rgb = mix(rgb, col_v, a_v);
            alpha = max(alpha, a_v * 0.85);
        }
        if a_u > 0.0 {
            rgb = mix(rgb, col_u, a_u);
            alpha = max(alpha, a_u * 0.85);
        }
    }

    // Fade with distance from the eye and at grazing angles to avoid moiré.
    // Measured from the eye: in orthographic views the near plane can sit far behind it.
    let dist = length(hit - g.eye.xyz);
    let reach = max(g.grid.x * 400.0, 1e-3);
    alpha *= 1.0 - smoothstep(reach * 0.25, reach, dist);
    alpha *= smoothstep(0.0, 0.12, abs(denom));

    if alpha <= 0.002 {
        discard;
    }
    var out: GridFrag;
    let clip = g.view_proj * vec4<f32>(hit, 1.0);
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    out.color = vec4<f32>(rgb, alpha);
    return out;
}
