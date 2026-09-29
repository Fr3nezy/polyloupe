//! File format loaders. Everything here runs on a background thread.

use crate::i18n::trf;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use glam::{Mat4, Quat, Vec3};

use crate::scene::{
    Aabb, AlphaMode, Animation, Channel, ChannelTex, Clip, Image, Interpolation, Material, Mesh,
    MeshData, MeshRig, MorphTarget, Node, Property, Scene, Skin, unique_edges, y_up_to_z_up,
};

pub const SUPPORTED_EXTENSIONS: &[&str] = &["glb", "gltf", "fbx", "obj", "stl", "ply", "3mf", "dae"];
pub const ENVIRONMENT_EXTENSIONS: &[&str] = &["hdr", "exr"];

pub fn is_supported(path: &Path) -> bool {
    extension(path).is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.as_str()))
}

pub fn is_environment(path: &Path) -> bool {
    extension(path).is_some_and(|e| ENVIRONMENT_EXTENSIONS.contains(&e.as_str()))
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

pub fn load(path: &Path) -> Result<Scene, String> {
    load_with(path, &[])
}

/// Like [`load`], also looking for texture files (by name) in `texture_dirs` when they aren't
/// where the model says: the "Choose texture folder" fix for models whose textures moved.
pub fn load_with(path: &Path, texture_dirs: &[PathBuf]) -> Result<Scene, String> {
    let mut scene = match extension(path).as_deref() {
        Some("glb" | "gltf") => load_gltf(path, texture_dirs),
        Some("fbx" | "obj") => load_ufbx(path, texture_dirs),
        Some("stl") => load_stl(path),
        Some("ply") => crate::formats::ply::load(path),
        Some("3mf") => crate::formats::threemf::load(path),
        Some("dae") => crate::formats::collada::load(path, texture_dirs),
        Some(other) => Err(format!(".{other} files aren't supported yet")),
        None => Err("The file has no extension, so its format is unknown".into()),
    }?;
    drop_missing_textures(&mut scene);
    Ok(scene)
}

/// First existing file among `candidates`, then the same file names inside `dirs`.
fn find_texture(candidates: &[PathBuf], dirs: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| !p.as_os_str().is_empty() && p.is_file()).cloned().or_else(|| {
        let names: Vec<&std::ffi::OsStr> = candidates.iter().filter_map(|p| p.file_name()).collect();
        dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
    })
}

