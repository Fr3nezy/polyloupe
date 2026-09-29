//! COLLADA (.dae): geometry (triangles, polylist, polygons), the visual scene's node tree
//! with its transforms, instanced nodes, skinned geometry in bind pose, and common-profile
//! materials (diffuse color or texture, transparency). Honors the asset's unit and up axis.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{Mat4, Vec3};
use roxmltree::{Document, Node};

use crate::scene::{Material, Mesh, MeshData, MeshRig, Scene, Units, y_up_to_z_up};

/// One primitive of a geometry, already unrolled into an indexed mesh.
struct Primitive {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Option<Vec<[f32; 2]>>,
    colors: Option<Vec<[f32; 4]>>,
    indices: Vec<u32>,
    /// Material symbol, bound per instance.
    symbol: Option<String>,
    source_vertices: usize,
}

struct Parser<'a, 'input> {
    ids: HashMap<&'a str, Node<'a, 'input>>,
    geometries: HashMap<String, Vec<Primitive>>,
    materials: Vec<Material>,
    material_of: HashMap<String, usize>,
    texture_files: Vec<(String, Vec<PathBuf>)>,
    texture_of: HashMap<String, usize>,
    base: PathBuf,
    meshes: Vec<Mesh>,
    source_vertices: usize,
}

fn tag<'a, 'input>(n: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn tags<'a, 'input: 'a>(n: Node<'a, 'input>, name: &'a str) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    n.children().filter(move |c| c.has_tag_name(name))
}

fn floats(n: Node) -> Vec<f32> {
    n.text().unwrap_or("").split_ascii_whitespace().filter_map(|v| v.parse().ok()).collect()
}

fn url(v: &str) -> &str {
    v.trim_start_matches('#')
}

pub fn load(path: &Path, texture_dirs: &[PathBuf]) -> Result<Scene, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let (mut scene, files) = parse(&text, base)?;
    let mut warnings = Vec::new();
    scene.images = crate::loader::load_texture_files(files, texture_dirs, &mut warnings);
    scene.warnings = warnings;
    Ok(scene)
}

/// Parses the document; textures come back as file candidates for the caller to decode.
pub fn parse(text: &str, base: PathBuf) -> Result<(Scene, Vec<(String, Vec<PathBuf>)>), String> {
    let doc = Document::parse(text).map_err(|e| format!("Invalid COLLADA XML: {e}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("COLLADA") {
        return Err("Not a COLLADA file".into());
    }
    let asset = tag(root, "asset");
    let meter = asset.and_then(|a| tag(a, "unit")).and_then(|u| u.attribute("meter")).and_then(|m| m.parse::<f64>().ok()).unwrap_or(1.0);
    let up = asset.and_then(|a| tag(a, "up_axis")).and_then(|u| u.text()).unwrap_or("Y_UP").trim().to_string();
    let axes = match up.as_str() {
        "Z_UP" => Mat4::IDENTITY,
        // X up: X becomes Z.
        "X_UP" => Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2),
        _ => y_up_to_z_up(),
    };
    let world = axes * Mat4::from_scale(Vec3::splat(meter as f32));

    let mut p = Parser {
        ids: doc.descendants().filter_map(|n| Some((n.attribute("id")?, n))).collect(),
        geometries: HashMap::new(),
        materials: vec![Material::default()],
        material_of: HashMap::new(),
        texture_files: Vec::new(),
        texture_of: HashMap::new(),
        base,
        meshes: Vec::new(),
        source_vertices: 0,
    };

    let scene_node = tag(root, "scene")
        .and_then(|s| tag(s, "instance_visual_scene"))
        .and_then(|i| i.attribute("url"))
        .and_then(|u| p.ids.get(url(u)).copied())
        .or_else(|| doc.descendants().find(|n| n.has_tag_name("visual_scene")))
        .ok_or("The COLLADA file has no visual scene")?;
    for node in tags(scene_node, "node") {
        p.node(node, world, 0);
    }
    if p.meshes.is_empty() {
        return Err("The COLLADA file has no geometry in its scene".into());
    }
    let mut scene = Scene::new(p.meshes, p.materials, Vec::new(), p.source_vertices);
    scene.units = Units::Declared(meter);
    Ok((scene, p.texture_files))
}

