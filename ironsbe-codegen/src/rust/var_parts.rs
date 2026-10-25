//! Variable parts of a message or group entry: its repeating groups followed
//! by its `<data>` fields, in schema order.
//!
//! Encoders append these parts at a write cursor and decoders walk them on
//! the wire, so both sides share one ordered model. This module builds that
//! model and emits the two helpers that depend on it:
//!
//! - the *encoder guard* (issue #63): a `written` counter plus helpers that
//!   write an empty header for every part the caller skipped, so a frame is
//!   always well-formed and a part written out of schema order panics
//!   instead of silently corrupting what follows;
//! - the *reader skip* (issue #64): a `part` counter plus helpers that walk
//!   unread parts exactly once, so a sequential reader never re-visits the
//!   wire.
//!
//! Both emitters are parameterised by the cursor place expression because
//! message hosts own their cursor (`self.limit`, `self.pos`) while group and
//! entry hosts borrow the parent's (`*self.limit`, `*self.pos`).

use crate::rust::groups::GroupLayout;
use crate::rust::var_data::VarDataInfo;

/// One variable part of an owner (a message or a group entry).
pub(crate) enum VarPart<'p> {
    /// A repeating group.
    Group {
        /// Schema name, for doc comments and panic messages.
        name: String,
        /// Group decoder type, qualified as needed from the host.
        decoder_type: String,
        /// Group encoder type, qualified as needed from the host.
        encoder_type: String,
        /// Group reader type, qualified as needed from the host. Only
        /// emitted for variable-stride groups.
        reader_type: String,
        /// True when entries are exactly `blockLength` bytes: no nested
        /// groups and no var data.
        fixed_stride: bool,
    },
    /// A `<data>` field.
    Data(&'p VarDataInfo),
}

/// Builds the ordered variable parts of an owner.
///
/// # Arguments
/// * `groups` - The owner's repeating groups, in schema order
/// * `var_data` - The owner's `<data>` fields, in schema order
/// * `type_prefix` - Module path prefix for the group codec types, e.g.
///   `quote::` on a message host or empty inside the message module
pub(crate) fn collect_var_parts<'p>(
    groups: &[GroupLayout<'_>],
    var_data: &'p [VarDataInfo],
    type_prefix: &str,
) -> Vec<VarPart<'p>> {
    let mut parts = Vec::with_capacity(groups.len() + var_data.len());
    for layout in groups {
        parts.push(VarPart::Group {
            name: layout.group.name.clone(),
            decoder_type: format!("{type_prefix}{}", layout.group.decoder_name()),
            encoder_type: format!("{type_prefix}{}", layout.group.encoder_name()),
            reader_type: format!("{type_prefix}{}", layout.group.reader_name()),
            fixed_stride: layout.is_fixed_stride(),
        });
    }
    parts.extend(var_data.iter().map(VarPart::Data));
    parts
}

