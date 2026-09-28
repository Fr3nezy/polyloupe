//! Explorer thumbnail handler (IThumbnailProvider) for 3D model files.
//!
//! The DLL stays small and dependency-light: rendering happens in `polyloupe.exe` (installed next
//! to it), normally the warm `--thumbnail-server`, and the DLL only hands Explorer the bitmap.
//! Explorer caches the result, so each file renders once.
//!
//! Registration:
//! - all users (HKLM, admin, what the installer does): `regsvr32 /n /i:allusers`, and
//!   `regsvr32 /u /n /i:allusers` to remove. Every format uses the in-process CLSID: Explorer
//!   passes the real path (no copy of the file, and .gltf/.obj find the files next to them), and
//!   the isolated surrogate failed to load the DLL from Program Files (0x8007007E). Only IPC
//!   runs inside Explorer; the renderer is a separate process.
//! - per user (HKCU, no admin rights): `regsvr32 polyloupe_thumbs.dll`, `regsvr32 /u` to remove.
//!   Windows ignores DisableProcessIsolation there, so Explorer runs the handler isolated and
//!   only hands it the file's bytes.

#![allow(non_snake_case)]
// MSVC reports creating the import library for the COM exports; that is expected.
#![allow(linker_messages)]

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::time::{Duration, Instant};

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
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE,
    REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
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

/// Isolated handler: Explorer runs it in a surrogate process and only gives it a stream.
const CLSID_THUMBNAILER: GUID = GUID::from_u128(0x1fcd9853_22d9_46b7_a5c0_cba9444584c9);
const CLSID_STRING: &str = "{1fcd9853-22d9-46b7-a5c0-cba9444584c9}";
/// The same handler registered with DisableProcessIsolation (all-users install only).
const CLSID_THUMBNAILER_INPROC: GUID = GUID::from_u128(0x1db9db9f_86bc_47bb_88ed_99c5551e49f1);
const CLSID_INPROC_STRING: &str = "{1db9db9f-86bc-47bb-88ed-99c5551e49f1}";
/// The shell's IThumbnailProvider handler category.
const THUMBNAIL_HANDLER_KEY: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
const EXTENSIONS: &[&str] = &[".glb", ".gltf", ".fbx", ".obj", ".stl", ".ply", ".3mf", ".dae"];
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

static MODULE: AtomicIsize = AtomicIsize::new(0);
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0 as isize, Ordering::Relaxed);
    }
    BOOL(1)
}

/// `%LOCALAPPDATA%\Poly Loupe`: the thumbnail server record, and the debug log.
fn app_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Poly Loupe"))
}

/// Appends to `thumbs.log` when a `thumbs-debug` file exists next to it, so a user can trace
/// what Explorer asks for without a debug build.
fn log(message: impl FnOnce() -> String) {
    let Some(dir) = app_dir() else { return };
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
    /// Builds the path-only provider (the DisableProcessIsolation CLSID).
    path_only: bool,
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
        let provider: IUnknown = if self.path_only {
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

type SourceSlot = Mutex<Option<Source>>;

/// Isolated provider. Explorer's surrogate only ever uses the stream; the path initializers are
/// kept for other hosts.
#[implement(
    IThumbnailProvider,
    IInitializeWithStream,
    IInitializeWithItem,
    IInitializeWithFile
)]
#[derive(Default)]
struct ThumbnailProvider {
    source: SourceSlot,
}

/// In-process provider for formats with files next to them. It deliberately has no
/// IInitializeWithStream: the shell prefers a stream whenever a handler offers one, and a stream
/// does not say which folder the model is in.
#[implement(IThumbnailProvider, IInitializeWithItem, IInitializeWithFile)]
#[derive(Default)]
struct PathThumbnailProvider {
    source: SourceSlot,
}

fn store(slot: &SourceSlot, source: Source) -> Result<()> {
    *slot.lock().map_err(|_| windows_core::Error::from(E_FAIL))? = Some(source);
    Ok(())
}

