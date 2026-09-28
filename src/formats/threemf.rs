//! 3MF (3D Manufacturing Format): a zip with an XML model. Reads mesh objects, components
//! (objects made of other objects), build items with their transforms, base materials and
//! color groups (per-triangle colors become materials). Units come from the model
//! (millimeters by default) and are converted to meters. 3MF is Z up, like the viewer.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use glam::{Mat4, Vec3, Vec4};

use crate::scene::{Material, Mesh, MeshData, MeshRig, Scene, Units, unique_edges};

struct Object {
    name: String,
    vertices: Vec<[f32; 3]>,
    /// (v1, v2, v3, material key)
    triangles: Vec<([u32; 3], Option<(u32, u32)>)>,
    /// Object-level default material (pid, pindex).
    default: Option<(u32, u32)>,
    components: Vec<(u32, Mat4)>,
}

pub fn load(path: &Path) -> Result<Scene, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("Not a valid 3MF (zip) file: {e}"))?;
    // The start part is named in _rels/.rels; 3D/3dmodel.model by convention.
    let mut model_path = "3D/3dmodel.model".to_string();
    if let Ok(mut rels) = zip.by_name("_rels/.rels") {
        let mut text = String::new();
        if rels.read_to_string(&mut text).is_ok() {
            if let Ok(doc) = roxmltree::Document::parse(&text) {
                if let Some(target) = doc
                    .descendants()
                    .filter(|n| n.has_tag_name("Relationship"))
                    .find(|n| n.attribute("Type").is_some_and(|t| t.ends_with("/3dmodel")))
                    .and_then(|n| n.attribute("Target"))
                {
                    model_path = target.trim_start_matches('/').to_string();
                }
            }
        }
    }
    let mut text = String::new();
    zip.by_name(&model_path)
        .map_err(|_| format!("The 3MF file has no model part ({model_path})"))?
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    parse(&text)
}

fn transform(attr: Option<&str>) -> Mat4 {
    let Some(text) = attr else { return Mat4::IDENTITY };
    let m: Vec<f32> = text.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    if m.len() != 12 {
        return Mat4::IDENTITY;
    }
    // Row-vector convention: p' = p * M, rows m00..m02, m10..m12, m20..m22, m30..m32.
    Mat4::from_cols(
        Vec4::new(m[0], m[1], m[2], 0.0),
        Vec4::new(m[3], m[4], m[5], 0.0),
        Vec4::new(m[6], m[7], m[8], 0.0),
        Vec4::new(m[9], m[10], m[11], 1.0),
    )
}

fn color(text: &str) -> Option<[f32; 4]> {
    let hex = text.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
    let a = if hex.len() >= 8 { byte(6)? } else { 255 };
    let lin = crate::render::srgb_to_linear([r, g, b]);
    Some([lin[0], lin[1], lin[2], a as f32 / 255.0])
}

