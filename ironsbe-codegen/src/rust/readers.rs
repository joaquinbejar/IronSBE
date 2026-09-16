//! Sequential (single-pass) reader code generation.
//!
//! Emits, per message with at least one var data field or variable-stride
//! group, a `<Message>Reader` that decodes the message front to back with
//! one advancing cursor, plus a `<Group>GroupReader` / `<Group>EntryReader`
//! pair for each of its variable-stride groups (recursively). Fixed-stride
//! groups are served by the existing `<Group>GroupDecoder` iterator, whose
//! extent is O(1), so the reader steps past them without walking.
//!
//! Every group header, entry and var data header is visited exactly once:
//! a message with N group entries and M var data fields costs O(N + M)
//! header reads, where the random-access decoders re-walk the preceding
//! variable-stride groups on every accessor call (issue #64).
//!
//! The random-access decoders emitted by `messages` and `groups` are left
//! untouched. The readers reuse their field getters and their `end_offset()`
//! walkers to skip parts the caller did not read, so both APIs agree on the
//! wire layout by construction.

use ironsbe_schema::ir::{ResolvedMessage, SchemaIr, to_snake_case};

use crate::rust::fields::generate_field_getter;
use crate::rust::groups::GroupLayout;
use crate::rust::var_data::VarDataInfo;
use crate::rust::var_parts::{VarPart, collect_var_parts, generate_reader_parts_skip};