fn path_from_file(path: &PCWSTR) -> Result<Source> {
    let path = unsafe { path.to_string() }.map_err(|_| windows_core::Error::from(E_FAIL))?;
    Ok(Source::Path(PathBuf::from(path)))
}

fn path_from_item(item: Ref<IShellItem>) -> Result<Source> {
    let item = item.ok()?;
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
    let path = unsafe { name.to_string() };
    unsafe { CoTaskMemFree(Some(name.0 as *const c_void)) };
    let path = path.map_err(|_| windows_core::Error::from(E_FAIL))?;
    Ok(Source::Path(PathBuf::from(path)))
}

fn thumbnail(
    slot: &SourceSlot,
    cx: u32,
    bitmap: *mut HBITMAP,
    alpha: *mut WTS_ALPHATYPE,
) -> Result<()> {
    let guard = slot.lock().map_err(|_| windows_core::Error::from(E_FAIL))?;
    let rendered = match guard
        .as_ref()
        .ok_or(windows_core::Error::from(E_UNEXPECTED))?
    {
        Source::Path(path) => render(path, cx),
        Source::Stream(stream) => render_stream(stream, cx),
    };
    log(|| format!("thumbnail {cx}px: {}", if rendered.is_some() { "ok" } else { "FAILED" }));
    let (w, h, rgba) = rendered.ok_or(windows_core::Error::from(E_FAIL))?;
    let hbitmap = to_hbitmap(w, h, &rgba)?;
    unsafe {
        *bitmap = hbitmap;
        *alpha = WTSAT_ARGB;
    }
    Ok(())
}

impl IInitializeWithStream_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> Result<()> {
        log(|| "init stream".into());
        store(&self.source, Source::Stream(stream.ok()?.clone()))
    }
}

impl IInitializeWithFile_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, path: &PCWSTR, _mode: u32) -> Result<()> {
        let source = path_from_file(path)?;
        log(|| format!("init file {}", describe(&source)));
        store(&self.source, source)
    }
}

impl IInitializeWithItem_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, item: Ref<IShellItem>, _mode: u32) -> Result<()> {
        let source = path_from_item(item)?;
        log(|| format!("init item {}", describe(&source)));
        store(&self.source, source)
    }
}

fn describe(source: &Source) -> String {
    match source {
        Source::Path(p) => p.display().to_string(),
        Source::Stream(_) => "stream".into(),
    }
}

impl IThumbnailProvider_Impl for ThumbnailProvider_Impl {
    fn GetThumbnail(&self, cx: u32, bitmap: *mut HBITMAP, alpha: *mut WTS_ALPHATYPE) -> Result<()> {
        thumbnail(&self.source, cx, bitmap, alpha)
    }
}

impl IInitializeWithFile_Impl for PathThumbnailProvider_Impl {
    fn Initialize(&self, path: &PCWSTR, _mode: u32) -> Result<()> {
        let source = path_from_file(path)?;
        log(|| format!("path-only init file {}", describe(&source)));
        store(&self.source, source)
    }
}

impl IInitializeWithItem_Impl for PathThumbnailProvider_Impl {
    fn Initialize(&self, item: Ref<IShellItem>, _mode: u32) -> Result<()> {
        let source = path_from_item(item)?;
        log(|| format!("path-only init item {}", describe(&source)));
        store(&self.source, source)
    }
}

impl IThumbnailProvider_Impl for PathThumbnailProvider_Impl {
    fn GetThumbnail(&self, cx: u32, bitmap: *mut HBITMAP, alpha: *mut WTS_ALPHATYPE) -> Result<()> {
        thumbnail(&self.source, cx, bitmap, alpha)
    }
}

