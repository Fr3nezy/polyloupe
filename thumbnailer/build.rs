fn main() {
    // Mark the COM entry points PRIVATE (they're looked up with GetProcAddress, never linked).
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("exports.def");
    println!("cargo:rustc-cdylib-link-arg=/DEF:{}", def.display());
    println!("cargo:rerun-if-changed=exports.def");
}