/// True when a message gets a sequential reader: it has var data, or at
/// least one of its repeating groups has a variable stride. Messages with
/// only fixed-stride groups already position every part in O(1).
pub(crate) fn message_needs_reader(groups: &[GroupLayout<'_>], var_data: &[VarDataInfo]) -> bool {
    !var_data.is_empty() || groups.iter().any(|g| !g.is_fixed_stride())
}

/// Generates the sequential reader of a message.
///
/// Emitted next to the message decoder, so it can reuse the decoder's
/// header validation and reach the message-scoped group module.
pub(crate) fn generate_message_reader(
    ir: &SchemaIr,
    msg: &ResolvedMessage,
    groups: &[GroupLayout<'_>],
    var_data: &[VarDataInfo],
) -> String {
    let mut output = String::new();
    let reader_name = msg.reader_name();
    let decoder_name = msg.decoder_name();
    let mod_name = to_snake_case(&msg.name);
    let parts = collect_var_parts(groups, var_data, &format!("{mod_name}::"));

    output.push_str(&format!(
        "/// {} Reader: sequential, single-pass decoder.\n",
        msg.name
    ));
    output.push_str("///\n");
    output.push_str("/// Reads the message front to back with one advancing cursor: fixed\n");
    output.push_str("/// fields first, then each repeating group, then each var data field,\n");
    output.push_str("/// all in schema order. Every group header, entry and var data header\n");
    output.push_str("/// is visited exactly once, so a message with N group entries and M var\n");
    output.push_str("/// data fields costs O(N + M) header reads. The random-access\n");
    output.push_str(&format!(
        "/// [`{decoder_name}`] re-walks the preceding variable-stride groups on\n"
    ));
    output.push_str("/// every accessor call instead.\n");
    output.push_str("///\n");
    output.push_str("/// Parts the caller does not read are walked when a later part is read\n");
    output.push_str("/// or on `finish()`, so reading only the last field is still one pass.\n");
    output.push_str("/// Calling an accessor and ignoring its result skips that part.\n");
    output.push_str("///\n");
    output.push_str("/// # Panics\n");
    output.push_str("/// Accessors panic when called out of schema order (a part earlier than\n");
    output.push_str("/// the last one read) and when the buffer is shorter than the wire\n");
    output.push_str("/// lengths claim.\n");
    output.push_str("#[derive(Debug)]\n");
    output.push_str(&format!("pub struct {reader_name}<'a> {{\n"));
    output.push_str("    buffer: &'a [u8],\n");
    output.push_str("    offset: usize,\n");
    output.push_str("    acting_version: u16,\n");
    output.push_str("    pos: usize,\n");
    output.push_str("    part: u16,\n");
    output.push_str("}\n\n");

    output.push_str(&format!("impl<'a> {reader_name}<'a> {{\n"));
    output.push_str(&format!(
        "    /// Template ID for this message.\n\
         pub const TEMPLATE_ID: u16 = {};\n",
        msg.template_id
    ));
    output.push_str(&format!(
        "    /// Block length of the fixed portion.\n\
         pub const BLOCK_LENGTH: u16 = {};\n\n",
        msg.block_length
    ));

    output.push_str("    /// Wraps a buffer at the root block (after the message header).\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Arguments\n");
    output.push_str("    /// * `buffer` - Buffer containing the message\n");
    output.push_str("    /// * `offset` - Offset to the start of the root block (after header)\n");
    output.push_str("    /// * `acting_version` - Schema version for compatibility\n");
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str(
        "    pub fn wrap(buffer: &'a [u8], offset: usize, acting_version: u16) -> Self {\n",
    );
    output.push_str("        Self {\n");
    output.push_str("            buffer,\n");
    output.push_str("            offset,\n");
    output.push_str("            acting_version,\n");
    output.push_str("            pos: offset + Self::BLOCK_LENGTH as usize,\n");
    output.push_str("            part: 0,\n");
    output.push_str("        }\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Validates the message header and wraps the message, with the same\n");
    output.push_str(&format!("    /// checks as `{decoder_name}::decode`.\n"));
    output.push_str("    ///\n");
    output.push_str("    /// # Errors\n");
    output.push_str("    /// Returns an error if the buffer is shorter than the header and root\n");
    output.push_str("    /// block, or if the template or schema id does not match.\n");
    output.push_str("    pub fn decode(buffer: &'a [u8]) -> Result<Self, DecodeError> {\n");
    output.push_str(&format!(
        "        let decoder = {decoder_name}::decode(buffer)?;\n"
    ));
    output.push_str("        Ok(Self::wrap(\n");
    output.push_str("            buffer,\n");
    output.push_str("            MessageHeader::ENCODED_LENGTH,\n");
    output.push_str("            decoder.acting_version,\n");
    output.push_str("        ))\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Schema version the message was encoded with.\n");
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub const fn acting_version(&self) -> u16 {\n");
    output.push_str("        self.acting_version\n");
    output.push_str("    }\n\n");

    // Fixed field getters: same emitter as the decoder (`buffer` + `offset`)
    for field in &msg.fields {
        output.push_str(&generate_field_getter(ir, field));
    }

    // Variable parts, in schema order
    output.push_str(&generate_reader_part_accessors(
        &parts,
        "self.pos",
        "&mut self.pos",
    ));

    // Finish
    output.push_str("    /// Walks every part not read yet and returns the encoded length of\n");
    output.push_str("    /// the message in bytes: header, fixed block and variable section.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str("    /// Panics if the buffer is shorter than the wire lengths claim.\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub fn finish(mut self) -> usize {\n");
    output.push_str("        self.sbe_skip_to(Self::SBE_VAR_PARTS);\n");
    output.push_str("        MessageHeader::ENCODED_LENGTH + (self.pos - self.offset)\n");
    output.push_str("    }\n\n");

    output.push_str(&generate_reader_parts_skip(&parts, "self.pos"));
    output.push_str("}\n\n");

    output
}

/// Generates the sequential reader of a variable-stride group (group reader
/// plus entry reader) and, recursively, of its variable-stride nested
/// groups. Fixed-stride groups get nothing: their decoder already serves the
/// readers.
pub(crate) fn generate_group_reader(ir: &SchemaIr, layout: &GroupLayout<'_>) -> String {
    let mut output = String::new();
    if layout.is_fixed_stride() {
        return output;
    }
    let group = layout.group;
    let reader_name = group.reader_name();
    let entry_name = group.entry_reader_name();
    let entry_decoder = group.entry_decoder_name();

    output.push_str(&format!(
        "/// {} Group Reader: sequential entry access over the parent's cursor.\n",
        group.name
    ));
    output.push_str("///\n");
    output.push_str("/// Entries are consumed in order with `next_entry()`; each entry reader\n");
    output.push_str("/// advances the shared cursor as its parts are read, and walks the parts\n");
    output.push_str("/// it did not read when dropped. Dropping the group reader walks the\n");
    output.push_str("/// entries not consumed, so the cursor always lands right after the\n");
    output.push_str("/// group.\n");
    output.push_str("#[derive(Debug)]\n");
    output.push_str(&format!("pub struct {reader_name}<'r, 'a> {{\n"));
    output.push_str("    buffer: &'a [u8],\n");
    output.push_str("    pos: &'r mut usize,\n");
    output.push_str("    block_length: u16,\n");
    output.push_str("    count: u16,\n");
    output.push_str("    index: u16,\n");
    output.push_str("}\n\n");

    output.push_str(&format!("impl<'r, 'a> {reader_name}<'r, 'a> {{\n"));
    output.push_str("    /// Reads the group header at the cursor and advances past it.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str("    /// Panics if the buffer is shorter than the group header.\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub fn wrap(buffer: &'a [u8], pos: &'r mut usize) -> Self {\n");
    output.push_str("        let header = GroupHeader::wrap(buffer, *pos);\n");
    output.push_str("        *pos += GroupHeader::ENCODED_LENGTH;\n");
    output.push_str("        Self {\n");
    output.push_str("            buffer,\n");
    output.push_str("            pos,\n");
    output.push_str("            block_length: header.block_length,\n");
    output.push_str("            count: header.num_in_group,\n");
    output.push_str("            index: 0,\n");
    output.push_str("        }\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Returns the number of entries in the group.\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub const fn count(&self) -> u16 {\n");
    output.push_str("        self.count\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Returns true if the group is empty.\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub const fn is_empty(&self) -> bool {\n");
    output.push_str("        self.count == 0\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Returns the number of entries not consumed yet.\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub const fn remaining(&self) -> u16 {\n");
    output.push_str("        self.count - self.index\n");
    output.push_str("    }\n\n");

    output.push_str(
        "    /// Returns the next entry reader, or `None` when every entry was consumed.\n",
    );
    output.push_str("    ///\n");
    output.push_str("    /// The previous entry must be dropped first (it borrows this reader);\n");
    output.push_str("    /// on drop it walks the parts it did not read, so this entry starts\n");
    output.push_str("    /// where the wire says it does.\n");
    output.push_str(&format!(
        "    pub fn next_entry(&mut self) -> Option<{entry_name}<'_, 'a>> {{\n"
    ));
    output.push_str("        if self.index >= self.count {\n");
    output.push_str("            return None;\n");
    output.push_str("        }\n");
    output.push_str("        let offset = *self.pos;\n");
    output.push_str("        *self.pos = offset + self.block_length as usize;\n");
    output.push_str("        self.index += 1;\n");
    output.push_str(&format!(
        "        Some({entry_name}::wrap(self.buffer, offset, &mut *self.pos))\n"
    ));
    output.push_str("    }\n");
    output.push_str("}\n\n");

    output.push_str(&format!("impl Drop for {reader_name}<'_, '_> {{\n"));
    output
        .push_str("    /// Walks the entries not consumed so the cursor lands after the group.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str("    /// Panics if the buffer is shorter than the wire lengths claim.\n");
    output.push_str("    fn drop(&mut self) {\n");
    output.push_str("        if std::thread::panicking() {\n");
    output.push_str("            return;\n");
    output.push_str("        }\n");
    output.push_str("        while self.index < self.count {\n");
    output.push_str(&format!(
        "            *self.pos = {entry_decoder}::wrap(self.buffer, *self.pos, self.block_length).end_offset();\n"
    ));
    output.push_str("            self.index += 1;\n");
    output.push_str("        }\n");
    output.push_str("    }\n");
    output.push_str("}\n\n");

    output.push_str(&generate_entry_reader(ir, layout));

    for nested in &layout.nested {
        output.push_str(&generate_group_reader(ir, nested));
    }

    output
}

/// Generates the entry reader of a variable-stride group.
fn generate_entry_reader(ir: &SchemaIr, layout: &GroupLayout<'_>) -> String {
    let mut output = String::new();
    let group = layout.group;
    let entry_name = group.entry_reader_name();
    let parts = collect_var_parts(&layout.nested, &layout.var_data, "");

    output.push_str(&format!(
        "/// {} Entry Reader: sequential access to one entry.\n",
        group.name
    ));
    output.push_str("///\n");
    output.push_str("/// Fixed fields are read at their offsets inside the entry block; nested\n");
    output.push_str("/// groups and var data are read at the shared cursor, in schema order,\n");
    output.push_str("/// and any of them not read when the entry is dropped is walked so the\n");
    output.push_str("/// next entry starts where the wire says it does.\n");
    output.push_str("#[derive(Debug)]\n");
    output.push_str(&format!("pub struct {entry_name}<'e, 'a> {{\n"));
    output.push_str("    buffer: &'a [u8],\n");
    output.push_str("    offset: usize,\n");
    output.push_str("    pos: &'e mut usize,\n");
    output.push_str("    part: u16,\n");
    output.push_str("}\n\n");

    output.push_str(&format!("impl<'e, 'a> {entry_name}<'e, 'a> {{\n"));
    output.push_str("    fn wrap(buffer: &'a [u8], offset: usize, pos: &'e mut usize) -> Self {\n");
    output.push_str("        Self { buffer, offset, pos, part: 0 }\n");
    output.push_str("    }\n\n");

    for field in &group.fields {
        output.push_str(&generate_field_getter(ir, field));
    }

    output.push_str(&generate_reader_part_accessors(
        &parts,
        "*self.pos",
        "&mut *self.pos",
    ));

    output.push_str(&generate_reader_parts_skip(&parts, "*self.pos"));
    output.push_str("}\n\n");

    output.push_str(&format!("impl Drop for {entry_name}<'_, '_> {{\n"));
    output.push_str("    /// Walks the nested groups and var data not read through this entry.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str("    /// Panics if the buffer is shorter than the wire lengths claim.\n");
    output.push_str("    fn drop(&mut self) {\n");
    output.push_str("        if std::thread::panicking() {\n");
    output.push_str("            return;\n");
    output.push_str("        }\n");
    output.push_str("        self.sbe_skip_to(Self::SBE_VAR_PARTS);\n");
    output.push_str("    }\n");
    output.push_str("}\n\n");

    output
}

/// Generates one accessor per variable part of a reader host, in schema
/// order: group readers or decoders, then var data getters.
///
/// # Arguments
/// * `parts` - The host's variable parts
/// * `cursor` - Place expression of the read cursor (`self.pos` / `*self.pos`)
/// * `cursor_ref` - Expression lending the cursor to a group reader
///   (`&mut self.pos` / `&mut *self.pos`)
fn generate_reader_part_accessors(parts: &[VarPart<'_>], cursor: &str, cursor_ref: &str) -> String {
    let mut output = String::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            VarPart::Group {
                name,
                decoder_type,
                reader_type,
                fixed_stride,
                ..
            } => {
                if *fixed_stride {
                    output.push_str(&generate_fixed_group_accessor(
                        index,
                        name,
                        decoder_type,
                        cursor,
                    ));
                } else {
                    output.push_str(&generate_variable_group_accessor(
                        index,
                        name,
                        reader_type,
                        cursor_ref,
                    ));
                }
            }
            VarPart::Data(info) => {
                output.push_str(&generate_reader_var_data_getter(index, info, cursor));
            }
        }
    }
    output
}

/// Generates the accessor of a fixed-stride group on a reader: it returns
/// the existing group decoder (an iterator) and steps the cursor past the
/// group in O(1).
fn generate_fixed_group_accessor(
    part_index: usize,
    group_name: &str,
    decoder_type: &str,
    cursor: &str,
) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Reads the {group_name} repeating group at the cursor and advances past it.\n"
    ));
    output.push_str("    ///\n");
    output
        .push_str("    /// Entries are exactly `blockLength` bytes, so the returned iterator is\n");
    output
        .push_str("    /// positioned in O(1) and the cursor moves straight to the group's end.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output
        .push_str("    /// Panics if a later part was already read, or if the buffer is shorter\n");
    output.push_str("    /// than the group header claims.\n");
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn {}(&mut self) -> {decoder_type}<'a> {{\n",
        to_snake_case(group_name)
    ));
    output.push_str(&format!(
        "        self.sbe_advance_to({part_index}, \"repeating group '{group_name}'\");\n"
    ));
    output.push_str(&format!(
        "        let group = {decoder_type}::wrap(self.buffer, {cursor});\n"
    ));
    output.push_str(&format!("        {cursor} = group.end_offset();\n"));
    output.push_str(&format!("        self.part = {};\n", part_index + 1));
    output.push_str("        group\n");
    output.push_str("    }\n\n");

    output
}

/// Generates the accessor of a variable-stride group on a reader: it hands
/// out a group reader that borrows the cursor.
fn generate_variable_group_accessor(
    part_index: usize,
    group_name: &str,
    reader_type: &str,
    cursor_ref: &str,
) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Reads the {group_name} repeating group at the cursor.\n"
    ));
    output.push_str("    ///\n");
    output.push_str("    /// The returned reader borrows the cursor: entries are consumed with\n");
    output.push_str("    /// `next_entry()`, and any entry not consumed when it is dropped is\n");
    output.push_str("    /// walked so the cursor lands right after the group.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output
        .push_str("    /// Panics if a later part was already read, or if the buffer is shorter\n");
    output.push_str("    /// than the group header.\n");
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn {}(&mut self) -> {reader_type}<'_, 'a> {{\n",
        to_snake_case(group_name)
    ));
    output.push_str(&format!(
        "        self.sbe_advance_to({part_index}, \"repeating group '{group_name}'\");\n"
    ));
    output.push_str(&format!("        self.part = {};\n", part_index + 1));
    output.push_str(&format!(
        "        {reader_type}::wrap(self.buffer, {cursor_ref})\n"
    ));
    output.push_str("    }\n\n");

    output
}

