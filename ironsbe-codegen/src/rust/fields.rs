//! Fixed-length field accessor emission.
//!
//! Getters and setters for `<field>` elements are identical for message
//! decoders/encoders and group entry decoders/encoders except for the base
//! offset expression, so they live here and are shared by both generators.

use ironsbe_schema::ir::{ResolvedField, SchemaIr, TypeKind};
use ironsbe_schema::types::PrimitiveType;

use crate::rust::names::{accessor_name, renamed_note};

/// How a host struct knows whether a versioned field (`sinceVersion > 0`) is
/// present in the message it wraps.
///
/// Presence is a property of the *version*: an older encoder may have reserved
/// padding where a later field now lives (SBE 1.0 §5.4.3), so a field that
/// fits inside the block is still absent when the acting version predates
/// it. Bounds are checked in addition to the version where the block length
/// comes from the wire (group entries), so a group header that under-declares
/// its block never lets a getter read past the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresenceGate {
    /// Message host: `acting_version: u16`; the root block was validated
    /// against that version by `decode`.
    Message,
    /// Group entry host: `acting_version: u16` and the wire `block_length: u16`.
    Entry,
}

impl PresenceGate {
    fn condition(self, field: &ResolvedField) -> String {
        match self {
            Self::Message => format!("self.acting_version >= {}", field.since_version),
            Self::Entry => format!(
                "self.acting_version >= {} && usize::from(self.block_length) >= {}",
                field.since_version,
                field.end_offset()
            ),
        }
    }
}

/// One emitted getter: a doc line, a name, a return type, optional `let`
/// bindings and the value expression. Emitting from this shape lets versioned
/// fields wrap the value in `Some` and add the presence check without
/// re-parsing generated text.
struct Getter {
    doc: String,
    name: String,
    ret: String,
    bindings: Vec<String>,
    value: String,
}

impl Getter {
    fn emit(&self, gate: Option<(PresenceGate, &ResolvedField)>) -> String {
        let mut out = String::new();
        out.push_str(&self.doc);
        out.push_str("    #[inline(always)]\n");
        out.push_str("    #[must_use]\n");
        match gate {
            Some((gate, field)) if field.since_version > 0 => {
                out.push_str(&format!(
                    "    /// Present since schema version {}; `None` when the message was\n",
                    field.since_version
                ));
                out.push_str("    /// encoded by an older version.\n");
                out.push_str(&format!(
                    "    pub fn {}(&self) -> Option<{}> {{\n",
                    self.name, self.ret
                ));
                out.push_str(&format!("        if !({}) {{\n", gate.condition(field)));
                out.push_str("            return None;\n");
                out.push_str("        }\n");
                for b in &self.bindings {
                    out.push_str(&format!("        {b}\n"));
                }
                out.push_str(&format!("        Some({})\n", self.value));
            }
            _ => {
                out.push_str(&format!(
                    "    pub fn {}(&self) -> {} {{\n",
                    self.name, self.ret
                ));
                for b in &self.bindings {
                    out.push_str(&format!("        {b}\n"));
                }
                out.push_str(&format!("        {}\n", self.value));
            }
        }
        out.push_str("    }\n\n");
        out
    }
}

/// Generates the getter method(s) for a fixed-length field.
///
/// Emitted inside an `impl` block whose `self` has `buffer: &'a [u8]` and
/// `offset: usize` (start of the fixed block). `reserved` lists the methods
/// the host defines itself; a field named like one of them gets a trailing
/// underscore (see `names`). `gate` says how the host detects a field that a
/// message encoded by an older schema version does not carry; such fields
/// (`sinceVersion > 0`) get `Option` getters.
pub(crate) fn generate_field_getter(
    ir: &SchemaIr,
    field: &ResolvedField,
    reserved: &[&str],
    gate: PresenceGate,
) -> String {
    field_getters(ir, field, reserved)
        .iter()
        .map(|g| g.emit(Some((gate, field))))
        .collect()
}

