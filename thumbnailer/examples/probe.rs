//! Loads the handler DLL like Explorer would (without registering it) and saves the
//! thumbnail it produces: `cargo run -p polyloupe-thumbs --example probe -- MODEL OUT.png [SIZE]`.

use std::ffi::c_void;

use windows::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW, HGDIOBJ};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, IClassFactory};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::IThumbnailProvider;
use windows::Win32::UI::Shell::PropertiesSystem::IInitializeWithFile;
use windows_core::{GUID, HRESULT, Interface, PCWSTR, s};

type GetClassObject =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

fn main() -> windows_core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (model, out) = (&args[1], &args[2]);
    let size: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
    let dll = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("polyloupe_thumbs.dll");
    let dll_w: Vec<u16> = dll.to_string_lossy().encode_utf16().chain([0]).collect();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let lib = LoadLibraryW(PCWSTR(dll_w.as_ptr()))?;
        let proc = GetProcAddress(lib, s!("DllGetClassObject")).expect("DllGetClassObject export");
        let get: GetClassObject = std::mem::transmute(proc);
        let clsid = GUID::from_u128(0x1fcd9853_22d9_46b7_a5c0_cba9444584c9);
        let mut factory: *mut c_void = std::ptr::null_mut();
        get(&clsid, &IClassFactory::IID, &mut factory).ok()?;
        let factory = IClassFactory::from_raw(factory);
        let provider: IThumbnailProvider = factory.CreateInstance(None)?;
        let init: IInitializeWithFile = provider.cast()?;
        let path_w: Vec<u16> = model.encode_utf16().chain([0]).collect();
        init.Initialize(PCWSTR(path_w.as_ptr()), 0)?;
        let mut hbmp = Default::default();
        let mut alpha = Default::default();
        let start = std::time::Instant::now();
        provider.GetThumbnail(size, &mut hbmp, &mut alpha)?;
        let mut bm = BITMAP::default();
        GetObjectW(
            HGDIOBJ(hbmp.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut _ as *mut c_void),
        );
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
        let src = std::slice::from_raw_parts(bm.bmBits as *const u8, (w * h * 4) as usize);
        let rgba: Vec<u8> = src
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        image::save_buffer(out, &rgba, w, h, image::ColorType::Rgba8).unwrap();
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        println!(
            "{w}x{h} alpha={:?} in {} ms",
            alpha,
            start.elapsed().as_millis()
        );
    }
    Ok(())
}