impl<'a, 'input> Parser<'a, 'input> {
    fn node(&mut self, node: Node<'a, 'input>, parent: Mat4, depth: u32) {
        if depth > 64 {
            return;
        }
        let m = parent * local_transform(node);
        let name = node.attribute("name").or(node.attribute("id")).unwrap_or("Node").to_string();
        for child in node.children().filter(|c| c.is_element()) {
            match child.tag_name().name() {
                "node" => self.node(child, m, depth + 1),
                "instance_node" => {
                    if let Some(target) = child.attribute("url").and_then(|u| self.ids.get(url(u)).copied()) {
                        self.node(target, m, depth + 1);
                    }
                }
                "instance_geometry" => {
                    if let Some(id) = child.attribute("url").map(url) {
                        let bindings = self.bindings(child);
                        self.place(id, &name, m, &bindings);
                    }
                }
                // Skinned meshes: the skin's source geometry in bind pose.
                "instance_controller" => {
                    let Some(controller) = child.attribute("url").and_then(|u| self.ids.get(url(u)).copied()) else { continue };
                    let Some(skin) = tag(controller, "skin") else { continue };
                    let bind = tag(skin, "bind_shape_matrix").map(floats).filter(|v| v.len() == 16);
                    let bind = bind.map_or(Mat4::IDENTITY, |v| Mat4::from_cols_array(&v.try_into().unwrap()).transpose());
                    if let Some(id) = skin.attribute("source").map(url) {
                        let bindings = self.bindings(child);
                        self.place(id, &name, m * bind, &bindings);
                    }
                }
                _ => {}
            }
        }
    }