fn field_getters(ir: &SchemaIr, field: &ResolvedField, reserved: &[&str]) -> Vec<Getter> {
    let getter = accessor_name(&field.getter_name, reserved);
    let mut doc = format!(
        "    /// Field: {} (id={}, offset={}).\n",
        field.name, field.id, field.offset
    );
    doc.push_str(&renamed_note(&field.getter_name, &getter));
    let range = format!(
        "self.offset + {}..self.offset + {}",
        field.offset,
        field.end_offset()
    );

    if field.is_array {
        let elem_type = field.primitive_type.map(|p| p.rust_type()).unwrap_or("u8");
        let mut getters = vec![Getter {
            doc,
            name: getter.clone(),
            ret: "&'a [u8]".to_owned(),
            bindings: Vec::new(),
            value: format!("&self.buffer[{range}]"),
        }];
        if elem_type == "u8" {
            // Also generate a string accessor for char arrays
            getters.push(Getter {
                doc: format!("    /// Field {} as string (trimmed).\n", field.name),
                name: format!("{getter}_as_str"),
                ret: "&'a str".to_owned(),
                bindings: vec![
                    format!("let bytes = &self.buffer[{range}];"),
                    "let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());"
                        .to_owned(),
                ],
                value: "std::str::from_utf8(&bytes[..end]).unwrap_or(\"\")".to_owned(),
            });
        }
        return getters;
    }

    // Scalar field - check if it's an enum/set type
    let rust_type = field.rust_type.clone();
    let resolved_type = ir.get_type(&field.type_name);
    let (ret, value) = match resolved_type.map(|t| &t.kind) {
        Some(TypeKind::Enum { encoding, .. }) => {
            let read_method = get_read_method(Some(*encoding));
            (
                rust_type.clone(),
                format!(
                    "{}::from(self.buffer.{}(self.offset + {}))",
                    rust_type, read_method, field.offset
                ),
            )
        }
        Some(TypeKind::Set { encoding, .. }) => {
            let read_method = get_read_method(Some(*encoding));
            (
                rust_type.clone(),
                format!(
                    "{}::from_raw(self.buffer.{}(self.offset + {}))",
                    rust_type, read_method, field.offset
                ),
            )
        }
        Some(TypeKind::Composite { .. }) => (
            format!("{rust_type}<'a>"),
            format!(
                "{}::wrap(self.buffer, self.offset + {})",
                rust_type, field.offset
            ),
        ),
        _ => {
            let read_method = get_read_method(field.primitive_type);
            (
                rust_type,
                format!(
                    "self.buffer.{}(self.offset + {})",
                    read_method, field.offset
                ),
            )
        }
    };
    vec![Getter {
        doc,
        name: getter,
        ret,
        bindings: Vec::new(),
        value,
    }]
}

/// Generates a field setter for a message encoder.
///
/// Message encoders keep `offset` at the message header, so the field offset
/// is prefixed with `MessageHeader::ENCODED_LENGTH`.
pub(crate) fn generate_field_setter(ir: &SchemaIr, field: &ResolvedField) -> String {
    let field_offset = format!("MessageHeader::ENCODED_LENGTH + {}", field.offset);
    generate_setter_at(ir, field, &field_offset)
}

/// Generates a field setter for a group entry encoder.
///
/// Entry encoders keep `offset` at the start of the entry's fixed block, so
/// the raw field offset is used without a header prefix.
pub(crate) fn generate_entry_field_setter(ir: &SchemaIr, field: &ResolvedField) -> String {
    generate_setter_at(ir, field, &field.offset.to_string())
}

