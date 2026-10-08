//! STEP (.step/.stp) through `polyloupe_step.dll`, which wraps OpenCASCADE.
//!
//! The DLL sits next to the executable and is loaded on the first STEP file only, so the CAD
//! kernel never slows down startup or other formats. It stays loaded afterwards.
//!
//! Reading a big STEP takes seconds (OpenCASCADE builds the exact geometry before meshing it),
//! so the meshed result is cached per file in `%LOCALAPPDATA%\PolyLoupe\cache\step`: opening
//! the same file again, or its Explorer thumbnail, skips OpenCASCADE.

use std::path::Path;
use std::sync::OnceLock;

use glam::Mat4;

use crate::scene::{Material, Mesh, MeshData, MeshRig, Scene};

const DLL_NAME: &str = "polyloupe_step.dll";
const ABI: u32 = 2;

type LoadFn = unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32;
type FreeFn = unsafe extern "C" fn(*mut u8, usize);

struct Api {
    load: LoadFn,
    free: FreeFn,
    _lib: libloading::Library,
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let dir = std::env::current_exe().map_err(|e| e.to_string())?;
        let path = dir.with_file_name(DLL_NAME);
        // SAFETY: our own DLL, built from this workspace; its initializers only set up OpenCASCADE.
        unsafe {
            let lib = libloading::Library::new(&path).map_err(|e| format!("STEP support is missing ({DLL_NAME}: {e})"))?;
            let abi: libloading::Symbol<unsafe extern "C" fn() -> u32> =
                lib.get(b"polyloupe_step_abi").map_err(|e| e.to_string())?;
            if abi() != ABI {
                return Err(format!("{DLL_NAME} doesn't match this PolyLoupe version"));
            }
            let load = *lib.get::<LoadFn>(b"polyloupe_step_load").map_err(|e| e.to_string())?;
            let free = *lib.get::<FreeFn>(b"polyloupe_step_free").map_err(|e| e.to_string())?;
            Ok(Api { load, free, _lib: lib })
        }
    })
    .as_ref()
    .map_err(Clone::clone)
}

pub fn load(path: &Path) -> Result<Scene, String> {
    let cache = cache_file(path);
    if let Some(bytes) = cache.as_ref().and_then(|c| std::fs::read(c).ok()) {
        if let Some(scene) = parse(&bytes, path) {
            // Recently used entries survive the size cap.
            if let Ok(f) = std::fs::File::options().write(true).open(cache.as_ref().unwrap()) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            return Ok(scene);
        }
    }
    let api = api()?;
    let path_str = path.to_str().ok_or("the path isn't valid Unicode")?;
    let mut ptr = std::ptr::null_mut();
    let mut len = 0usize;
    // SAFETY: the DLL fills ptr/len with a buffer we hand back to its own free function.
    let status = unsafe { (api.load)(path_str.as_ptr(), path_str.len(), &mut ptr, &mut len) };
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { (api.free)(ptr, len) };
    if status != 0 {
        return Err(String::from_utf8_lossy(&bytes).into_owned());
    }
    let scene = parse(&bytes, path).ok_or("the STEP reader returned malformed data")?;
    if let Some(cache) = cache {
        store(&cache, &bytes);
    }
    Ok(scene)
}

/// Cache entries never take more than this; the least recently used go first.
const CACHE_LIMIT: u64 = 1 << 30;

fn cache_dir() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("PolyLoupe").join("cache").join("step"))
}

/// The cache entry for `path`: keyed by the file (path, size, modification time) and by the
/// reader DLL, so a new PolyLoupe version meshes again.
fn cache_file(path: &Path) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let stamp = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()));
    let dll = std::env::current_exe().ok()?.with_file_name(DLL_NAME);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (std::fs::canonicalize(path).ok()?, stamp(path)?, stamp(&dll)?, ABI).hash(&mut h);
    Some(cache_dir()?.join(format!("{:016x}.mesh", h.finish())))
}

fn store(file: &Path, bytes: &[u8]) {
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // A file cut short (crash, full disk) fails to parse and is simply read again from the STEP.
    if std::fs::write(file, bytes).is_err() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= CACHE_LIMIT {
            break;
        }
        if path != file && std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

fn parse(bytes: &[u8], path: &Path) -> Option<Scene> {
    let mut r = Reader { bytes, pos: 0 };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("STEP");
    let parts = r.u32()? as usize;
    let mut meshes = Vec::with_capacity(parts);
    let mut materials: Vec<Material> = Vec::new();
    let mut vertex_total = 0;
    for i in 0..parts {
        let name_len = r.u32()? as usize;
        let part_name = String::from_utf8_lossy(r.bytes(name_len)?).into_owned();
        let rgba = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
        let material = match materials.iter().position(|m| m.base_color == rgba) {
            Some(m) => m,
            None => {
                let name = format!("Color {}", materials.len() + 1);
                materials.push(Material { name, base_color: rgba, ..Material::default() });
                materials.len() - 1
            }
        };
        let n = r.u32()? as usize;
        let positions = r.vec3s(n)?;
        let normals = r.vec3s(n)?;
        let ni = r.u32()? as usize;
        let indices: Vec<u32> = (0..ni).map(|_| r.u32()).collect::<Option<_>>()?;
        if indices.iter().any(|&i| i as usize >= n) {
            return None;
        }
        vertex_total += n;
        let name = match (part_name.is_empty(), parts) {
            (false, _) => part_name,
            (true, 1) => stem.to_string(),
            (true, _) => format!("{stem} {}", i + 1),
        };
        meshes.push(Mesh::build(
            MeshData {
                name,
                positions,
                normals: Some(normals),
                uvs: None,
                tangents: None,
                colors: None,
                indices,
                transform: Mat4::IDENTITY,
                material,
                rig: MeshRig::default(),
            },
            false,
        ));
    }
    Some(Scene::new(meshes, materials, Vec::new(), vertex_total))
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let b = self.bytes.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(b)
    }
    fn take4(&mut self) -> Option<[u8; 4]> {
        let b = self.bytes.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        b.try_into().ok()
    }
    fn u32(&mut self) -> Option<u32> {
        self.take4().map(u32::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take4().map(f32::from_le_bytes)
    }
    fn vec3s(&mut self, n: usize) -> Option<Vec<[f32; 3]>> {
        (0..n).map(|_| Some([self.f32()?, self.f32()?, self.f32()?])).collect()
    }
}