/// Emits the encoder guard for an owner with `parts`: the part count, the
/// fill loop used on drop and on `finish()`, the ordered-write check used by
/// every var data setter and group accessor, and the per-part empty header
/// writer.
///
/// The host struct must carry `written: u16` (parts written so far).
///
/// # Arguments
/// * `parts` - The owner's variable parts, in schema order
/// * `cursor` - Place expression of the write cursor: `self.limit` on a
///   message encoder, `*self.limit` on an entry encoder
pub(crate) fn generate_encoder_parts_guard(parts: &[VarPart<'_>], cursor: &str) -> String {
    let mut output = String::new();
    if parts.is_empty() {
        return output;
    }

    output.push_str("    /// Number of repeating groups and var data fields of this owner.\n");
    output.push_str(&format!(
        "    const SBE_VAR_PARTS: u16 = {};\n\n",
        parts.len()
    ));

    output
        .push_str("    /// Writes an empty header for every variable part in `written..target`.\n");
    output.push_str("    fn sbe_fill_to(&mut self, target: u16) {\n");
    output.push_str("        while self.written < target {\n");
    output.push_str("            self.sbe_write_empty_part(self.written);\n");
    output.push_str("            self.written += 1;\n");
    output.push_str("        }\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Encoded size in bytes of each variable part's empty header.\n");
    output.push_str(&format!(
        "    const SBE_EMPTY_PART_LEN: [usize; {}] = [",
        parts.len()
    ));
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        match part {
            VarPart::Group { .. } => output.push_str("GroupHeader::ENCODED_LENGTH"),
            VarPart::Data(info) => output.push_str(&info.header_length.to_string()),
        }
    }
    output.push_str("];\n\n");

    output.push_str("    /// Bytes `sbe_fill_to(target)` writes: the empty headers of every\n");
    output.push_str("    /// part in `written..target`, or `None` if the sum overflows.\n");
    output.push_str("    #[inline]\n");
    output.push_str("    fn sbe_fill_len(&self, target: u16) -> Option<usize> {\n");
    output.push_str("        Self::SBE_EMPTY_PART_LEN\n");
    output.push_str("            .get(usize::from(self.written)..usize::from(target))\n");
    output.push_str("            .unwrap_or(&[])\n");
    output.push_str("            .iter()\n");
    output.push_str("            .try_fold(0usize, |acc, len| acc.checked_add(*len))\n");
    output.push_str("    }\n\n");

    output
        .push_str("    /// Checks that part `target` comes after every part written so far and\n");
    output
        .push_str("    /// that the buffer holds the empty headers of the parts in between plus\n");
    output.push_str("    /// `extra` bytes, then writes those empty headers.\n");
    output.push_str("    ///\n");
    output.push_str("    /// Nothing is written when it returns an error.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Errors\n");
    output.push_str(
        "    /// Returns [`ironsbe_core::error::EncodeError::OutOfOrder`] if part `target`\n",
    );
    output.push_str(
        "    /// (or a later one) was already written, and\n    /// [`ironsbe_core::error::EncodeError::BufferTooShort`] if the buffer is too\n",
    );
    output.push_str("    /// short.\n");
    output.push_str("    #[inline]\n");
    output.push_str(
        "    fn sbe_advance_to(&mut self, target: u16, what: &'static str, extra: usize) -> Result<(), ironsbe_core::error::EncodeError> {\n",
    );
    output.push_str("        if self.written > target {\n");
    output.push_str(
        "            return Err(ironsbe_core::error::EncodeError::OutOfOrder { field: what, written: self.written });\n",
    );
    output.push_str("        }\n");
    output.push_str("        let required = self\n");
    output.push_str("            .sbe_fill_len(target)\n");
    output.push_str(&format!(
        "            .and_then(|fill| ({cursor}).checked_add(fill))\n"
    ));
    output.push_str("            .and_then(|end| end.checked_add(extra));\n");
    output.push_str(
        "        ironsbe_core::bounds::ensure_capacity(self.buffer.len(), required, what)?;\n",
    );
    output.push_str("        self.sbe_fill_to(target);\n");
    output.push_str("        Ok(())\n");
    output.push_str("    }\n\n");

    output.push_str(
        "    /// Writes the empty header of variable part `index` at the write cursor:\n",
    );
    output
        .push_str("    /// a group header with zero entries, or a zero-length var data header.\n");
    output.push_str("    fn sbe_write_empty_part(&mut self, index: u16) {\n");
    output.push_str("        match index {\n");
    for (index, part) in parts.iter().enumerate() {
        output.push_str(&format!("            {index} => {{\n"));
        match part {
            VarPart::Group {
                name, encoder_type, ..
            } => {
                output.push_str(&format!(
                    "                // {name}: empty repeating group\n"
                ));
                output.push_str(&format!(
                    "                GroupHeader::new({encoder_type}::BLOCK_LENGTH, 0).encode(self.buffer, {cursor});\n"
                ));
                output.push_str(&format!(
                    "                {cursor} += GroupHeader::ENCODED_LENGTH;\n"
                ));
            }
            VarPart::Data(info) => {
                output.push_str(&format!(
                    "                // {}: zero-length var data\n",
                    info.name
                ));
                output.push_str(&format!(
                    "                self.buffer.{}({cursor}, 0);\n",
                    info.write_method
                ));
                output.push_str(&format!(
                    "                {cursor} += {};\n",
                    info.header_length
                ));
            }
        }
        output.push_str("            }\n");
    }
    output.push_str("            _ => {}\n");
    output.push_str("        }\n");
    output.push_str("    }\n\n");

    output
}

