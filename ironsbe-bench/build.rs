//! Generates the Rust codecs used by the `sequential_decode` bench from an
//! inline SBE schema at build time.
//!
//! The output lands in `OUT_DIR/bench_schema.rs` and is included by the
//! bench, so it measures real generator output.

use std::env;
use std::fs;
use std::path::PathBuf;

/// A price book: one variable-stride repeating group (each level carries a
/// var data `venue`) followed by eight message-level var data fields. This
/// is the shape where random-access decoding re-walks the group once per
/// var data accessor (issue #64).
const BENCH_SCHEMA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="bench" id="7" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint8" primitiveType="uint8"/>
        <type name="uint16" primitiveType="uint16"/>
        <type name="uint64" primitiveType="uint64"/>
        <type name="int64" primitiveType="int64"/>
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
    </types>

    <sbe:message name="Book" id="1" blockLength="8">
        <field name="seq" id="1" type="uint64" offset="0"/>
        <group name="levels" id="10" dimensionType="groupSizeEncoding" blockLength="16">
            <field name="price" id="11" type="int64" offset="0"/>
            <field name="size" id="12" type="uint64" offset="8"/>
            <data name="venue" id="13" type="varDataEncoding8"/>
        </group>
        <data name="symbol" id="2" type="varStringEncoding"/>
        <data name="account" id="3" type="varStringEncoding"/>
        <data name="clOrdId" id="4" type="varStringEncoding"/>
        <data name="text" id="5" type="varStringEncoding"/>
        <data name="source" id="6" type="varStringEncoding"/>
        <data name="target" id="7" type="varStringEncoding"/>
        <data name="tag" id="8" type="varStringEncoding"/>
        <data name="trailer" id="9" type="varStringEncoding"/>
    </sbe:message>
</sbe:messageSchema>"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let code = ironsbe_codegen::generate_from_xml(BENCH_SCHEMA)
        .expect("codegen failed for the bench schema");
    fs::write(out_dir.join("bench_schema.rs"), code)
        .expect("failed to write generated bench codecs");
}
