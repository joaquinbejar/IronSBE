//! Generates Real Logic's example schemas (`Car` versions 0 and 1) in
//! panic-free mode, `GeneratorOptions::with_panicking_setters(false)`, for
//! `src/lib.rs` to compile under the crate's strict lint bar.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

/// `file` from `ironsbe-codegen-tests/schemas/real-logic/`, with its
/// `xi:include` of `common-types.xml` expanded in place (the generator does
/// not resolve includes).
fn real_logic_schema(file: &str) -> Result<String, Box<dyn Error>> {
    let dir = PathBuf::from("../ironsbe-codegen-tests/schemas/real-logic");
    println!("cargo:rerun-if-changed=../ironsbe-codegen-tests/schemas/real-logic/{file}");
    println!("cargo:rerun-if-changed=../ironsbe-codegen-tests/schemas/real-logic/common-types.xml");
    let schema = fs::read_to_string(dir.join(file))?;
    let common = fs::read_to_string(dir.join("common-types.xml"))?;
    let common = common
        .split_once("?>")
        .map_or(common.as_str(), |(_, body)| body)
        .trim();
    Ok(schema.replace(r#"<xi:include href="common-types.xml"/>"#, common))
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    for (file, out) in [
        ("example-schema.xml", "car_v0.rs"),
        ("example-extension-schema.xml", "car_v1.rs"),
    ] {
        let options = ironsbe_codegen::GeneratorOptions::default().with_panicking_setters(false);
        let code =
            ironsbe_codegen::generate_from_xml_with_options(&real_logic_schema(file)?, options)?;
        fs::write(out_dir.join(out), code)?;
    }
    Ok(())
}
