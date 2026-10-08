//! Compiles the C++ STEP reader against the OpenCASCADE that `cadrum` builds and links.
//! `OCCT_ROOT` (set in .cargo/config.toml) tells both where that build lives.

fn main() {
    println!("cargo:rerun-if-changed=src/reader.cpp");
    println!("cargo:rerun-if-env-changed=OCCT_ROOT");
    let root = std::path::PathBuf::from(std::env::var("OCCT_ROOT").expect("OCCT_ROOT is set in .cargo/config.toml"));
    // cadrum's dependency build script installs OCCT before this one runs.
    let include = ["inc", "include/opencascade", "include"].iter().map(|d| root.join(d)).find(|p| p.join("Standard.hxx").exists());
    let include = include.unwrap_or_else(|| panic!("OpenCASCADE headers not found under {}", root.display()));
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").file("src/reader.cpp").include(include).define("_USE_MATH_DEFINES", None);
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Same flags cadrum compiles OCCT and its wrapper with.
        build.flag("/utf-8").flag("/D_USE_STD_VECTOR_ALGORITHMS=0").flag("/EHsc");
    }
    build.compile("polyloupe_step_reader");
}
