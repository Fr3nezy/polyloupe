//! Explorer thumbnail handlers (IThumbnailProvider) for 3D model files. Two classes:
//!
//! - Stream handler, for single-file formats (.glb .fbx .stl .ply .3mf .dae). Self-contained,
//!   like Blender's `.blend` handler: the DLL parses the model and renders it on the CPU
//!   (`polyloupe_core::thumbnail`), with no GPU, helper process, socket or server. That lets
//!   Windows run it isolated in its thumbnail process (`dllhost`), which only hands the handler a
//!   stream of the file's bytes: the same path for every drive and every caller.
//! - Path handler, for formats that keep data in files next to the model (.gltf with external
//!   buffers and textures, .obj with its .mtl). Those need the file's path, which Windows only
//!   gives a handler that opts out of isolation (`DisableProcessIsolation`), so it runs inside
//!   Explorer. Like F3D's handler, it never parses the model there: it runs
//!   `polyloupe.exe --thumbnail` (the same CPU renderer, no window) with a timeout and reads back
//!   the PNG, so a bad file can only take down that child process. It implements no stream
//!   initializer on purpose: Explorer prefers the stream whenever a handler offers one.
//!
//! Registration:
//! - all users (HKLM, admin, what the installer does): `regsvr32 /n /i:allusers`, and
//!   `regsvr32 /u /n /i:allusers` to remove.
//! - per user (HKCU, no admin rights): `regsvr32 polyloupe_thumbs.dll`, `regsvr32 /u` to remove.
//!
//! Windows can keep resolving thumbnail handlers through registrations that are gone from the
//! registry: on a development machine, Explorer and the thumbnail process kept asking for an older
//! per-user CLSID and DLL path across reboots. So this handler has a CLSID of its own, registration
//! clears the CLSIDs earlier builds used, and the DLL still answers to those in case Windows
//! remembers one.

#![allow(non_snake_case)]
// MSVC reports creating the import library for the COM exports; that is expected.
#![allow(linker_messages)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_UNEXPECTED, HINSTANCE, HMODULE,
    S_FALSE, S_OK,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{
    CoTaskMemFree, IClassFactory, IClassFactory_Impl, IStream, STATFLAG_DEFAULT, STATSTG,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW,
    RegGetValueW, RegSetValueExW,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithFile, IInitializeWithFile_Impl, IInitializeWithStream,
    IInitializeWithStream_Impl,
};
use windows::Win32::UI::Shell::{
    IInitializeWithItem, IInitializeWithItem_Impl, IShellItem, IThumbnailProvider,
    IThumbnailProvider_Impl, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, SIGDN_FILESYSPATH,
    WTS_ALPHATYPE, WTSAT_ARGB,
};
use windows_core::{BOOL, GUID, HRESULT, IUnknown, Interface, PCWSTR, Ref, Result, implement};

const CLSID_THUMBNAILER: GUID = GUID::from_u128(0x0e196db0_b2ec_4fbc_b2e4_5fd1b0bf39df);
const CLSID_STRING: &str = "{0e196db0-b2ec-4fbc-b2e4-5fd1b0bf39df}";
/// CLSIDs of earlier builds (isolated, and in-process with DisableProcessIsolation).
const LEGACY_CLSIDS: [(GUID, &str); 2] = [
    (GUID::from_u128(0x1fcd9853_22d9_46b7_a5c0_cba9444584c9), "{1fcd9853-22d9-46b7-a5c0-cba9444584c9}"),
    (GUID::from_u128(0x1db9db9f_86bc_47bb_88ed_99c5551e49f1), "{1db9db9f-86bc-47bb-88ed-99c5551e49f1}"),
];
const CLSID_PATH_THUMBNAILER: GUID = GUID::from_u128(0xdc5c1405_f891_448f_8713_3de1d48c455c);
const CLSID_PATH_STRING: &str = "{dc5c1405-f891-448f-8713-3de1d48c455c}";
/// The shell's IThumbnailProvider handler category.
const THUMBNAIL_HANDLER_KEY: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
const EXTENSIONS: &[&str] = &[".glb", ".gltf", ".fbx", ".obj", ".stl", ".ply", ".3mf", ".dae"];
/// Formats whose data can live in other files: served by the path handler.
const PATH_EXTENSIONS: &[&str] = &[".gltf", ".obj"];
/// How long the path handler waits for `polyloupe.exe --thumbnail` before killing it.
const VIEWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

