//! Generates Rust codecs from an inline SBE schema at build time.
//!
//! The output lands in `OUT_DIR/var_data_schema.rs` and is included by
//! `src/lib.rs`, so the integration tests in `tests/` compile and exercise
//! real generator output instead of asserting on strings.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Schema exercising every var data layout the generator supports:
/// `uint16`, `uint8` and `uint32` length headers, var data after two flat
/// repeating groups, var data with no groups, and groups with no var data.
const VAR_DATA_SCHEMA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="vardata" id="42" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint8" primitiveType="uint8"/>
        <type name="uint16" primitiveType="uint16"/>
        <type name="uint32" primitiveType="uint32"/>
        <type name="uint64" primitiveType="uint64"/>
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
        <composite name="varStringEncoding">
            <type name="length" primitiveType="uint16"/>
            <type name="varData" primitiveType="uint8" length="0" characterEncoding="UTF-8"/>
        </composite>
        <composite name="varDataEncoding8">
            <type name="length" primitiveType="uint8"/>
            <type name="varData" primitiveType="uint8" length="0"/>
        </composite>
        <composite name="varDataEncoding32">
            <type name="length" primitiveType="uint32"/>
            <type name="varData" primitiveType="uint8" length="0"/>
        </composite>
    </types>

    <sbe:message name="Example" id="1" blockLength="8">
        <field name="qty" id="1" type="uint64" offset="0"/>
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="12">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <field name="ratio" id="12" type="uint32" offset="8"/>
        </group>
        <group name="notes" id="20" dimensionType="groupSizeEncoding" blockLength="2">
            <field name="code" id="21" type="uint16" offset="0"/>
        </group>
        <data name="label" id="2" type="varStringEncoding"/>
        <data name="payload" id="3" type="varDataEncoding8"/>
    </sbe:message>

    <sbe:message name="OnlyData" id="2" blockLength="0">
        <data name="blob" id="1" type="varDataEncoding32"/>
    </sbe:message>

    <sbe:message name="GroupsOnly" id="3" blockLength="4">
        <field name="requestId" id="1" type="uint32" offset="0"/>
        <group name="items" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="itemId" id="11" type="uint64" offset="0"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let code = ironsbe_codegen::generate_from_xml(VAR_DATA_SCHEMA)
        .expect("codegen failed for the var data schema");
    fs::write(out_dir.join("var_data_schema.rs"), code)
        .expect("failed to write generated var data codecs");
}