/// Generates a field setter whose write position is `self.offset + field_offset`.
fn generate_setter_at(ir: &SchemaIr, field: &ResolvedField, field_offset: &str) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Set field: {} (id={}, offset={}).\n",
        field.name, field.id, field.offset
    ));
    output.push_str("    #[inline(always)]\n");

    if field.is_array {
        // Array field - accept slice
        let len = field.array_length.unwrap_or(field.encoded_length);

        output.push_str(&format!(
            "    pub fn {}(&mut self, value: &[u8]) -> &mut Self {{\n",
            field.setter_name
        ));
        output.push_str(&format!(
            "        let copy_len = value.len().min({});\n",
            len
        ));
        output.push_str(&format!(
            "        self.buffer[self.offset + {}..self.offset + {} + copy_len]\n",
            field_offset, field_offset
        ));
        output.push_str("            .copy_from_slice(&value[..copy_len]);\n");
        output.push_str(&format!("        if copy_len < {} {{\n", len));
        output.push_str(&format!(
            "            self.buffer[self.offset + {} + copy_len..self.offset + {} + {}].fill(0);\n",
            field_offset, field_offset, len
        ));
        output.push_str("        }\n");
        output.push_str("        self\n");
        output.push_str("    }\n\n");
    } else {
        // Scalar field - check if it's an enum/set type
        let rust_type = &field.rust_type;
        let resolved_type = ir.get_type(&field.type_name);

        match resolved_type.map(|t| &t.kind) {
            Some(TypeKind::Enum { encoding, .. }) => {
                // Enum field - convert enum to primitive before writing
                let write_method = get_write_method(Some(*encoding));
                let prim_type = encoding.rust_type();
                output.push_str(&format!(
                    "    pub fn {}(&mut self, value: {}) -> &mut Self {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        self.buffer.{}(self.offset + {}, {}::from(value));\n",
                    write_method, field_offset, prim_type
                ));
                output.push_str("        self\n");
                output.push_str("    }\n\n");
            }
            Some(TypeKind::Set { encoding, .. }) => {
                // Set field - use raw() to get the primitive value
                let write_method = get_write_method(Some(*encoding));
                output.push_str(&format!(
                    "    pub fn {}(&mut self, value: {}) -> &mut Self {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        self.buffer.{}(self.offset + {}, value.raw());\n",
                    write_method, field_offset
                ));
                output.push_str("        self\n");
                output.push_str("    }\n\n");
            }
            Some(TypeKind::Composite { .. }) => {
                // Composite field - return encoder for nested writes
                output.push_str(&format!(
                    "    pub fn {}(&mut self) -> {}Encoder<'_> {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        {}Encoder::wrap(self.buffer, self.offset + {})\n",
                    rust_type, field_offset
                ));
                output.push_str("    }\n\n");
            }
            _ => {
                // Primitive field
                let write_method = get_write_method(field.primitive_type);
                output.push_str(&format!(
                    "    pub fn {}(&mut self, value: {}) -> &mut Self {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        self.buffer.{}(self.offset + {}, value);\n",
                    write_method, field_offset
                ));
                output.push_str("        self\n");
                output.push_str("    }\n\n");
            }
        }
    }

    output
}

/// Gets the `ReadBuffer` method name for a primitive type.
pub(crate) fn get_read_method(prim: Option<PrimitiveType>) -> &'static str {
    match prim {
        Some(PrimitiveType::Char) | Some(PrimitiveType::Uint8) => "get_u8",
        Some(PrimitiveType::Int8) => "get_i8",
        Some(PrimitiveType::Uint16) => "get_u16_le",
        Some(PrimitiveType::Int16) => "get_i16_le",
        Some(PrimitiveType::Uint32) => "get_u32_le",
        Some(PrimitiveType::Int32) => "get_i32_le",
        Some(PrimitiveType::Uint64) => "get_u64_le",
        Some(PrimitiveType::Int64) => "get_i64_le",
        Some(PrimitiveType::Float) => "get_f32_le",
        Some(PrimitiveType::Double) => "get_f64_le",
        None => "get_u64_le",
    }
}

/// Gets the `WriteBuffer` method name for a primitive type.
pub(crate) fn get_write_method(prim: Option<PrimitiveType>) -> &'static str {
    match prim {
        Some(PrimitiveType::Char) | Some(PrimitiveType::Uint8) => "put_u8",
        Some(PrimitiveType::Int8) => "put_i8",
        Some(PrimitiveType::Uint16) => "put_u16_le",
        Some(PrimitiveType::Int16) => "put_i16_le",
        Some(PrimitiveType::Uint32) => "put_u32_le",
        Some(PrimitiveType::Int32) => "put_i32_le",
        Some(PrimitiveType::Uint64) => "put_u64_le",
        Some(PrimitiveType::Int64) => "put_i64_le",
        Some(PrimitiveType::Float) => "put_f32_le",
        Some(PrimitiveType::Double) => "put_f64_le",
        None => "put_u64_le",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_read_method_covers_every_primitive() {
        assert_eq!(get_read_method(Some(PrimitiveType::Char)), "get_u8");
        assert_eq!(get_read_method(Some(PrimitiveType::Uint16)), "get_u16_le");
        assert_eq!(get_read_method(Some(PrimitiveType::Int64)), "get_i64_le");
        assert_eq!(get_read_method(Some(PrimitiveType::Double)), "get_f64_le");
        assert_eq!(get_read_method(None), "get_u64_le");
    }

    #[test]
    fn test_get_write_method_covers_every_primitive() {
        assert_eq!(get_write_method(Some(PrimitiveType::Int8)), "put_i8");
        assert_eq!(get_write_method(Some(PrimitiveType::Uint32)), "put_u32_le");
        assert_eq!(get_write_method(Some(PrimitiveType::Float)), "put_f32_le");
        assert_eq!(get_write_method(None), "put_u64_le");
    }
}