static MODULE: AtomicIsize = AtomicIsize::new(0);
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0 as isize, Ordering::Relaxed);
    }
    BOOL(1)
}

/// Appends to `%LOCALAPPDATA%\PolyLoupe\thumbs.log` when a `thumbs-debug` file exists next to
/// it, so a user can trace what Explorer asks for without a debug build.
fn log(message: impl FnOnce() -> String) {
    let Some(dir) = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("PolyLoupe")) else { return };
    if !dir.join("thumbs-debug").exists() {
        return;
    }
    use std::io::Write;
    let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("thumbs.log")) {
        let _ = writeln!(f, "{secs:.3} {exe}:{} {}", std::process::id(), message());
    }
}

fn module_path() -> Option<PathBuf> {
    let module = HMODULE(MODULE.load(Ordering::Relaxed) as *mut c_void);
    let mut buf = vec![0u16; 1024];
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

// --- COM objects -------------------------------------------------------------------------------

#[implement(IClassFactory)]
struct Factory {
    path_based: bool,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let provider: IUnknown = if self.path_based {
            PathThumbnailProvider::default().into()
        } else {
            ThumbnailProvider::default().into()
        };
        unsafe { provider.query(riid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// What the shell handed us.
enum Source {
    Path(PathBuf),
    Stream(IStream),
}

/// Explorer's thumbnail process only ever uses the stream; the path initializers serve other
/// hosts (and the probe example).
#[implement(
    IThumbnailProvider,
    IInitializeWithStream,
    IInitializeWithItem,
    IInitializeWithFile
)]
#[derive(Default)]
struct ThumbnailProvider {
    source: Mutex<Option<Source>>,
}

impl ThumbnailProvider_Impl {
    fn store(&self, source: Source) -> Result<()> {
        *self.source.lock().map_err(|_| windows_core::Error::from(E_FAIL))? = Some(source);
        Ok(())
    }
}

impl IInitializeWithStream_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> Result<()> {
        self.store(Source::Stream(stream.ok()?.clone()))
    }
}

impl IInitializeWithFile_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, path: &PCWSTR, _mode: u32) -> Result<()> {
        let path = unsafe { path.to_string() }.map_err(|_| windows_core::Error::from(E_FAIL))?;
        self.store(Source::Path(PathBuf::from(path)))
    }
}

impl IInitializeWithItem_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, item: Ref<IShellItem>, _mode: u32) -> Result<()> {
        let name = unsafe { item.ok()?.GetDisplayName(SIGDN_FILESYSPATH)? };
        let path = unsafe { name.to_string() };
        unsafe { CoTaskMemFree(Some(name.0 as *const c_void)) };
        let path = path.map_err(|_| windows_core::Error::from(E_FAIL))?;
        self.store(Source::Path(PathBuf::from(path)))
    }
}

impl IThumbnailProvider_Impl for ThumbnailProvider_Impl {
    fn GetThumbnail(&self, cx: u32, bitmap: *mut HBITMAP, alpha: *mut WTS_ALPHATYPE) -> Result<()> {
        let started = std::time::Instant::now();
        let guard = self.source.lock().map_err(|_| windows_core::Error::from(E_FAIL))?;
        let size = cx.clamp(16, 1024);
        let (name, rendered) = match guard.as_ref().ok_or(windows_core::Error::from(E_UNEXPECTED))? {
            Source::Path(path) => (path.display().to_string(), render_file(path, size)),
            Source::Stream(stream) => render_stream(stream, size),
        };
        log(|| match &rendered {
            Ok(_) => format!("{size}px ok in {} ms: {name}", started.elapsed().as_millis()),
            Err(e) => format!("{size}px FAILED in {} ms: {name}: {e}", started.elapsed().as_millis()),
        });
        let rgba = rendered.map_err(|_| windows_core::Error::from(E_FAIL))?;
        let hbitmap = to_hbitmap(size, size, &rgba)?;
        unsafe {
            *bitmap = hbitmap;
            *alpha = WTSAT_ARGB;
        }
        Ok(())
    }
}