/// Renders from a stream. Explorer's isolated surrogate only reports the bare file name, so the
/// bytes are copied to a temporary file with the same extension. Files next to the model are not
/// reachable from there: .obj renders without its .mtl, and a .gltf with external buffers fails
/// (Explorer then keeps the normal icon). The all-users install avoids this for those formats.
fn render_stream(stream: &IStream, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let mut stat = STATSTG::default();
    unsafe { stream.Stat(&mut stat, STATFLAG_DEFAULT) }.ok()?;
    let name = (!stat.pwcsName.is_null())
        .then(|| unsafe { stat.pwcsName.to_string() }.ok())
        .flatten();
    unsafe { CoTaskMemFree(Some(stat.pwcsName.0 as *const c_void)) };
    let name = PathBuf::from(name?);
    log(|| format!("stream name {:?} absolute={}", name, name.is_absolute()));
    if name.is_absolute() && name.is_file() {
        return render(&name, size);
    }

    let ext = name.extension()?.to_string_lossy().to_lowercase();
    let mut bytes = Vec::with_capacity(stat.cbSize as usize);
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        let mut read = 0u32;
        let hr = unsafe {
            stream.Read(
                chunk.as_mut_ptr() as *mut c_void,
                chunk.len() as u32,
                Some(&mut read),
            )
        };
        if hr.is_err() {
            return None;
        }
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read as usize]);
    }
    let copy = std::env::temp_dir().join(format!(
        "polyloupe-thumb-src-{}-{}.{ext}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&copy, &bytes).ok()?;
    let result = render(&copy, size);
    let _ = std::fs::remove_file(&copy);
    result
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Renders through the warm thumbnail server (`polyloupe --thumbnail-server`), starting it when
/// none is running; falls back to a one-shot `polyloupe --thumbnail` run.
fn render(path: &std::path::Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let exe = module_path()?.parent()?.join("polyloupe.exe");
    match ask_server(path, size) {
        Server::Answered(result) => return result,
        Server::Unreachable => log(|| "server unreachable, starting one".into()),
    }
    // Break away from the surrogate's job so the server outlives this dllhost; not every job
    // allows it, hence the retry.
    let spawn = |flags| {
        use std::process::Stdio;
        // No inherited handles: the server must not keep the caller's pipes open.
        Command::new(&exe)
            .arg("--thumbnail-server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    if spawn(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(CREATE_NO_WINDOW)).is_ok() {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
            if let Server::Answered(result) = ask_server(path, size) {
                return result;
            }
        }
    }
    log(|| "server didn't answer, rendering once".into());
    render_once(&exe, path, size)
}

enum Server {
    /// The server replied: an image, or `None` when the file couldn't be rendered.
    Answered(Option<(u32, u32, Vec<u8>)>),
    Unreachable,
}

fn ask_server(path: &std::path::Path, size: u32) -> Server {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    let record = app_dir().and_then(|d| std::fs::read_to_string(d.join("thumbnail-server")).ok());
    let Some((port, token)) = record.as_deref().and_then(|r| r.trim().split_once(' ')) else {
        return Server::Unreachable;
    };
    let Ok(port) = port.parse::<u16>() else { return Server::Unreachable };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) else {
        return Server::Unreachable;
    };
    let _ = stream.set_read_timeout(Some(RENDER_TIMEOUT));
    if writeln!(stream, "{token}\n{}\n{}", size.clamp(16, 1024), path.display()).is_err() {
        return Server::Unreachable;
    }
    let mut reader = BufReader::new(stream);
    let mut header = String::new();
    if reader.read_line(&mut header).is_err() || header.is_empty() {
        return Server::Unreachable;
    }
    let mut parts = header.split_whitespace();
    if parts.next() != Some("ok") {
        return Server::Answered(None);
    }
    let (Some(Ok(w)), Some(Ok(h))) = (parts.next().map(str::parse::<u32>), parts.next().map(str::parse::<u32>)) else {
        return Server::Answered(None);
    };
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    Server::Answered(reader.read_exact(&mut rgba).ok().map(|_| (w, h, rgba)))
}

/// Runs `polyloupe.exe --thumbnail` into a raw `.rgba` file (no image decoder in this DLL: it
/// must stay dependency-light to load inside Explorer's restricted thumbnail surrogate).
fn render_once(exe: &std::path::Path, path: &std::path::Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let out = std::env::temp_dir().join(format!(
        "polyloupe-thumb-{}-{}.rgba",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut child = Command::new(exe)
        .arg("--thumbnail")
        .arg(path)
        .arg(&out)
        .arg(size.clamp(16, 1024).to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let start = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if start.elapsed() > RENDER_TIMEOUT => {
                let _ = child.kill();
                break false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break false,
        }
    };
    let data = ok.then(|| std::fs::read(&out).ok()).flatten();
    let _ = std::fs::remove_file(&out);
    let data = data?;
    let w = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
    let h = u32::from_le_bytes(data.get(4..8)?.try_into().ok()?);
    let pixels = data.get(8..8 + (w * h * 4) as usize)?.to_vec();
    Some((w, h, pixels))
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
    log(|| format!("DllGetClassObject {:?}", unsafe { clsid.as_ref() }));
    if clsid.is_null()
        || object.is_null()
        || !matches!(
            unsafe { *clsid },
            CLSID_THUMBNAILER | CLSID_THUMBNAILER_INPROC
        )
    {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory {
        path_only: unsafe { *clsid } == CLSID_THUMBNAILER_INPROC,
    }
    .into();
    unsafe { factory.query(riid, object) }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // Kept loaded for the (short) life of the thumbnail surrogate process.
    S_FALSE
}

#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    finish(register(HKEY_CURRENT_USER, false))
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
    let root = if cmdline.trim().eq_ignore_ascii_case("allusers") {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };
    let machine = root == HKEY_LOCAL_MACHINE;
    if install.as_bool() {
        finish(register(root, machine))
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

/// `in_process` routes every format to the non-isolated CLSID (Windows ignores
/// DisableProcessIsolation under HKCU, so it only means something machine-wide).
fn register(root: HKEY, in_process: bool) -> Result<()> {
    let dll = module_path().ok_or(windows_core::Error::from(E_FAIL))?;
    let dll = dll.to_string_lossy();
    register_clsid(root, CLSID_STRING, &dll, false)?;
    if in_process {
        register_clsid(root, CLSID_INPROC_STRING, &dll, true)?;
    }
    for ext in EXTENSIONS {
        let clsid = if in_process { CLSID_INPROC_STRING } else { CLSID_STRING };
        set_value(
            root,
            &format!("{CLASSES}\\{ext}\\ShellEx\\{THUMBNAIL_HANDLER_KEY}"),
            None,
            clsid,
        )?;
    }
    Ok(())
}

fn register_clsid(root: HKEY, clsid: &str, dll: &str, in_process: bool) -> Result<()> {
    let key = format!("{CLASSES}\\CLSID\\{clsid}");
    set_value(root, &key, None, "Poly Loupe Thumbnail Provider")?;
    if in_process {
        set_dword(root, &key, "DisableProcessIsolation", 1)?;
    }
    let server = format!("{key}\\InprocServer32");
    set_value(root, &server, None, dll)?;
    set_value(root, &server, Some("ThreadingModel"), "Apartment")
}

fn unregister(root: HKEY) {
    for clsid in [CLSID_STRING, CLSID_INPROC_STRING] {
        let _ = delete_tree(root, &format!("{CLASSES}\\CLSID\\{clsid}"));
    }
    for ext in EXTENSIONS {
        let _ = delete_tree(
            root,
            &format!("{CLASSES}\\{ext}\\ShellEx\\{THUMBNAIL_HANDLER_KEY}"),
        );
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn set_value(root: HKEY, key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
    write_value(root, key, name, REG_SZ, &data)
}

fn set_dword(root: HKEY, key: &str, name: &str, value: u32) -> Result<()> {
    write_value(root, key, Some(name), REG_DWORD, &value.to_le_bytes())
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