/// Emits the reader skip helpers for an owner with `parts`: the part count,
/// the skip loop used on drop and on `finish()`, the ordered-read check used
/// by every accessor, and the per-part walker.
///
/// The host struct must carry `part: u16` (parts consumed so far) and
/// `buffer: &'a [u8]`.
///
/// # Arguments
/// * `parts` - The owner's variable parts, in schema order
/// * `cursor` - Place expression of the read cursor: `self.pos` on a message
///   reader, `*self.pos` on an entry reader
pub(crate) fn generate_reader_parts_skip(parts: &[VarPart<'_>], cursor: &str) -> String {
    let mut output = String::new();
    if parts.is_empty() {
        return output;
    }

    output.push_str("    /// Number of repeating groups and var data fields of this owner.\n");
    output.push_str(&format!(
        "    const SBE_VAR_PARTS: u16 = {};\n\n",
        parts.len()
    ));

    output.push_str(
        "    /// Walks every variable part in `part..target` on the wire, reading each\n",
    );
    output.push_str("    /// header once.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Errors\n");
    output.push_str(
        "    /// Returns [`DecodeError::VarPartOutOfBounds`] if a part extends past the\n",
    );
    output.push_str("    /// end of the buffer; the parts walked so far stay consumed.\n");
    output.push_str("    fn sbe_skip_to(&mut self, target: u16) -> Result<(), DecodeError> {\n");
    output.push_str("        while self.part < target {\n");
    output.push_str("            self.sbe_skip_part(self.part)?;\n");
    output.push_str("            self.part += 1;\n");
    output.push_str("        }\n");
    output.push_str("        Ok(())\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Checks that part `target` comes after every part read so far and\n");
    output.push_str("    /// skips the parts in between.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Errors\n");
    output.push_str(
        "    /// Returns [`DecodeError::OutOfOrder`] if part `target` (or a later one) was\n",
    );
    output.push_str("    /// already read, and the `sbe_skip_to` errors.\n");
    output.push_str("    #[inline]\n");
    output.push_str(
        "    fn sbe_advance_to(&mut self, target: u16, what: &'static str) -> Result<(), DecodeError> {\n",
    );
    output.push_str("        if self.part > target {\n");
    output.push_str(
        "            return Err(DecodeError::OutOfOrder { part: what, read: self.part });\n",
    );
    output.push_str("        }\n");
    output.push_str("        self.sbe_skip_to(target)\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Advances the cursor past variable part `index`, reading its header\n");
    output.push_str("    /// once and checking that the part fits in the buffer.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Errors\n");
    output.push_str(
        "    /// Returns [`DecodeError::VarPartOutOfBounds`] if the part extends past the\n",
    );
    output.push_str("    /// end of the buffer.\n");
    output.push_str("    fn sbe_skip_part(&mut self, index: u16) -> Result<(), DecodeError> {\n");
    output.push_str("        match index {\n");
    for (index, part) in parts.iter().enumerate() {
        output.push_str(&format!("            {index} => {{\n"));
        match part {
            VarPart::Group {
                name, decoder_type, ..
            } => {
                output.push_str(&format!(
                    "                // {name}: walk the repeating group\n"
                ));
                output.push_str(&format!(
                    "                {cursor} = {decoder_type}::wrap(self.buffer, {cursor}, self.acting_version)?.end_offset()?;\n"
                ));
            }
            VarPart::Data(info) => {
                output.push_str(&format!(
                    "                // {}: length header + payload\n",
                    info.name
                ));
                output.push_str(&format!(
                    "                {cursor} = ironsbe_core::bounds::var_data_end(self.buffer, {cursor}, {}, \"{}\")?;\n",
                    info.header_length,
                    info.part_name()
                ));
            }
        }
        output.push_str("            }\n");
    }
    output.push_str("            _ => {}\n");
    output.push_str("        }\n");
    output.push_str("        Ok(())\n");
    output.push_str("    }\n\n");

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironsbe_schema::ir::to_snake_case;

    fn info(name: &str, header_length: usize) -> VarDataInfo {
        let (length_type, length_rust_type, write_method) = match header_length {
            1 => ("uint8", "u8", "put_u8"),
            4 => ("uint32", "u32", "put_u32_le"),
            _ => ("uint16", "u16", "put_u16_le"),
        };
        VarDataInfo {
            name: name.to_string(),
            getter: to_snake_case(name),
            accessor: to_snake_case(name),
            id: 1,
            length_type,
            length_rust_type,
            header_length,
            write_method,
        }
    }

    fn group(name: &str, fixed_stride: bool) -> VarPart<'static> {
        VarPart::Group {
            name: name.to_string(),
            decoder_type: format!("m::{name}GroupDecoder"),
            encoder_type: format!("m::{name}GroupEncoder"),
            reader_type: format!("m::{name}GroupReader"),
            fixed_stride,
        }
    }

    #[test]
    fn test_empty_parts_emit_nothing() {
        assert!(generate_encoder_parts_guard(&[], "self.limit").is_empty());
        assert!(generate_reader_parts_skip(&[], "self.pos").is_empty());
    }

    #[test]
    fn test_encoder_guard_one_arm_per_part_in_order() {
        let data = [info("label", 2), info("payload", 1)];
        let parts = [
            group("Legs", true),
            VarPart::Data(&data[0]),
            VarPart::Data(&data[1]),
        ];
        let code = generate_encoder_parts_guard(&parts, "self.limit");

        assert!(code.contains("const SBE_VAR_PARTS: u16 = 3;"));
        assert!(code.contains("fn sbe_fill_to(&mut self, target: u16)"));
        assert!(code.contains(
            "fn sbe_advance_to(&mut self, target: u16, what: &'static str, extra: usize) -> Result<(), ironsbe_core::error::EncodeError>"
        ));
        assert!(code.contains(
            "ironsbe_core::error::EncodeError::OutOfOrder { field: what, written: self.written }"
        ));
        assert!(code.contains(
            "ironsbe_core::bounds::ensure_capacity(self.buffer.len(), required, what)?;"
        ));
        // capacity is checked before the empty headers are written
        let check = code.find("ensure_capacity").expect("capacity check");
        let fill = code
            .find("        self.sbe_fill_to(target);")
            .expect("fill");
        assert!(check < fill);

        let legs = code.find("0 => {").expect("legs arm");
        let label = code.find("1 => {").expect("label arm");
        let payload = code.find("2 => {").expect("payload arm");
        assert!(legs < label && label < payload);
        assert!(code.contains(
            "GroupHeader::new(m::LegsGroupEncoder::BLOCK_LENGTH, 0).encode(self.buffer, self.limit);"
        ));
        assert!(code.contains("self.limit += GroupHeader::ENCODED_LENGTH;"));
        assert!(
            code.contains(
                "self.buffer.put_u16_le(self.limit, 0);\n                self.limit += 2;"
            )
        );
        assert!(
            code.contains("self.buffer.put_u8(self.limit, 0);\n                self.limit += 1;")
        );
    }

    #[test]
    fn test_encoder_guard_uses_borrowed_cursor_expression() {
        let data = [info("legTag", 2)];
        let parts = [VarPart::Data(&data[0])];
        let code = generate_encoder_parts_guard(&parts, "*self.limit");
        assert!(code.contains("self.buffer.put_u16_le(*self.limit, 0);"));
        assert!(code.contains("*self.limit += 2;"));
    }

    #[test]
    fn test_reader_skip_walks_groups_and_var_data_once() {
        let data = [info("trailer", 4)];
        let parts = [
            group("Orders", false),
            group("Flags", true),
            VarPart::Data(&data[0]),
        ];
        let code = generate_reader_parts_skip(&parts, "self.pos");

        assert!(code.contains("const SBE_VAR_PARTS: u16 = 3;"));
        assert!(code.contains("fn sbe_skip_to(&mut self, target: u16) -> Result<(), DecodeError>"));
        assert!(code.contains(
            "fn sbe_advance_to(&mut self, target: u16, what: &'static str) -> Result<(), DecodeError>"
        ));
        assert!(
            code.contains("return Err(DecodeError::OutOfOrder { part: what, read: self.part });")
        );
        assert!(!code.contains("sbe_bounded"));
        assert!(code.contains(
            "self.pos = m::OrdersGroupDecoder::wrap(self.buffer, self.pos, self.acting_version)?.end_offset()?;"
        ));
        assert!(code.contains(
            "self.pos = m::FlagsGroupDecoder::wrap(self.buffer, self.pos, self.acting_version)?.end_offset()?;"
        ));
        assert!(code.contains(
            "self.pos = ironsbe_core::bounds::var_data_end(self.buffer, self.pos, 4, \"var data field 'trailer'\")?;"
        ));
        // every header is read exactly once per skipped part
        assert_eq!(code.matches("var_data_end").count(), 1);
    }

    #[test]
    fn test_reader_skip_uses_borrowed_cursor_expression() {
        let data = [info("note", 1)];
        let parts = [VarPart::Data(&data[0])];
        let code = generate_reader_parts_skip(&parts, "*self.pos");
        assert!(code.contains(
            "*self.pos = ironsbe_core::bounds::var_data_end(self.buffer, *self.pos, 1, \"var data field 'note'\")?;"
        ));
    }
}