/// The path handler: runs inside Explorer, so all it does there is start the viewer.
#[implement(IThumbnailProvider, IInitializeWithItem, IInitializeWithFile)]
#[derive(Default)]
struct PathThumbnailProvider {
    path: Mutex<Option<PathBuf>>,
}

impl PathThumbnailProvider_Impl {
    fn store(&self, path: PathBuf) -> Result<()> {
        *self.path.lock().map_err(|_| windows_core::Error::from(E_FAIL))? = Some(path);
        Ok(())
    }
}

impl IInitializeWithFile_Impl for PathThumbnailProvider_Impl {
    fn Initialize(&self, path: &PCWSTR, _mode: u32) -> Result<()> {
        let path = unsafe { path.to_string() }.map_err(|_| windows_core::Error::from(E_FAIL))?;
        self.store(PathBuf::from(path))
    }
}

impl IInitializeWithItem_Impl for PathThumbnailProvider_Impl {
    fn Initialize(&self, item: Ref<IShellItem>, _mode: u32) -> Result<()> {
        let name = unsafe { item.ok()?.GetDisplayName(SIGDN_FILESYSPATH)? };
        let path = unsafe { name.to_string() };
        unsafe { CoTaskMemFree(Some(name.0 as *const c_void)) };
        self.store(PathBuf::from(path.map_err(|_| windows_core::Error::from(E_FAIL))?))
    }
}

impl IThumbnailProvider_Impl for PathThumbnailProvider_Impl {
    fn GetThumbnail(&self, cx: u32, bitmap: *mut HBITMAP, alpha: *mut WTS_ALPHATYPE) -> Result<()> {
        let started = std::time::Instant::now();
        let path = self.path.lock().map_err(|_| windows_core::Error::from(E_FAIL))?.clone();
        let path = path.ok_or(windows_core::Error::from(E_UNEXPECTED))?;
        let size = cx.clamp(16, 1024);
        let rendered = render_with_viewer(&path, size);
        log(|| match &rendered {
            Ok(_) => format!("{size}px ok via polyloupe.exe in {} ms: {}", started.elapsed().as_millis(), path.display()),
            Err(e) => format!("{size}px FAILED via polyloupe.exe in {} ms: {}: {e}", started.elapsed().as_millis(), path.display()),
        });
        let (width, height, rgba) = rendered.map_err(|_| windows_core::Error::from(E_FAIL))?;
        let hbitmap = to_hbitmap(width, height, &rgba)?;
        unsafe {
            *bitmap = hbitmap;
            *alpha = WTSAT_ARGB;
        }
        Ok(())
    }
}

/// `polyloupe.exe` next to this DLL (the install folder), else wherever the installer registered it.
fn viewer_exe() -> Option<PathBuf> {
    if let Some(beside) = module_path().and_then(|p| p.parent().map(|d| d.join("polyloupe.exe"))) {
        if beside.is_file() {
            return Some(beside);
        }
    }
    let command = get_value(HKEY_LOCAL_MACHINE, "Software\\Classes\\Applications\\polyloupe.exe\\shell\\open\\command")?;
    let exe = PathBuf::from(command.trim_start().strip_prefix('"')?.split('"').next()?);
    exe.is_file().then_some(exe)
}