/// Exporters often keep paths to textures that never shipped with the model. Showing those
/// maps as pink hides the model, so they are dropped and the material falls back to its plain
/// color; the warning toast still says what's missing.
fn drop_missing_textures(scene: &mut Scene) {
    let missing: Vec<bool> = scene.images.iter().map(|i| i.name.ends_with(MISSING_SUFFIX)).collect();
    let gone = |i: &Option<usize>| i.is_some_and(|i| missing.get(i).copied().unwrap_or(false));
    for m in &mut scene.materials {
        if gone(&m.base_color_tex) {
            m.base_color_tex = None;
        }
        if gone(&m.normal_tex) {
            m.normal_tex = None;
        }
        if gone(&m.emissive_tex) {
            m.emissive_tex = None;
        }
        for t in [&mut m.metallic_tex, &mut m.roughness_tex, &mut m.occlusion_tex] {
            if t.is_some_and(|c| missing.get(c.image).copied().unwrap_or(false)) {
                *t = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Images

enum ImageSource {
    Bytes(Vec<u8>),
    Missing,
}

struct ImageJob {
    name: String,
    source: ImageSource,
}

/// Texture files by name and candidate paths (looked up in `dirs` too), decoded in parallel.
/// KHR_texture_transform (tiling, offset, rotation of a texture's UVs), baked into the mesh UVs.
/// The spec allows one per texture; materials almost always use the same for all of them, so
/// the base color's wins, then metallic-roughness, then emissive.
fn uv_transform(material: &gltf::Material<'_>) -> Option<impl Fn([f32; 2]) -> [f32; 2]> {
    let pbr = material.pbr_metallic_roughness();
    let t = pbr
        .base_color_texture()
        .and_then(|i| i.texture_transform())
        .or_else(|| pbr.metallic_roughness_texture().and_then(|i| i.texture_transform()))
        .or_else(|| material.emissive_texture().and_then(|i| i.texture_transform()))?;
    let ([ox, oy], [sx, sy], (s, c)) = (t.offset(), t.scale(), t.rotation().sin_cos());
    // uv' = translation * rotation * scale * uv, as the extension defines it.
    Some(move |[u, v]: [f32; 2]| [c * sx * u + s * sy * v + ox, -s * sx * u + c * sy * v + oy])
}

pub(crate) fn load_texture_files(files: Vec<(String, Vec<PathBuf>)>, dirs: &[PathBuf], warnings: &mut Vec<String>) -> Vec<Image> {
    let jobs = files
        .into_iter()
        .map(|(name, candidates)| {
            let source = find_texture(&candidates, dirs)
                .and_then(|p| std::fs::read(p).ok())
                .map_or(ImageSource::Missing, ImageSource::Bytes);
            ImageJob { name, source }
        })
        .collect();
    decode_images(jobs, warnings)
}

/// Decodes all images in parallel and builds their mip chains. Failures become a placeholder
/// named with [`MISSING_SUFFIX`] plus a warning; [`drop_missing_textures`] then unhooks them.
fn decode_images(jobs: Vec<ImageJob>, warnings: &mut Vec<String>) -> Vec<Image> {
    let count = jobs.len();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(count.max(1));
    let mut results: Vec<Option<Result<Image, String>>> = (0..count).map(|_| None).collect();
    let chunks: Vec<Vec<(usize, Result<Image, String>)>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(i) else { break };
                        done.push((i, decode_one(job)));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    for (i, r) in chunks.into_iter().flatten() {
        results[i] = Some(r);
    }
    results
        .into_iter()
        .enumerate()
        .map(|(i, r)| match r {
            Some(Ok(img)) => img,
            Some(Err(e)) => {
                warnings.push(e);
                missing_image(&jobs[i].name)
            }
            None => missing_image(&jobs[i].name),
        })
        .collect()
}

fn decode_one(job: &ImageJob) -> Result<Image, String> {
    let bytes = match &job.source {
        ImageSource::Bytes(b) => b,
        ImageSource::Missing => return Err(trf("Missing texture: {name}", &[("name", &job.name)])),
    };
    let decoded = image::load_from_memory(bytes).or_else(|_| {
        let format = image::ImageFormat::from_path(&job.name)
            .map_err(|_| image::ImageError::Unsupported(image::error::ImageFormatHint::Unknown.into()))?;
        image::load_from_memory_with_format(bytes, format)
    });
    let rgba = decoded
        .map_err(|e| trf("Couldn't decode texture {name}: {error}", &[("name", &job.name), ("error", &e)]))?
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(Image {
        name: job.name.clone(),
        width: w,
        height: h,
        mips: build_mips(w, h, rgba.into_raw()),
    })
}

pub const MISSING_SUFFIX: &str = " (missing)";

fn missing_image(name: &str) -> Image {
    Image {
        name: format!("{name}{MISSING_SUFFIX}"),
        width: 1,
        height: 1,
        mips: vec![vec![255, 0, 255, 255]],
    }
}

/// Box-filtered mip chain down to 1x1.
fn build_mips(width: u32, height: u32, level0: Vec<u8>) -> Vec<Vec<u8>> {
    let mut mips = vec![level0];
    let (mut w, mut h) = (width as usize, height as usize);
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let src = mips.last().expect("level 0 exists");
        let mut dst = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            let y0 = (y * 2).min(h - 1);
            let y1 = (y * 2 + 1).min(h - 1);
            for x in 0..nw {
                let x0 = (x * 2).min(w - 1);
                let x1 = (x * 2 + 1).min(w - 1);
                for c in 0..4 {
                    let sum = src[(y0 * w + x0) * 4 + c] as u32
                        + src[(y0 * w + x1) * 4 + c] as u32
                        + src[(y1 * w + x0) * 4 + c] as u32
                        + src[(y1 * w + x1) * 4 + c] as u32;
                    dst[(y * nw + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        mips.push(dst);
        w = nw;
        h = nh;
    }
    mips
}

fn percent_decode(s: &str) -> String {
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

// ---------------------------------------------------------------------------------------------
// glTF / GLB

fn load_gltf(path: &Path, texture_dirs: &[PathBuf]) -> Result<Scene, String> {
    use base64::Engine;

    let gltf = gltf::Gltf::open(path).map_err(|e| format!("Invalid glTF: {e}"))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let buffers = gltf::import_buffers(&gltf.document, Some(base), gltf.blob.clone())
        .map_err(|e| format!("Couldn't read glTF buffers: {e}"))?;
    let doc = &gltf.document;
    let mut warnings = Vec::new();

    let jobs: Vec<ImageJob> = doc
        .images()
        .map(|img| {
            let name = img.name().map(str::to_string);
            match img.source() {
                gltf::image::Source::View { view, mime_type } => {
                    let data = &buffers[view.buffer().index()].0;
                    let bytes = data[view.offset()..view.offset() + view.length()].to_vec();
                    let ext = mime_type.rsplit('/').next().unwrap_or("png");
                    ImageJob {
                        name: name.unwrap_or_else(|| format!("image_{}.{ext}", img.index())),
                        source: ImageSource::Bytes(bytes),
                    }
                }
                gltf::image::Source::Uri { uri, .. } => {
                    if let Some(rest) = uri.strip_prefix("data:") {
                        let payload = rest.split_once(',').map(|(_, p)| p).unwrap_or("");
                        let source = base64::engine::general_purpose::STANDARD
                            .decode(payload)
                            .map_or(ImageSource::Missing, ImageSource::Bytes);
                        ImageJob {
                            name: name.unwrap_or_else(|| format!("image_{}", img.index())),
                            source,
                        }
                    } else {
                        let file = find_texture(&[base.join(percent_decode(uri))], texture_dirs);
                        ImageJob {
                            name: uri.to_string(),
                            source: file
                                .and_then(|f| std::fs::read(f).ok())
                                .map_or(ImageSource::Missing, ImageSource::Bytes),
                        }
                    }
                }
            }
        })
        .collect();

    let tex_image = |t: gltf::Texture| t.source().index();
    let mut materials: Vec<Material> = doc
        .materials()
        .map(|m| {
            let pbr = m.pbr_metallic_roughness();
            let mr = pbr.metallic_roughness_texture().map(|i| tex_image(i.texture()));
            let e = m.emissive_factor();
            Material {
                name: m.name().unwrap_or("Material").to_string(),
                base_color: pbr.base_color_factor(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                emissive: e,
                normal_scale: m.normal_texture().map_or(1.0, |n| n.scale()),
                occlusion_strength: m.occlusion_texture().map_or(1.0, |o| o.strength()),
                alpha_mode: match m.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                    gltf::material::AlphaMode::Mask => AlphaMode::Mask(m.alpha_cutoff().unwrap_or(0.5)),
                    gltf::material::AlphaMode::Blend => AlphaMode::Blend,
                },
                base_color_tex: pbr.base_color_texture().map(|i| tex_image(i.texture())),
                normal_tex: m.normal_texture().map(|n| tex_image(n.texture())),
                emissive_tex: m.emissive_texture().map(|i| tex_image(i.texture())),
                // glTF packs roughness in G and metallic in B of one texture.
                roughness_tex: mr.map(|image| ChannelTex { image, channel: 1 }),
                metallic_tex: mr.map(|image| ChannelTex { image, channel: 2 }),
                occlusion_tex: m
                    .occlusion_texture()
                    .map(|o| ChannelTex { image: tex_image(o.texture()), channel: 0 }),
            }
        })
        .collect();
    let default_material = materials.len();
    materials.push(Material::default());

    // Node hierarchy. Our node 0 is a root carrying the Y-up -> Z-up conversion, so animated
    // and skinned content lands in Blender's orientation too.
    let mut anim = Animation::default();
    anim.nodes.push(Node {
        name: "Root".into(),
        parent: None,
        translation: Vec3::ZERO,
        rotation: Quat::from_mat4(&y_up_to_z_up()),
        scale: Vec3::ONE,
        weights: Vec::new(),
    });
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    let roots: Vec<gltf::Node> = match scene {
        Some(s) => s.nodes().collect(),
        None => doc.nodes().collect(),
    };
    let mut node_map: HashMap<usize, usize> = HashMap::new();
    let mut order: Vec<(gltf::Node, usize)> = Vec::new();
    let mut stack: Vec<(gltf::Node, usize)> = roots.into_iter().rev().map(|n| (n, 0)).collect();
    while let Some((node, parent)) = stack.pop() {
        let (t, r, sc) = node.transform().decomposed();
        let idx = anim.nodes.len();
        let weights = node
            .weights()
            .or_else(|| node.mesh().and_then(|m| m.weights()))
            .map(|w| w.to_vec())
            .unwrap_or_default();
        anim.nodes.push(Node {
            name: node.name().unwrap_or("Node").to_string(),
            parent: Some(parent),
            translation: Vec3::from(t),
            rotation: Quat::from_array(r).normalize(),
            scale: Vec3::from(sc),
            weights,
        });
        node_map.insert(node.index(), idx);
        for child in node.children().collect::<Vec<_>>().into_iter().rev() {
            stack.push((child, idx));
        }
        order.push((node, idx));
    }

    let mut skin_map: HashMap<usize, usize> = HashMap::new();
    for skin in doc.skins() {
        let joints: Option<Vec<usize>> = skin.joints().map(|j| node_map.get(&j.index()).copied()).collect();
        let Some(joints) = joints else { continue };
        let reader = skin.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
        let inverse_bind = match reader.read_inverse_bind_matrices() {
            Some(m) => m.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
            None => vec![Mat4::IDENTITY; joints.len()],
        };
        skin_map.insert(skin.index(), anim.skins.len());
        anim.skins.push(Skin { joints, inverse_bind });
    }

    let rest = anim.rest_world();
    let mut meshes = Vec::new();
    let mut vertex_count = 0;
    for (node, idx) in &order {
        let Some(mesh) = node.mesh() else { continue };
        let skin = node.skin().and_then(|s| skin_map.get(&s.index()).copied());
        let base_name = node.name().or_else(|| mesh.name()).unwrap_or("Mesh").to_string();
        let prim_count = mesh.primitives().len();
        for (pi, prim) in mesh.primitives().enumerate() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = reader.read_positions() else { continue };
            let positions: Vec<[f32; 3]> = positions.collect();
            let count = positions.len();
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..count as u32).collect(),
            };
            vertex_count += count;
            let material = prim.material().index().unwrap_or(default_material);
            let needs_tangents = materials[material].normal_tex.is_some();
            let morph_targets = reader
                .read_morph_targets()
                .enumerate()
                .map(|(i, (p, n, _))| MorphTarget {
                    name: format!("Target {i}"),
                    positions: p.map(|p| p.collect()).unwrap_or_else(|| vec![[0.0; 3]; count]),
                    normals: n.map(|n| n.collect()),
                })
                .collect();
            let (joints, weights) = match skin {
                Some(_) => (
                    reader.read_joints(0).map(|j| j.into_u16().collect()),
                    reader.read_weights(0).map(|w| w.into_f32().collect()),
                ),
                None => (None, None),
            };
            let skinned = skin.is_some() && joints.is_some() && weights.is_some();
            let data = MeshData {
                name: if prim_count > 1 { format!("{base_name}.{pi:03}") } else { base_name.clone() },
                positions,
                normals: reader.read_normals().map(|n| n.collect()),
                uvs: reader.read_tex_coords(0).map(|t| {
                    let uvs = t.into_f32();
                    match uv_transform(&prim.material()) {
                        Some(transform) => uvs.map(|uv| transform(uv)).collect(),
                        None => uvs.collect(),
                    }
                }),
                tangents: reader.read_tangents().map(|t| t.collect()),
                colors: reader.read_colors(0).map(|c| c.into_rgba_f32().collect()),
                indices,
                // glTF: skinned vertices end up in world space through the joints alone.
                transform: if skinned { Mat4::IDENTITY } else { rest[*idx] },
                material,
                rig: MeshRig {
                    node: Some(*idx),
                    offset: Mat4::IDENTITY,
                    skin: if skinned { skin } else { None },
                    joints: if skinned { joints } else { None },
                    weights: if skinned { weights } else { None },
                    morph_targets,
                },
            };
            meshes.push(finish_mesh(Mesh::build(data, needs_tangents), &anim, &rest));
        }
    }

    for (ai, a) in doc.animations().enumerate() {
        let mut channels = Vec::new();
        for ch in a.channels() {
            let Some(&node) = node_map.get(&ch.target().node().index()) else { continue };
            let reader = ch.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(times) = reader.read_inputs() else { continue };
            let times: Vec<f32> = times.collect();
            use gltf::animation::util::ReadOutputs;
            let (property, values): (Property, Vec<f32>) = match reader.read_outputs() {
                Some(ReadOutputs::Translations(it)) => (Property::Translation, it.flatten().collect()),
                Some(ReadOutputs::Rotations(it)) => (Property::Rotation, it.into_f32().flatten().collect()),
                Some(ReadOutputs::Scales(it)) => (Property::Scale, it.flatten().collect()),
                Some(ReadOutputs::MorphTargetWeights(it)) => (Property::Weights, it.into_f32().collect()),
                None => continue,
            };
            let interpolation = match ch.sampler().interpolation() {
                gltf::animation::Interpolation::Step => Interpolation::Step,
                gltf::animation::Interpolation::Linear => Interpolation::Linear,
                gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
            };
            let per_key = if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
            if times.is_empty() || values.len() % (times.len() * per_key) != 0 {
                continue;
            }
            let width = values.len() / (times.len() * per_key);
            channels.push(Channel { node, property, interpolation, times, values, width });
        }
        if channels.is_empty() {
            continue;
        }
        let end = channels.iter().filter_map(|c| c.times.last()).fold(0.0f32, |a, &b| a.max(b));
        anim.clips.push(Clip {
            name: a.name().map(str::to_string).unwrap_or_else(|| format!("Animation {}", ai + 1)),
            start: 0.0,
            duration: end.max(1e-3),
            channels,
        });
    }
    anim.fps = detect_fps(&anim.clips);

    let images = decode_images(jobs, &mut warnings);
    let mut scene = Scene::new(meshes, materials, images, vertex_count);
    scene.units = crate::scene::Units::Meters;
    scene.warnings = warnings;
    scene.animation = anim;
    Ok(scene)
}

/// Skinned meshes get their bounds from the rest pose, since their raw positions are in bind
/// space (and, for glTF, still Y-up).
fn finish_mesh(mut mesh: Mesh, anim: &Animation, rest: &[Mat4]) -> Mesh {
    if let (Some(s), Some(joints), Some(weights)) = (mesh.rig.skin, &mesh.rig.joints, &mesh.rig.weights) {
        let skin = &anim.skins[s];
        let mats: Vec<Mat4> = skin.joints.iter().zip(&skin.inverse_bind).map(|(&j, ib)| rest[j] * *ib).collect();
        let mut b = Aabb::EMPTY;
        for ((p, j), w) in mesh.positions.iter().zip(joints).zip(weights) {
            let p = Vec3::from(*p);
            let mut out = Vec3::ZERO;
            let mut total = 0.0;
            for k in 0..4 {
                if w[k] > 0.0 {
                    if let Some(m) = mats.get(j[k] as usize) {
                        out += m.transform_point3(p) * w[k];
                        total += w[k];
                    }
                }
            }
            b.grow(if total > 0.0 { out / total } else { p });
        }
        mesh.bounds = b;
    }
    mesh
}

/// Picks the common frame rate that keyframe times line up with (glTF stores seconds).
fn detect_fps(clips: &[Clip]) -> f32 {
    let times: Vec<f32> = clips
        .iter()
        .flat_map(|c| c.channels.iter().flat_map(|ch| ch.times.iter().copied()))
        .take(2000)
        .collect();
    for fps in [24.0f32, 30.0, 25.0, 60.0, 50.0, 120.0] {
        if times.iter().all(|t| ((t * fps) - (t * fps).round()).abs() < 0.02) {
            return fps;
        }
    }
    30.0
}

// ---------------------------------------------------------------------------------------------
// FBX and OBJ (ufbx)

fn load_ufbx(path: &Path, texture_dirs: &[PathBuf]) -> Result<Scene, String> {
    let path_str = path.to_str().ok_or("The file path isn't valid Unicode")?;
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_z_up(),
        target_unit_meters: 1.0,
        // The root node carries axis/unit conversion; animation stays in file space below it.
        space_conversion: ufbx::SpaceConversion::TransformRoot,
        generate_missing_normals: true,
        obj_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        obj_unit_meters: 1.0,
        // Needed to read an OBJ's .mtl file.
        load_external_files: true,
        ignore_missing_external_files: true,
        // Recovers metallic/roughness/alpha from Blender-exported FBX materials.
        use_blender_pbr_material: true,
        ..Default::default()
    };
    let scene = ufbx::load_file(path_str, opts).map_err(|e| format!("Couldn't read the file: {}", e.description))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let mut warnings = Vec::new();

    // Textures -> image jobs, deduplicated by resolved file (or embedded element).
    let mut jobs: Vec<ImageJob> = Vec::new();
    let mut image_of_texture: HashMap<u32, usize> = HashMap::new();
    let mut image_of_path: HashMap<PathBuf, usize> = HashMap::new();
    let mut resolve_texture = |tex: &ufbx::Texture| -> usize {
        let tex = tex.file_textures.first().map(|t| &**t).unwrap_or(tex);
        if let Some(&i) = image_of_texture.get(&tex.element.typed_id) {
            return i;
        }
        let name = if tex.filename.is_empty() {
            tex.element.name.to_string()
        } else {
            file_name_of(&tex.filename)
        };
        let index = if !tex.content.is_empty() {
            jobs.push(ImageJob { name, source: ImageSource::Bytes(tex.content.to_vec()) });
            jobs.len() - 1
        } else {
            let candidates = [
                PathBuf::from(&*tex.absolute_filename),
                base.join(&*tex.relative_filename),
                PathBuf::from(&*tex.filename),
                base.join(file_name_of(&tex.filename)),
                base.join("textures").join(file_name_of(&tex.filename)),
                base.join("Textures").join(file_name_of(&tex.filename)),
            ];
            let found = find_texture(&candidates, texture_dirs);
            match found {
                Some(p) => {
                    if let Some(&i) = image_of_path.get(&p) {
                        i
                    } else {
                        let source = std::fs::read(&p).map_or(ImageSource::Missing, ImageSource::Bytes);
                        jobs.push(ImageJob { name, source });
                        image_of_path.insert(p, jobs.len() - 1);
                        jobs.len() - 1
                    }
                }
                None => {
                    jobs.push(ImageJob { name, source: ImageSource::Missing });
                    jobs.len() - 1
                }
            }
        };
        image_of_texture.insert(tex.element.typed_id, index);
        index
    };

    let mut material_index: HashMap<u32, usize> = HashMap::new();
    let mut materials = Vec::new();
    for m in scene.materials.iter() {
        let pbr = &m.pbr;
        let value = |map: &ufbx::MaterialMap, default: f64| if map.has_value { map.value_vec4.x } else { default };
        let base = if pbr.base_color.has_value {
            pbr.base_color.value_vec4
        } else if m.fbx.diffuse_color.has_value {
            m.fbx.diffuse_color.value_vec4
        } else {
            ufbx::Vec4 { x: 0.8, y: 0.8, z: 0.8, w: 1.0 }
        };
        let base_factor = value(&pbr.base_factor, 1.0);
        let opacity = value(&pbr.opacity, 1.0) as f32;
        let emission = &pbr.emission_color;
        let emission_factor = value(&pbr.emission_factor, 1.0);
        let base_tex = map_texture(&pbr.base_color).or_else(|| map_texture(&m.fbx.diffuse_color));
        let normal_tex = map_texture(&pbr.normal_map).or_else(|| map_texture(&m.fbx.normal_map));
        let channel = |t: Option<&ufbx::Texture>, resolve: &mut dyn FnMut(&ufbx::Texture) -> usize| {
            t.map(|t| ChannelTex { image: resolve(t), channel: 0 })
        };
        let material = Material {
            name: m.element.name.to_string(),
            base_color: [
                (base.x * base_factor) as f32,
                (base.y * base_factor) as f32,
                (base.z * base_factor) as f32,
                opacity,
            ],
            metallic: value(&pbr.metalness, 0.0) as f32,
            roughness: value(&pbr.roughness, 0.5) as f32,
            emissive: if emission.has_value {
                [
                    (emission.value_vec4.x * emission_factor) as f32,
                    (emission.value_vec4.y * emission_factor) as f32,
                    (emission.value_vec4.z * emission_factor) as f32,
                ]
            } else {
                [0.0; 3]
            },
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_mode: if opacity < 0.999 { AlphaMode::Blend } else { AlphaMode::Opaque },
            base_color_tex: base_tex.map(&mut resolve_texture),
            normal_tex: normal_tex.map(&mut resolve_texture),
            emissive_tex: map_texture(emission).map(&mut resolve_texture),
            metallic_tex: channel(map_texture(&pbr.metalness), &mut resolve_texture),
            roughness_tex: channel(map_texture(&pbr.roughness), &mut resolve_texture),
            occlusion_tex: channel(map_texture(&pbr.ambient_occlusion), &mut resolve_texture),
        };
        // In FBX/OBJ (unlike glTF) a connected texture replaces the scalar value.
        let mut material = material;
        if material.base_color_tex.is_some() {
            material.base_color = [1.0, 1.0, 1.0, material.base_color[3]];
        }
        if material.metallic_tex.is_some() {
            material.metallic = 1.0;
        }
        if material.roughness_tex.is_some() {
            material.roughness = 1.0;
        }
        log::debug!(
            "material {:?}: base {:?}, metallic {}, roughness {}",
            material.name,
            material.base_color,
            material.metallic,
            material.roughness
        );
        material_index.insert(m.element.typed_id, materials.len());
        materials.push(material);
    }
    let default_material = materials.len();
    materials.push(Material::default());

    // Node hierarchy, parents first. With `TransformRoot`, the root node carries the axis and
    // unit conversion, so file transforms (and their animation) stay untouched below it.
    let mut anim = Animation::default();
    let mut node_index: HashMap<u32, usize> = HashMap::new();
    let mut ordered: Vec<&ufbx::Node> = Vec::new();
    let mut stack: Vec<(&ufbx::Node, Option<usize>)> = vec![(&*scene.root_node, None)];
    while let Some((node, parent)) = stack.pop() {
        let t = &node.local_transform;
        let idx = anim.nodes.len();
        anim.nodes.push(Node {
            name: node.element.name.to_string(),
            parent,
            translation: ufbx_vec3(&t.translation),
            rotation: ufbx_quat(&t.rotation),
            scale: ufbx_vec3(&t.scale),
            weights: Vec::new(),
        });
        node_index.insert(node.element.typed_id, idx);
        ordered.push(node);
        for child in node.children.iter().rev() {
            stack.push((child, Some(idx)));
        }
    }
    let rest = anim.rest_world();

    let mut meshes = Vec::new();
    let mut vertex_count = 0;
    let mut counted_meshes = std::collections::HashSet::new();
    // Blend channels per node, in morph target order, for baking their weights.
    let mut node_blend_channels: Vec<(usize, Vec<&ufbx::BlendChannel>)> = Vec::new();
    for node in &ordered {
        let Some(mesh) = node.mesh.as_ref() else { continue };
        let idx = node_index[&node.element.typed_id];
        if counted_meshes.insert(mesh.element.typed_id) {
            vertex_count += mesh.num_vertices;
        }
        let offset = ufbx_matrix(&node.geometry_to_node);
        let world = rest[idx] * offset;

        // Skinning: bones become joints; geometry_to_bone is the inverse bind matrix.
        let skin_deformer = mesh.skin_deformers.first();
        let skin = skin_deformer.and_then(|sd| {
            let mut joints = Vec::new();
            let mut inverse_bind = Vec::new();
            for cluster in sd.clusters.iter() {
                let bone = cluster.bone_node.as_ref().and_then(|b| node_index.get(&b.element.typed_id))?;
                joints.push(*bone);
                inverse_bind.push(ufbx_matrix(&cluster.geometry_to_bone));
            }
            anim.skins.push(Skin { joints, inverse_bind });
            Some(anim.skins.len() - 1)
        });

        // Blend shapes: one morph target per channel (its final shape).
        let channels: Vec<&ufbx::BlendChannel> = mesh
            .blend_deformers
            .iter()
            .flat_map(|bd| bd.channels.iter().map(|c| &**c))
            .filter(|c| c.target_shape.is_some() || !c.keyframes.is_empty())
            .collect();
        if !channels.is_empty() {
            anim.nodes[idx].weights = channels.iter().map(|c| c.weight as f32).collect();
            node_blend_channels.push((idx, channels.clone()));
        }

        let parts = mesh.material_parts.len();
        let mut tri = vec![0u32; mesh.max_face_triangles * 3];
        for part in mesh.material_parts.iter() {
            if part.num_triangles == 0 {
                continue;
            }
            let pi = part.index as usize;
            let mat = node
                .materials
                .get(pi)
                .or_else(|| mesh.materials.get(pi))
                .and_then(|m| material_index.get(&m.element.typed_id).copied())
                .unwrap_or(default_material);

            let has_n = mesh.vertex_normal.exists;
            let has_uv = mesh.vertex_uv.exists;
            let has_c = mesh.vertex_color.exists;
            let mut map: HashMap<[u32; 4], u32> = HashMap::new();
            let (mut positions, mut normals, mut uvs, mut colors) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            // Control point of each output vertex, for skin weights and blend shapes.
            let mut control_points: Vec<u32> = Vec::new();
            let mut indices = Vec::with_capacity(part.num_triangles * 3);
            for &fi in part.face_indices.iter() {
                let face = mesh.faces[fi as usize];
                let n = mesh.triangulate_face(&mut tri, face) as usize;
                for &ix in &tri[..n * 3] {
                    let ix = ix as usize;
                    let key = [
                        mesh.vertex_position.indices[ix],
                        if has_n { mesh.vertex_normal.indices[ix] } else { 0 },
                        if has_uv { mesh.vertex_uv.indices[ix] } else { 0 },
                        if has_c { mesh.vertex_color.indices[ix] } else { 0 },
                    ];
                    let next = positions.len() as u32;
                    let v = *map.entry(key).or_insert_with(|| {
                        let p = mesh.vertex_position.values[key[0] as usize];
                        positions.push([p.x as f32, p.y as f32, p.z as f32]);
                        control_points.push(mesh.vertex_indices[ix]);
                        if has_n {
                            let q = mesh.vertex_normal.values[key[1] as usize];
                            normals.push([q.x as f32, q.y as f32, q.z as f32]);
                        }
                        if has_uv {
                            let t = mesh.vertex_uv.values[key[2] as usize];
                            // FBX/OBJ UVs start at the bottom; images are stored top row first.
                            uvs.push([t.x as f32, 1.0 - t.y as f32]);
                        }
                        if has_c {
                            let c = mesh.vertex_color.values[key[3] as usize];
                            colors.push([c.x as f32, c.y as f32, c.z as f32, c.w as f32]);
                        }
                        next
                    });
                    indices.push(v);
                }
            }

            let (joints, weights) = match (skin, skin_deformer) {
                (Some(_), Some(sd)) => {
                    let mut joints = Vec::with_capacity(control_points.len());
                    let mut weights = Vec::with_capacity(control_points.len());
                    for &cp in &control_points {
                        let sv = &sd.vertices[cp as usize];
                        let mut w: Vec<(u32, f32)> = (0..sv.num_weights)
                            .map(|k| {
                                let sw = &sd.weights[(sv.weight_begin + k) as usize];
                                (sw.cluster_index, sw.weight as f32)
                            })
                            .collect();
                        w.sort_by(|a, b| b.1.total_cmp(&a.1));
                        w.truncate(4);
                        let total: f32 = w.iter().map(|x| x.1).sum::<f32>().max(1e-8);
                        let mut j = [0u16; 4];
                        let mut ww = [0f32; 4];
                        for (k, (c, x)) in w.into_iter().enumerate() {
                            j[k] = c as u16;
                            ww[k] = x / total;
                        }
                        joints.push(j);
                        weights.push(ww);
                    }
                    (Some(joints), Some(weights))
                }
                _ => (None, None),
            };

            let morph_targets = channels
                .iter()
                .map(|c| {
                    let shape = c.target_shape.as_ref().or_else(|| c.keyframes.last().map(|k| &k.shape));
                    let mut by_cp: HashMap<u32, (ufbx::Vec3, Option<ufbx::Vec3>)> = HashMap::new();
                    if let Some(shape) = shape {
                        for (k, &cp) in shape.offset_vertices.iter().enumerate() {
                            by_cp.insert(cp, (shape.position_offsets[k], shape.normal_offsets.get(k).copied()));
                        }
                    }
                    let delta = |cp: &u32| by_cp.get(cp);
                    MorphTarget {
                        name: c.element.name.to_string(),
                        positions: control_points
                            .iter()
                            .map(|cp| delta(cp).map_or([0.0; 3], |(p, _)| [p.x as f32, p.y as f32, p.z as f32]))
                            .collect(),
                        normals: None,
                    }
                })
                .collect();

            let name = if parts > 1 {
                format!("{}.{:03}", node.element.name, pi)
            } else {
                node.element.name.to_string()
            };
            let needs_tangents = materials[mat].normal_tex.is_some();
            let skinned = skin.is_some();
            let mesh_out = Mesh::build(
                MeshData {
                    name,
                    positions,
                    normals: has_n.then_some(normals),
                    uvs: has_uv.then_some(uvs),
                    tangents: None,
                    colors: has_c.then_some(colors),
                    indices,
                    transform: if skinned { Mat4::IDENTITY } else { world },
                    material: mat,
                    rig: MeshRig {
                        node: Some(idx),
                        offset,
                        skin,
                        joints,
                        weights,
                        morph_targets,
                    },
                },
                needs_tangents,
            );
            meshes.push(finish_mesh(mesh_out, &anim, &rest));
        }
    }

    // Animation: bake every stack at the file's frame rate. ufbx evaluates FBX's curve types,
    // rotation orders and pivots; the result is plain TRS keys.
    let fps = if scene.settings.frames_per_second > 0.0 { scene.settings.frames_per_second as f32 } else { 30.0 };
    anim.fps = fps;
    for stack in scene.anim_stacks.iter() {
        let (t0, t1) = (stack.time_begin, stack.time_end);
        if t1 <= t0 {
            continue;
        }
        let frames = (((t1 - t0) * fps as f64).round() as usize).clamp(1, 100_000);
        let times: Vec<f32> = (0..=frames).map(|f| f as f32 / fps).collect();
        let mut channels = Vec::new();
        for (idx, node) in ordered.iter().enumerate() {
            let (mut tr, mut ro, mut sc) = (Vec::new(), Vec::new(), Vec::new());
            for f in 0..=frames {
                let t = ufbx::evaluate_transform(&stack.anim, node, t0 + f as f64 / fps as f64);
                let q = ufbx_quat(&t.rotation);
                tr.extend_from_slice(&ufbx_vec3(&t.translation).to_array());
                ro.extend_from_slice(&q.to_array());
                sc.extend_from_slice(&ufbx_vec3(&t.scale).to_array());
            }
            for (property, values, width) in [(Property::Translation, tr, 3), (Property::Rotation, ro, 4), (Property::Scale, sc, 3)] {
                if varies(&values, width) {
                    channels.push(Channel { node: idx, property, interpolation: Interpolation::Linear, times: times.clone(), values, width });
                }
            }
        }
        for (idx, blend) in &node_blend_channels {
            let mut values = Vec::with_capacity((frames + 1) * blend.len());
            for f in 0..=frames {
                for c in blend {
                    values.push(ufbx::evaluate_blend_weight(&stack.anim, c, t0 + f as f64 / fps as f64) as f32);
                }
            }
            if varies(&values, blend.len()) {
                channels.push(Channel {
                    node: *idx,
                    property: Property::Weights,
                    interpolation: Interpolation::Linear,
                    times: times.clone(),
                    values,
                    width: blend.len(),
                });
            }
        }
        if !channels.is_empty() {
            anim.clips.push(Clip {
                name: stack.element.name.to_string(),
                start: 0.0,
                duration: (t1 - t0) as f32,
                channels,
            });
        }
    }

    let images = decode_images(jobs, &mut warnings);
    let mut result = Scene::new(meshes, materials, images, vertex_count);
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("fbx")) {
        result.units = crate::scene::Units::Declared(scene.settings.original_unit_meters as f64);
    }
    result.warnings = warnings;
    result.animation = anim;
    Ok(result)
}

/// True when any key differs from the first one (static channels are dropped).
fn varies(values: &[f32], width: usize) -> bool {
    let first = &values[..width.min(values.len())];
    values.chunks_exact(width).any(|k| k.iter().zip(first).any(|(a, b)| (a - b).abs() > 1e-5))
}

fn ufbx_vec3(v: &ufbx::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

fn ufbx_quat(q: &ufbx::Quat) -> Quat {
    Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32).normalize()
}

fn map_texture(map: &ufbx::MaterialMap) -> Option<&ufbx::Texture> {
    map.texture.as_deref()
}

fn ufbx_matrix(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols_array(&[
        m.m00 as f32, m.m10 as f32, m.m20 as f32, 0.0,
        m.m01 as f32, m.m11 as f32, m.m21 as f32, 0.0,
        m.m02 as f32, m.m12 as f32, m.m22 as f32, 0.0,
        m.m03 as f32, m.m13 as f32, m.m23 as f32, 1.0,
    ])
}

fn file_name_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

// ---------------------------------------------------------------------------------------------
// STL

fn load_stl(path: &Path) -> Result<Scene, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(file);
    let stl = stl_io::read_stl(&mut reader).map_err(|e| format!("Invalid STL: {e}"))?;

    // STL is faceted by nature (CAD, 3D printing): split vertices for flat shading.
    let mut positions = Vec::with_capacity(stl.faces.len() * 3);
    let mut normals = Vec::with_capacity(stl.faces.len() * 3);
    // Maps each shared STL vertex to one of its split copies, to keep wireframe edges unique.
    let mut first_copy = vec![u32::MAX; stl.vertices.len()];
    let mut shared_indices = Vec::with_capacity(stl.faces.len() * 3);
    for face in &stl.faces {
        let v = face.vertices.map(|i| Vec3::from(stl.vertices[i].0));
        let n = (v[1] - v[0]).cross(v[2] - v[0]).normalize_or(Vec3::Z).to_array();
        for (k, p) in v.iter().enumerate() {
            let split = positions.len() as u32;
            let src = face.vertices[k];
            if first_copy[src] == u32::MAX {
                first_copy[src] = split;
            }
            shared_indices.push(first_copy[src]);
            positions.push(p.to_array());
            normals.push(n);
        }
    }
    let indices: Vec<u32> = (0..positions.len() as u32).collect();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("STL").to_string();
    let mut mesh = Mesh::build(
        MeshData {
            name,
            positions,
            normals: Some(normals),
            uvs: None,
            tangents: None,
            colors: None,
            indices,
            transform: Mat4::IDENTITY,
            material: 0,
            rig: MeshRig::default(),
        },
        false,
    );
    mesh.edges = unique_edges(&shared_indices);

    Ok(Scene::new(vec![mesh], Vec::new(), Vec::new(), stl.vertices.len()))
}

// ---------------------------------------------------------------------------------------------
// Environment maps

/// Linear RGB float pixels of an equirectangular environment.
pub struct EnvImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

pub fn load_environment(path: &Path) -> Result<EnvImage, String> {
    let img = image::open(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let mut rgb = img.to_rgb32f();
    // Keep GPU memory and prefiltering time reasonable for 8K/16K HDRIs.
    const MAX_WIDTH: u32 = 4096;
    if rgb.width() > MAX_WIDTH {
        let h = (rgb.height() as u64 * MAX_WIDTH as u64 / rgb.width() as u64) as u32;
        rgb = image::imageops::resize(&rgb, MAX_WIDTH, h.max(1), image::imageops::FilterType::Triangle);
    }
    let (width, height) = rgb.dimensions();
    let pixels = rgb.pixels().map(|p| [p[0], p[1], p[2], 1.0]).collect();
    Ok(EnvImage { width, height, pixels })
}