pub fn parse(text: &str) -> Result<Scene, String> {
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("Invalid 3MF model XML: {e}"))?;
    let model = doc.root_element();
    let unit = match model.attribute("unit").unwrap_or("millimeter") {
        "micron" => 1e-6,
        "centimeter" => 1e-2,
        "inch" => 0.0254,
        "foot" => 0.3048,
        "meter" => 1.0,
        _ => 1e-3,
    };

    // Material groups: (group id, index) -> color.
    let mut colors: HashMap<(u32, u32), ([f32; 4], String)> = HashMap::new();
    for group in model.descendants().filter(|n| n.has_tag_name("basematerials") || n.has_tag_name("colorgroup")) {
        let Some(id) = group.attribute("id").and_then(|v| v.parse().ok()) else { continue };
        for (i, item) in group.children().filter(|n| n.is_element()).enumerate() {
            let c = item.attribute("displaycolor").or(item.attribute("color")).and_then(color).unwrap_or([0.8, 0.8, 0.8, 1.0]);
            let name = item.attribute("name").map_or_else(|| format!("Color {}", i + 1), str::to_string);
            colors.insert((id, i as u32), (c, name));
        }
    }

    let mut objects: HashMap<u32, Object> = HashMap::new();
    for node in model.descendants().filter(|n| n.has_tag_name("object")) {
        let Some(id) = node.attribute("id").and_then(|v| v.parse().ok()) else { continue };
        let num = |n: &roxmltree::Node, a: &str| n.attribute(a).and_then(|v| v.parse::<u32>().ok());
        let default = num(&node, "pid").map(|p| (p, num(&node, "pindex").unwrap_or(0)));
        let mut object = Object {
            name: node.attribute("name").map_or_else(|| format!("Object {id}"), str::to_string),
            vertices: Vec::new(),
            triangles: Vec::new(),
            default,
            components: Vec::new(),
        };
        for child in node.descendants() {
            if child.has_tag_name("vertex") {
                let f = |a| child.attribute(a).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                object.vertices.push([f("x"), f("y"), f("z")]);
            } else if child.has_tag_name("triangle") {
                let (Some(a), Some(b), Some(c)) = (num(&child, "v1"), num(&child, "v2"), num(&child, "v3")) else { continue };
                let key = num(&child, "pid").map(|p| (p, num(&child, "p1").unwrap_or(0)));
                object.triangles.push(([a, b, c], key));
            } else if child.has_tag_name("component") {
                if let Some(target) = num(&child, "objectid") {
                    object.components.push((target, transform(child.attribute("transform"))));
                }
            }
        }
        objects.insert(id, object);
    }

    // Build items, expanded through components.
    let root = Mat4::from_scale(Vec3::splat(unit as f32));
    let mut placed: Vec<(u32, Mat4)> = Vec::new();
    fn expand(objects: &HashMap<u32, Object>, id: u32, m: Mat4, depth: u32, out: &mut Vec<(u32, Mat4)>) {
        let Some(o) = objects.get(&id) else { return };
        if depth > 32 {
            return;
        }
        if !o.triangles.is_empty() {
            out.push((id, m));
        }
        for &(child, t) in &o.components {
            expand(objects, child, m * t, depth + 1, out);
        }
    }
    let items: Vec<_> = model.descendants().filter(|n| n.has_tag_name("item")).collect();
    if items.is_empty() {
        let mut ids: Vec<u32> = objects.keys().copied().collect();
        ids.sort();
        for id in ids {
            expand(&objects, id, root, 0, &mut placed);
        }
    }
    for item in items {
        if let Some(id) = item.attribute("objectid").and_then(|v| v.parse().ok()) {
            expand(&objects, id, root * transform(item.attribute("transform")), 0, &mut placed);
        }
    }
    if placed.is_empty() {
        return Err("The 3MF model has no printable objects".into());
    }

    // One mesh per (object instance, material). Faceted like STL: split per triangle.
    let mut materials = vec![Material::default()];
    let mut material_of: HashMap<(u32, u32), usize> = HashMap::new();
    let mut meshes = Vec::new();
    let mut source_vertices = 0;
    for (id, m) in placed {
        let o = &objects[&id];
        source_vertices += o.vertices.len();
        let mut groups: Vec<(usize, Vec<[u32; 3]>)> = Vec::new();
        for &(tri, key) in &o.triangles {
            if tri.iter().any(|&v| v as usize >= o.vertices.len()) {
                continue;
            }
            let material = match key.or(o.default).and_then(|k| colors.get(&k).map(|c| (k, c))) {
                Some((k, (c, name))) => *material_of.entry(k).or_insert_with(|| {
                    materials.push(Material { name: name.clone(), base_color: *c, ..Material::default() });
                    materials.len() - 1
                }),
                None => 0,
            };
            match groups.iter_mut().find(|g| g.0 == material) {
                Some(g) => g.1.push(tri),
                None => groups.push((material, vec![tri])),
            }
        }
        for (k, (material, tris)) in groups.into_iter().enumerate() {
            let mut positions = Vec::with_capacity(tris.len() * 3);
            let mut normals = Vec::with_capacity(tris.len() * 3);
            let mut shared = Vec::with_capacity(tris.len() * 3);
            let mut first_copy: HashMap<u32, u32> = HashMap::new();
            for tri in &tris {
                let p = tri.map(|v| Vec3::from(o.vertices[v as usize]));
                let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                for (j, &v) in tri.iter().enumerate() {
                    let idx = positions.len() as u32;
                    positions.push(p[j].to_array());
                    normals.push(n.to_array());
                    shared.push(*first_copy.entry(v).or_insert(idx));
                }
            }
            let indices: Vec<u32> = (0..positions.len() as u32).collect();
            let name = if k == 0 { o.name.clone() } else { format!("{}.{k:03}", o.name) };
            let mut mesh = Mesh::build(
                MeshData {
                    name,
                    positions,
                    normals: Some(normals),
                    uvs: None,
                    tangents: None,
                    colors: None,
                    indices,
                    transform: m,
                    material,
                    rig: MeshRig::default(),
                },
                false,
            );
            mesh.edges = unique_edges(&shared);
            meshes.push(mesh);
        }
    }
    let mut scene = Scene::new(meshes, materials, Vec::new(), source_vertices);
    scene.units = Units::Declared(unit);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="1"><base name="Red" displaycolor="#FF0000"/><base name="Blue" displaycolor="#0000FF"/></basematerials>
    <object id="2" type="model" pid="1" pindex="0">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="10"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/>
          <triangle v1="1" v2="2" v3="3" pid="1" p1="1"/><triangle v1="0" v2="3" v3="2"/>
        </triangles>
      </mesh>
    </object>
    <object id="3" type="model"><components><component objectid="2" transform="1 0 0 0 1 0 0 0 1 20 0 0"/></components></object>
  </resources>
  <build><item objectid="2"/><item objectid="3"/></build>
</model>"##;

    #[test]
    fn items_components_materials_and_units() {
        let scene = parse(MODEL).unwrap();
        // Two placements of the tetrahedron, each split into red and blue parts.
        assert_eq!(scene.meshes.len(), 4);
        assert_eq!(scene.materials.len(), 3);
        assert_eq!(scene.units, Units::Declared(1e-3));
        // 10 mm tetrahedron, second copy 20 mm along X: bounds 0..30 mm.
        assert!((scene.bounds.max.x - 0.03).abs() < 1e-6, "{:?}", scene.bounds);
        assert!((scene.bounds.max.z - 0.01).abs() < 1e-6);
    }
}