/// Renders `path` with `polyloupe.exe --thumbnail` in a child process, which can read the files
/// next to the model. The child is killed if it outlives `VIEWER_TIMEOUT`.
fn render_with_viewer(path: &Path, size: u32) -> std::result::Result<(u32, u32, Vec<u8>), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let exe = viewer_exe().ok_or("polyloupe.exe not found")?;
    let out = std::env::temp_dir().join(format!(
        "polyloupe-thumb-{}-{}.png",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut child = Command::new(&exe)
        .arg("--thumbnail")
        .arg(path)
        .arg(&out)
        .arg(size.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", exe.display()))?;
    let deadline = std::time::Instant::now() + VIEWER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&out);
                return Err("polyloupe.exe timed out".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let image = if status.success() {
        image::open(&out).map_err(|e| format!("couldn't read the thumbnail: {e}"))
    } else {
        Err(format!("polyloupe.exe failed ({status})"))
    };
    let _ = std::fs::remove_file(&out);
    let rgba = image?.to_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// Loads and renders a model. Parsers and the renderer run under `catch_unwind`: a malformed file
/// must fail the thumbnail, not take the host process down.
fn render_file(path: &Path, size: u32) -> std::result::Result<Vec<u8>, String> {
    std::panic::catch_unwind(|| {
        let scene = polyloupe_core::loader::load(path)?;
        polyloupe_core::thumbnail::render(&scene, size).ok_or_else(|| "nothing to draw".to_string())
    })
    .unwrap_or_else(|_| Err("the loader panicked".into()))
}

/// Renders from the stream Explorer's thumbnail process hands over. It only names the file (no
/// folder), so the bytes go to a temporary copy with the same name for the loaders. Files next to
/// the model are out of reach from there: an .obj renders without its .mtl, and a .gltf with
/// external buffers keeps the normal icon.
fn render_stream(stream: &IStream, size: u32) -> (String, std::result::Result<Vec<u8>, String>) {
    let mut stat = STATSTG::default();
    if let Err(e) = unsafe { stream.Stat(&mut stat, STATFLAG_DEFAULT) } {
        return ("stream".into(), Err(e.to_string()));
    }
    let name = (!stat.pwcsName.is_null()).then(|| unsafe { stat.pwcsName.to_string() }.ok()).flatten();
    unsafe { CoTaskMemFree(Some(stat.pwcsName.0 as *const c_void)) };
    let Some(name) = name else { return ("stream".into(), Err("the stream has no name".into())) };
    let path = PathBuf::from(&name);
    if path.is_absolute() && path.is_file() {
        return (name, render_file(&path, size));
    }
    let Some(file_name) = path.file_name().map(|n| n.to_owned()) else {
        return (name, Err("the stream has no file name".into()));
    };

    let mut bytes = Vec::with_capacity(stat.cbSize as usize);
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let mut read = 0u32;
        let hr = unsafe { stream.Read(chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, Some(&mut read)) };
        if hr.is_err() {
            return (name, Err(format!("reading the stream failed: {hr:?}")));
        }
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read as usize]);
    }
    let dir = std::env::temp_dir().join(format!(
        "polyloupe-thumb-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let copy = dir.join(file_name);
    let result = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&copy, &bytes))
        .map_err(|e| format!("couldn't write {}: {e}", copy.display()))
        .and_then(|_| render_file(&copy, size));
    let _ = std::fs::remove_dir_all(&dir);
    (name, result)
}

/// Top-down 32-bit DIB with premultiplied BGRA, as Explorer expects for WTSAT_ARGB.
fn to_hbitmap(width: u32, height: u32, rgba: &[u8]) -> Result<HBITMAP> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let hbitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    if bits.is_null() {
        return Err(E_FAIL.into());
    }
    let dst =
        unsafe { std::slice::from_raw_parts_mut(bits as *mut u8, (width * height * 4) as usize) };
    for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = (s[2] as u32 * a / 255) as u8;
        d[1] = (s[1] as u32 * a / 255) as u8;
        d[2] = (s[0] as u32 * a / 255) as u8;
        d[3] = s[3];
    }
    Ok(hbitmap)
}

// --- DLL exports -------------------------------------------------------------------------------

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    riid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    if clsid.is_null() || object.is_null() {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let clsid = unsafe { *clsid };
    let path_based = clsid == CLSID_PATH_THUMBNAILER;
    if !path_based && clsid != CLSID_THUMBNAILER && !LEGACY_CLSIDS.iter().any(|(g, _)| *g == clsid) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory { path_based }.into();
    unsafe { factory.query(riid, object) }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // Kept loaded for the (short) life of the thumbnail process.
    S_FALSE
}

#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    finish(register(HKEY_CURRENT_USER))
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    unregister(HKEY_CURRENT_USER);
    finish(Ok(()))
}