    /// symbol -> material index, from <bind_material>.
    fn bindings(&mut self, instance: Node<'a, 'input>) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        for im in instance.descendants().filter(|n| n.has_tag_name("instance_material")) {
            if let (Some(symbol), Some(target)) = (im.attribute("symbol"), im.attribute("target")) {
                let index = self.material(url(target));
                out.insert(symbol.to_string(), index);
            }
        }
        out
    }

    fn place(&mut self, geometry: &str, name: &str, transform: Mat4, bindings: &HashMap<String, usize>) {
        if !self.geometries.contains_key(geometry) {
            let prims = self.ids.get(geometry).copied().and_then(|g| tag(g, "mesh")).map(|m| self.mesh(m)).unwrap_or_default();
            self.geometries.insert(geometry.to_string(), prims);
        }
        let prims = &self.geometries[geometry];
        for (k, prim) in prims.iter().enumerate() {
            if prim.indices.is_empty() {
                continue;
            }
            self.source_vertices += prim.source_vertices;
            let material = prim.symbol.as_ref().and_then(|s| bindings.get(s).copied()).unwrap_or(0);
            let mesh = Mesh::build(
                MeshData {
                    name: if k == 0 { name.to_string() } else { format!("{name}.{k:03}") },
                    positions: prim.positions.clone(),
                    normals: prim.normals.clone(),
                    uvs: prim.uvs.clone(),
                    tangents: None,
                    colors: prim.colors.clone(),
                    indices: prim.indices.clone(),
                    transform,
                    material,
                    rig: MeshRig::default(),
                },
                self.materials.get(material).is_some_and(|m| m.normal_tex.is_some()),
            );
            self.meshes.push(mesh);
        }
    }

    /// A source's floats and stride.
    fn source(&self, id: &str) -> Option<(Vec<f32>, usize)> {
        let src = self.ids.get(id).copied()?;
        // <vertices> redirects to its POSITION input.
        if src.has_tag_name("vertices") {
            let input = tags(src, "input").find(|i| i.attribute("semantic") == Some("POSITION"))?;
            return self.source(url(input.attribute("source")?));
        }
        let data = floats(tag(src, "float_array")?);
        let stride = tag(src, "technique_common")
            .and_then(|t| tag(t, "accessor"))
            .and_then(|a| a.attribute("stride"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(3usize);
        Some((data, stride.max(1)))
    }

    fn mesh(&self, mesh: Node<'a, 'input>) -> Vec<Primitive> {
        let mut out = Vec::new();
        for prim in mesh.children().filter(|c| matches!(c.tag_name().name(), "triangles" | "polylist" | "polygons")) {
            // Inputs: semantic -> (offset, source). VERTEX expands to the <vertices> inputs.
            let mut inputs: Vec<(String, usize, String)> = Vec::new();
            for input in tags(prim, "input") {
                let (Some(semantic), Some(source)) = (input.attribute("semantic"), input.attribute("source")) else { continue };
                let offset = input.attribute("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
                if semantic == "TEXCOORD" && input.attribute("set").is_some_and(|s| s != "0") && inputs.iter().any(|i| i.0 == "TEXCOORD") {
                    continue;
                }
                if semantic == "VERTEX" {
                    if let Some(v) = self.ids.get(url(source)) {
                        for vi in tags(*v, "input") {
                            if let (Some(s), Some(src)) = (vi.attribute("semantic"), vi.attribute("source")) {
                                inputs.push((s.to_string(), offset, url(src).to_string()));
                            }
                        }
                    }
                } else {
                    inputs.push((semantic.to_string(), offset, url(source).to_string()));
                }
            }
            let stride = inputs.iter().map(|i| i.1).max().map_or(1, |m| m + 1);
            let Some(pos_in) = inputs.iter().find(|i| i.0 == "POSITION") else { continue };
            let Some(pos_data) = self.source(&pos_in.2) else { continue };
            let lookup = |sem: &str| inputs.iter().find(|i| i.0 == sem).and_then(|i| Some((i.1, self.source(&i.2)?)));
            let normal = lookup("NORMAL");
            let uv = lookup("TEXCOORD");
            let color = lookup("COLOR");

            // Polygons as lists of corners (each corner = `stride` indices).
            let mut polygons: Vec<Vec<usize>> = Vec::new();
            match prim.tag_name().name() {
                "triangles" => {
                    let p: Vec<usize> = tag(prim, "p").map(|n| floats(n).into_iter().map(|v| v as usize).collect()).unwrap_or_default();
                    for tri in p.chunks_exact(stride * 3) {
                        polygons.push(tri.to_vec());
                    }
                }
                "polylist" => {
                    let p: Vec<usize> = tag(prim, "p").map(|n| floats(n).into_iter().map(|v| v as usize).collect()).unwrap_or_default();
                    let counts: Vec<usize> = tag(prim, "vcount").map(|n| floats(n).into_iter().map(|v| v as usize).collect()).unwrap_or_default();
                    let mut at = 0;
                    for n in counts {
                        let len = n * stride;
                        if at + len > p.len() {
                            break;
                        }
                        polygons.push(p[at..at + len].to_vec());
                        at += len;
                    }
                }
                _ => {
                    for ph in tags(prim, "p") {
                        polygons.push(floats(ph).into_iter().map(|v| v as usize).collect());
                    }
                }
            }

            // Unroll corners, sharing identical ones so smooth normals and edges survive.
            let mut corner_index: HashMap<Vec<usize>, u32> = HashMap::new();
            let mut prim_out = Primitive {
                positions: Vec::new(),
                normals: normal.as_ref().map(|_| Vec::new()),
                uvs: uv.as_ref().map(|_| Vec::new()),
                colors: color.as_ref().map(|_| Vec::new()),
                indices: Vec::new(),
                symbol: prim.attribute("material").map(str::to_string),
                source_vertices: pos_data.0.len() / pos_data.1,
            };
            let get = |data: &(Vec<f32>, usize), i: usize, n: usize, default: f32| -> Vec<f32> {
                (0..n).map(|k| if k < data.1 { data.0.get(i * data.1 + k).copied().unwrap_or(default) } else { default }).collect()
            };
            for poly in &polygons {
                let mut ids = Vec::new();
                for corner in poly.chunks_exact(stride) {
                    let key = corner.to_vec();
                    let id = match corner_index.get(&key) {
                        Some(&id) => id,
                        None => {
                            let id = prim_out.positions.len() as u32;
                            let pos = get(&pos_data, corner[pos_in.1], 3, 0.0);
                            prim_out.positions.push([pos[0], pos[1], pos[2]]);
                            if let (Some(out), Some((off, data))) = (&mut prim_out.normals, &normal) {
                                let n = get(data, corner[*off], 3, 0.0);
                                out.push([n[0], n[1], n[2]]);
                            }
                            if let (Some(out), Some((off, data))) = (&mut prim_out.uvs, &uv) {
                                let t = get(data, corner[*off], 2, 0.0);
                                out.push([t[0], 1.0 - t[1]]);
                            }
                            if let (Some(out), Some((off, data))) = (&mut prim_out.colors, &color) {
                                let c = get(data, corner[*off], 4, 1.0);
                                out.push([c[0], c[1], c[2], c[3]]);
                            }
                            corner_index.insert(key, id);
                            id
                        }
                    };
                    ids.push(id);
                }
                for i in 1..ids.len().saturating_sub(1) {
                    prim_out.indices.extend_from_slice(&[ids[0], ids[i], ids[i + 1]]);
                }
            }
            out.push(prim_out);
        }
        out
    }

    /// Material by id: diffuse color or texture and transparency from the common profile.
    fn material(&mut self, id: &str) -> usize {
        if let Some(&i) = self.material_of.get(id) {
            return i;
        }
        let node = self.ids.get(id).copied();
        let name = node.and_then(|n| n.attribute("name")).unwrap_or(id).to_string();
        let effect = node
            .and_then(|n| tag(n, "instance_effect"))
            .and_then(|e| e.attribute("url"))
            .and_then(|u| self.ids.get(url(u)).copied());
        let mut material = Material { name, ..Material::default() };
        if let Some(effect) = effect {
            let technique = effect
                .descendants()
                .find(|n| n.has_tag_name("technique") && n.parent().is_some_and(|p| p.has_tag_name("profile_COMMON")));
            let shading = technique.and_then(|t| t.children().find(|c| matches!(c.tag_name().name(), "phong" | "lambert" | "blinn" | "constant")));
            if let Some(diffuse) = shading.and_then(|s| tag(s, "diffuse")) {
                if let Some(c) = tag(diffuse, "color").map(floats).filter(|c| c.len() >= 3) {
                    material.base_color = [c[0], c[1], c[2], c.get(3).copied().unwrap_or(1.0)];
                }
                if let Some(texture) = tag(diffuse, "texture").and_then(|t| t.attribute("texture")) {
                    if let Some(image) = self.resolve_image(effect, texture) {
                        material.base_color_tex = Some(image);
                        material.base_color = [1.0, 1.0, 1.0, 1.0];
                    }
                }
            }
            if let Some(t) = shading.and_then(|s| tag(s, "transparency")).and_then(|t| tag(t, "float")).and_then(|f| f.text()?.trim().parse::<f32>().ok()) {
                // Most exporters write opacity here (A_ONE); 0 would hide everything.
                if t > 0.0 && t < 0.999 {
                    material.base_color[3] *= t;
                    material.alpha_mode = crate::scene::AlphaMode::Blend;
                }
            }
        }
        self.materials.push(material);
        let index = self.materials.len() - 1;
        self.material_of.insert(id.to_string(), index);
        index
    }

    /// texture attribute -> sampler newparam -> surface newparam -> image -> file.
    fn resolve_image(&mut self, effect: Node<'a, 'input>, texture: &str) -> Option<usize> {
        let param = |sid: &str| effect.descendants().find(|n| n.has_tag_name("newparam") && n.attribute("sid") == Some(sid));
        let mut image_id = texture.to_string();
        if let Some(sampler) = param(texture).and_then(|p| tag(p, "sampler2D")) {
            if let Some(source) = tag(sampler, "source").and_then(|s| s.text()) {
                if let Some(init) = param(source.trim()).and_then(|p| tag(p, "surface")).and_then(|s| tag(s, "init_from")).and_then(|i| i.text()) {
                    image_id = init.trim().to_string();
                }
            } else if let Some(inst) = tag(sampler, "instance_image").and_then(|i| i.attribute("url")) {
                image_id = url(inst).to_string();
            }
        }
        if let Some(&i) = self.texture_of.get(&image_id) {
            return Some(i);
        }
        let image = self.ids.get(image_id.as_str()).copied()?;
        let init = tag(image, "init_from")?;
        let raw = tag(init, "ref").and_then(|r| r.text()).or(init.text())?.trim().to_string();
        let path = decode_path(&raw);
        let candidates = vec![self.base.join(&path), PathBuf::from(&path)];
        let name = Path::new(&path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
        self.texture_files.push((name, candidates));
        let index = self.texture_files.len() - 1;
        self.texture_of.insert(image_id, index);
        Some(index)
    }
}

/// file:/// URIs and %20-style escapes, as exporters write them.
fn decode_path(raw: &str) -> String {
    let s = raw.strip_prefix("file:///").or(raw.strip_prefix("file://")).unwrap_or(raw);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The node's own transform: its transform elements multiplied in document order.
fn local_transform(node: Node) -> Mat4 {
    let mut m = Mat4::IDENTITY;
    for t in node.children().filter(|c| c.is_element()) {
        let v = floats(t);
        let step = match t.tag_name().name() {
            "matrix" if v.len() == 16 => Mat4::from_cols_array(&v.clone().try_into().unwrap()).transpose(),
            "translate" if v.len() == 3 => Mat4::from_translation(Vec3::new(v[0], v[1], v[2])),
            "scale" if v.len() == 3 => Mat4::from_scale(Vec3::new(v[0], v[1], v[2])),
            "rotate" if v.len() == 4 => {
                let axis = Vec3::new(v[0], v[1], v[2]);
                if axis.length_squared() > 0.0 { Mat4::from_axis_angle(axis.normalize(), v[3].to_radians()) } else { Mat4::IDENTITY }
            }
            _ => continue,
        };
        m *= step;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAE: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
  <asset><unit name="centimeter" meter="0.01"/><up_axis>Y_UP</up_axis></asset>
  <library_effects><effect id="red-fx"><profile_COMMON><technique sid="common"><lambert>
    <diffuse><color>1 0 0 1</color></diffuse></lambert></technique></profile_COMMON></effect></library_effects>
  <library_materials><material id="red" name="Red"><instance_effect url="#red-fx"/></material></library_materials>
  <library_geometries><geometry id="quad" name="Quad"><mesh>
    <source id="pos"><float_array id="pos-a" count="12">0 0 0 100 0 0 100 100 0 0 100 0</float_array>
      <technique_common><accessor source="#pos-a" count="4" stride="3"/></technique_common></source>
    <vertices id="verts"><input semantic="POSITION" source="#pos"/></vertices>
    <polylist material="mat" count="1"><input semantic="VERTEX" source="#verts" offset="0"/>
      <vcount>4</vcount><p>0 1 2 3</p></polylist>
  </mesh></geometry></library_geometries>
  <library_visual_scenes><visual_scene id="scene"><node name="Q"><translate>0 0 50</translate>
    <instance_geometry url="#quad"><bind_material><technique_common>
      <instance_material symbol="mat" target="#red"/></technique_common></bind_material></instance_geometry>
  </node></visual_scene></library_visual_scenes>
  <scene><instance_visual_scene url="#scene"/></scene>
</COLLADA>"##;

    #[test]
    fn quad_with_units_up_axis_and_material() {
        let (scene, _) = parse(DAE, PathBuf::new()).unwrap();
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.meshes[0].indices.len(), 6);
        let m = &scene.materials[scene.meshes[0].material];
        assert_eq!(m.base_color, [1.0, 0.0, 0.0, 1.0]);
        // 100 cm quad in XY, Y up: 1 m wide, 1 m tall in Z after the axis swap, 0.5 m toward -Y.
        let b = scene.bounds;
        assert!((b.max.x - 1.0).abs() < 1e-5 && (b.max.z - 1.0).abs() < 1e-5, "{b:?}");
        assert!((b.min.y + 0.5).abs() < 1e-5, "{b:?}");
        assert_eq!(scene.units, Units::Declared(0.01));
    }
}
