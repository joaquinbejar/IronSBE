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

/// Edge cases of the review of the lint-clean output (IronSBE#80): a
/// fixed-only message whose name contains `GroupHeader` (no groups, so no
/// `GroupHeader` import), and a group whose fields are named
/// `actingVersion` and `actingBlockLength` (their getters read the fields,
/// with no metadata accessor of that name in the way).
const EDGES_SCHEMA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="edges" id="7" version="0" byteOrder="littleEndian">
    <types>
        <type name="uint16" primitiveType="uint16"/>
        <type name="uint32" primitiveType="uint32"/>
        <composite name="messageHeader">
            <type name="blockLength" primitiveType="uint16"/>
            <type name="templateId" primitiveType="uint16"/>
            <type name="schemaId" primitiveType="uint16"/>
            <type name="version" primitiveType="uint16"/>
        </composite>
        <composite name="groupSizeEncoding">
            <type name="blockLength" primitiveType="uint16"/>
            <type name="numInGroup" primitiveType="uint16"/>
        </composite>
    </types>
    <sbe:message name="GroupHeaderSnapshot" id="1">
        <field name="sequence" id="1" type="uint32"/>
    </sbe:message>
</sbe:messageSchema>"#;

/// The same, with a repeating group whose fields carry the names of the
/// entry metadata: a schema of its own, since the first must have no group.
const METADATA_NAMES_SCHEMA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="metadata" id="8" version="0" byteOrder="littleEndian">
    <types>
        <type name="uint16" primitiveType="uint16"/>
        <type name="uint32" primitiveType="uint32"/>
        <composite name="messageHeader">
            <type name="blockLength" primitiveType="uint16"/>
            <type name="templateId" primitiveType="uint16"/>
            <type name="schemaId" primitiveType="uint16"/>
            <type name="version" primitiveType="uint16"/>
        </composite>
        <composite name="groupSizeEncoding">
            <type name="blockLength" primitiveType="uint16"/>
            <type name="numInGroup" primitiveType="uint16"/>
        </composite>
    </types>
    <sbe:message name="Batch" id="1">
        <field name="sequence" id="1" type="uint32"/>
        <group name="items" id="10" dimensionType="groupSizeEncoding">
            <field name="actingVersion" id="11" type="uint16"/>
            <field name="actingBlockLength" id="12" type="uint16"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

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
    for (schema, out) in [
        (EDGES_SCHEMA, "edges.rs"),
        (METADATA_NAMES_SCHEMA, "metadata_names.rs"),
    ] {
        let options = ironsbe_codegen::GeneratorOptions::default().with_panicking_setters(false);
        let code = ironsbe_codegen::generate_from_xml_with_options(schema, options)?;
        fs::write(out_dir.join(out), code)?;
    }
    Ok(())
}