/// `regsvr32 [/u] /n /i:allusers`: machine-wide registration, used by the installer.
#[unsafe(no_mangle)]
extern "system" fn DllInstall(install: BOOL, cmdline: PCWSTR) -> HRESULT {
    let cmdline = if cmdline.is_null() {
        String::new()
    } else {
        unsafe { cmdline.to_string() }.unwrap_or_default()
    };
    let all_users = cmdline.trim().eq_ignore_ascii_case("allusers");
    let root = if all_users { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
    if all_users {
        // Per-user leftovers (a development registration) would shadow the machine-wide one.
        unregister(HKEY_CURRENT_USER);
    }
    if install.as_bool() {
        finish(register(root))
    } else {
        unregister(root);
        finish(Ok(()))
    }
}

fn finish(result: Result<()>) -> HRESULT {
    match result {
        Ok(()) => {
            unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
            S_OK
        }
        Err(e) => e.code(),
    }
}

const CLASSES: &str = "Software\\Classes";

fn register(root: HKEY) -> Result<()> {
    unregister(root);
    let dll = module_path().ok_or(windows_core::Error::from(E_FAIL))?;
    for (clsid, name) in [
        (CLSID_STRING, "PolyLoupe Thumbnail Provider"),
        (CLSID_PATH_STRING, "PolyLoupe Thumbnail Provider (files next to the model)"),
    ] {
        let key = format!("{CLASSES}\\CLSID\\{clsid}");
        set_value(root, &key, None, name)?;
        let server = format!("{key}\\InprocServer32");
        set_value(root, &server, None, &dll.to_string_lossy())?;
        set_value(root, &server, Some("ThreadingModel"), "Apartment")?;
    }
    // The only way Windows hands a thumbnail handler the file's path (see the module docs).
    let path_key = format!("{CLASSES}\\CLSID\\{CLSID_PATH_STRING}");
    write_value(root, &path_key, Some("DisableProcessIsolation"), REG_DWORD, &1u32.to_le_bytes())?;
    for ext in EXTENSIONS {
        let clsid = if PATH_EXTENSIONS.contains(ext) { CLSID_PATH_STRING } else { CLSID_STRING };
        set_value(root, &handler_key(ext), None, clsid)?;
    }
    Ok(())
}

/// Removes this handler and the ones older builds registered. An extension's thumbnail handler
/// entry is only removed when it points at one of ours; a `ShellEx` key left empty goes too, since
/// an empty per-user one can hide the machine-wide handler from Explorer.
fn unregister(root: HKEY) {
    let ours: Vec<&str> = LEGACY_CLSIDS.iter().map(|(_, s)| *s).chain([CLSID_STRING, CLSID_PATH_STRING]).collect();
    for ext in EXTENSIONS {
        let key = handler_key(ext);
        if get_value(root, &key).is_some_and(|v| ours.iter().any(|c| c.eq_ignore_ascii_case(&v))) {
            let _ = delete_tree(root, &key);
        }
        // RegDeleteKeyW refuses keys that still have subkeys: only an empty ShellEx is removed.
        let shellex = wide(&format!("{CLASSES}\\{ext}\\ShellEx"));
        let _ = unsafe { RegDeleteKeyW(root, PCWSTR(shellex.as_ptr())) };
    }
    for clsid in ours {
        let _ = delete_tree(root, &format!("{CLASSES}\\CLSID\\{clsid}"));
    }
}

fn handler_key(ext: &str) -> String {
    format!("{CLASSES}\\{ext}\\ShellEx\\{THUMBNAIL_HANDLER_KEY}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get_value(root: HKEY, key: &str) -> Option<String> {
    let key_w = wide(key);
    let mut buf = vec![0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            root,
            PCWSTR(key_w.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut len),
        )
        .ok()
        .ok()?;
    }
    let chars = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..chars.min(buf.len())]))
}

fn set_value(root: HKEY, key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
    write_value(root, key, name, REG_SZ, &data)
}

fn write_value(
    root: HKEY,
    key: &str,
    name: Option<&str>,
    kind: REG_VALUE_TYPE,
    data: &[u8],
) -> Result<()> {
    let key_w = wide(key);
    let mut hkey = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(key_w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()?;
    }
    let name_w = name.map(wide);
    let result = unsafe {
        RegSetValueExW(
            hkey,
            name_w
                .as_ref()
                .map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            None,
            kind,
            Some(data),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    result.ok()
}

fn delete_tree(root: HKEY, key: &str) -> Result<()> {
    let key_w = wide(key);
    unsafe { RegDeleteTreeW(root, PCWSTR(key_w.as_ptr())).ok() }
}