/// Generates the slice and string getters of a var data field on a reader:
/// the length header is read once at the cursor, which then moves past the
/// payload.
fn generate_reader_var_data_getter(part_index: usize, info: &VarDataInfo, cursor: &str) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Var data field: {} (id={}, length header: {}).\n",
        info.name, info.id, info.length_type
    ));
    output.push_str("    ///\n");
    output
        .push_str("    /// Reads the length header at the cursor and advances past the payload.\n");
    output
        .push_str("    /// Returns the raw bytes; call and ignore the result to skip the field.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output
        .push_str("    /// Panics if a later part was already read, or if the buffer is shorter\n");
    output.push_str("    /// than the length header claims.\n");
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn {}(&mut self) -> &'a [u8] {{\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        self.sbe_advance_to({part_index}, \"var data field '{}'\");\n",
        info.name
    ));
    output.push_str(&format!(
        "        let len = self.buffer.{}({cursor}) as usize;\n",
        info.read_method
    ));
    output.push_str(&format!(
        "        let start = {cursor} + {};\n",
        info.header_length
    ));
    output.push_str(&format!("        {cursor} = start + len;\n"));
    output.push_str(&format!("        self.part = {};\n", part_index + 1));
    output.push_str("        &self.buffer[start..start + len]\n");
    output.push_str("    }\n\n");

    output.push_str(&format!(
        "    /// Var data field `{}` as UTF-8 (empty string if not valid UTF-8).\n",
        info.name
    ));
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn {}_as_str(&mut self) -> &'a str {{\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        std::str::from_utf8(self.{}()).unwrap_or(\"\")\n",
        info.accessor
    ));
    output.push_str("    }\n\n");

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironsbe_schema::ir::to_snake_case;

    fn info(name: &str, header_length: usize) -> VarDataInfo {
        let (length_type, length_rust_type, read_method, write_method) = match header_length {
            1 => ("uint8", "u8", "get_u8", "put_u8"),
            4 => ("uint32", "u32", "get_u32_le", "put_u32_le"),
            _ => ("uint16", "u16", "get_u16_le", "put_u16_le"),
        };
        VarDataInfo {
            name: name.to_string(),
            accessor: to_snake_case(name),
            id: 1,
            length_type,
            length_rust_type,
            header_length,
            read_method,
            write_method,
        }
    }

    #[test]
    fn test_fixed_group_accessor_returns_decoder_and_steps_cursor() {
        let code =
            generate_fixed_group_accessor(1, "notes", "example::NotesGroupDecoder", "self.pos");
        assert!(code.contains("pub fn notes(&mut self) -> example::NotesGroupDecoder<'a> {"));
        assert!(code.contains("self.sbe_advance_to(1, \"repeating group 'notes'\");"));
        assert!(
            code.contains("let group = example::NotesGroupDecoder::wrap(self.buffer, self.pos);")
        );
        assert!(code.contains("self.pos = group.end_offset();"));
        assert!(code.contains("self.part = 2;"));
    }

    #[test]
    fn test_variable_group_accessor_lends_cursor_to_reader() {
        let code =
            generate_variable_group_accessor(0, "fills", "FillsGroupReader", "&mut *self.pos");
        assert!(code.contains("pub fn fills(&mut self) -> FillsGroupReader<'_, 'a> {"));
        assert!(code.contains("self.sbe_advance_to(0, \"repeating group 'fills'\");"));
        assert!(code.contains("self.part = 1;"));
        assert!(code.contains("FillsGroupReader::wrap(self.buffer, &mut *self.pos)"));
    }

    #[test]
    fn test_reader_var_data_getter_reads_header_once_and_advances() {
        let code = generate_reader_var_data_getter(2, &info("legTag", 2), "*self.pos");
        assert!(code.contains("pub fn leg_tag(&mut self) -> &'a [u8] {"));
        assert!(code.contains("self.sbe_advance_to(2, \"var data field 'legTag'\");"));
        assert!(code.contains("let len = self.buffer.get_u16_le(*self.pos) as usize;"));
        assert!(code.contains("let start = *self.pos + 2;"));
        assert!(code.contains("*self.pos = start + len;"));
        assert!(code.contains("self.part = 3;"));
        assert!(code.contains("&self.buffer[start..start + len]"));
        assert!(code.contains("pub fn leg_tag_as_str(&mut self) -> &'a str {"));
        assert_eq!(
            code.matches("get_u16_le").count(),
            1,
            "one header read per call"
        );
    }
}
