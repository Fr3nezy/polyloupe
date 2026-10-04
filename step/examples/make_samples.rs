//! Writes the STEP test files: `cargo run --release -p polyloupe-step --example make_samples`.
//!
//! - `test_assets/formats/showcase.step`: a small colored assembly (fillets, a cylinder, a
//!   plate with a hole), the regression file for the STEP loader.
//! - `target/step-stress.step`: 400 filleted parts, to time big assemblies (not committed).

use cadrum::{DVec3, Error, Solid};

fn rounded_cube(size: f64) -> Result<Solid, Error> {
    let cube = Solid::cube(DVec3::ZERO, DVec3::splat(size)).translate(-DVec3::ONE * (size / 2.0));
    cube.fillet_edges(size * 0.2, cube.iter_edge())
}

fn coin(radius: f64, height: f64) -> Result<Solid, Error> {
    let cyl = Solid::cylinder(radius, DVec3::Z * height);
    let top = cyl.iter_edge().filter(|e| [e.start_point(), e.end_point()].iter().all(|p| (p.z - height).abs() < 1e-6));
    cyl.fillet_edges(height * 0.3, top)
}

/// A 30 x 20 x 4 plate with a hole through it.
fn plate() -> Result<Solid, Error> {
    let plate = Solid::cube(DVec3::ZERO, DVec3::new(30.0, 20.0, 4.0));
    let hole = Solid::cylinder(5.0, DVec3::Z * 10.0).translate(DVec3::new(15.0, 10.0, -3.0));
    Ok(Solid::boolean_build(&(plate - hole))?.remove(0))
}

fn main() -> Result<(), Error> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let showcase = [
        rounded_cube(8.0)?.color("#d0a878").translate(DVec3::new(4.0, 4.0, 4.0)),
        coin(4.0, 2.0)?.color("#0052ff").translate(DVec3::new(18.0, 4.0, 0.0)),
        plate()?.color("#b0b4ba").translate(DVec3::new(-6.0, 12.0, 0.0)),
    ];
    let path = root.join("test_assets/formats/showcase.step");
    Solid::write_step(&showcase, &mut std::fs::File::create(&path).unwrap())?;
    println!("wrote {}", path.display());

    let mut stress = Vec::new();
    for i in 0..400 {
        let (x, y) = ((i % 20) as f64 * 12.0, (i / 20) as f64 * 12.0);
        stress.push(rounded_cube(8.0)?.translate(DVec3::new(x, y, 4.0)));
    }
    let path = root.join("target/step-stress.step");
    Solid::write_step(&stress, &mut std::fs::File::create(&path).unwrap())?;
    println!("wrote {}", path.display());
    Ok(())
}
