// Prepended with common.wgsl. Object and selection outlines from the object id buffer.

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var ids: texture_2d<u32>;
// Per object: 0 = unselected, 1 = selected, 2 = active.
@group(0) @binding(2) var<storage, read> selection: array<u32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn state(id: u32) -> u32 {
    if id == 0u {
        return 0u;
    }
    return selection[id - 1u];
}

@fragment
fn fs_outline(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(ids));
    let p = vec2<i32>(frag.xy);
    let center = textureLoad(ids, p, 0).r;
    var sel = state(center);
    var edge = false;
    var sel_edge = 0u;
    let offsets = array<vec2<i32>, 8>(
        vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1),
        vec2<i32>(1, 1), vec2<i32>(-1, -1), vec2<i32>(1, -1), vec2<i32>(-1, 1),
    );
    for (var i = 0; i < 8; i++) {
        let q = clamp(p + offsets[i], vec2<i32>(0), size - 1);
        let other = textureLoad(ids, q, 0).r;
        if other != center {
            edge = true;
            sel_edge = max(sel_edge, max(sel, state(other)));
        }
    }
    if !edge {
        discard;
    }
    if sel_edge == 2u {
        return g.active_color;
    }
    if sel_edge == 1u {
        return g.selected_color;
    }
    if g.extra.w == 1u {
        return vec4<f32>(0.0, 0.0, 0.0, 0.55);
    }
    discard;
    return vec4<f32>(0.0);
}
