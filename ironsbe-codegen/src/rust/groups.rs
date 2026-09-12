//! Repeating group encoder/decoder code generation.
//!
//! Emits, per group, a group decoder (an iterator over entry decoders), an
//! entry decoder, a group encoder and an entry encoder. Nested groups are
//! emitted recursively into the same message-scoped module.
//!
//! Per SBE 1.0 a group entry is laid out as fixed-length fields, then nested
//! repeating groups, then variable-length data. A group whose entries carry
//! neither nested groups nor var data has a *fixed stride* (`blockLength`
//! bytes per entry) and keeps O(1) positioning; any other group has a
//! *variable stride* and its extent is walked entry by entry on the wire.

use ironsbe_schema::ir::{ResolvedGroup, SchemaIr, to_snake_case};

use crate::error::CodegenError;
use crate::rust::fields::{generate_entry_field_setter, generate_field_getter};
use crate::rust::var_data::{
    VarDataInfo, end_offset_parts, generate_var_data_getter, generate_var_data_setter,
    resolve_var_data,
};

/// Byte offset just past the fixed block of a group entry, as seen from an
/// entry decoder (`block_length` is the wire value from the group header).
const ENTRY_BLOCK_END: &str = "self.offset + self.block_length as usize";

/// Generator for group encoders and decoders.
pub struct GroupGenerator;

impl GroupGenerator {
    /// Creates a new group generator.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for GroupGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// A repeating group with its var data layout resolved, recursively.
pub(crate) struct GroupLayout<'g> {
    /// The group definition from the IR.
    pub(crate) group: &'g ResolvedGroup,
    /// Resolved `<data>` fields of each entry, in schema order.
    pub(crate) var_data: Vec<VarDataInfo>,
    /// Nested groups of each entry, in schema order.
    pub(crate) nested: Vec<GroupLayout<'g>>,
}

impl<'g> GroupLayout<'g> {
    /// Resolves `group` and every nested group.
    ///
    /// # Arguments
    /// * `parent_context` - Human-readable owner, e.g. `message 'Quote'`
    /// * `parent_path` - Dotted owner path, e.g. `Quote`
    ///
    /// # Errors
    /// Propagates [`resolve_var_data`] errors for this group or any nested one.
    pub(crate) fn resolve(
        ir: &SchemaIr,
        parent_context: &str,
        parent_path: &str,
        group: &'g ResolvedGroup,
    ) -> Result<Self, CodegenError> {
        let context = format!("{parent_context}, group '{}'", group.name);
        let path = format!("{parent_path}.{}", group.name);
        let var_data = resolve_var_data(ir, &context, &path, &group.var_data)?;
        let nested = group
            .nested_groups
            .iter()
            .map(|g| Self::resolve(ir, &context, &path, g))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            group,
            var_data,
            nested,
        })
    }

    /// True when every entry is exactly `blockLength` bytes: no nested
    /// groups and no var data.
    pub(crate) fn is_fixed_stride(&self) -> bool {
        self.var_data.is_empty() && self.nested.is_empty()
    }

    /// Decoder type names of the nested groups, in schema order.
    fn nested_decoder_names(&self) -> Vec<String> {
        self.nested.iter().map(|n| n.group.decoder_name()).collect()
    }
}

/// Generates the private `group_offset(index)` walker on a decoder.
///
/// Emitted on message decoders (base = end of the root block, decoders
/// qualified with the message module) and on entry decoders (base = end of
/// the entry's fixed block, decoders in the same module). Shared by the group
/// accessors, the var data accessors and the end-offset computation.
///
/// # Arguments
/// * `base_expr` - Expression for the byte offset where the first group starts
/// * `decoders` - Group decoder type names in schema order
pub(crate) fn generate_group_offset_walker(base_expr: &str, decoders: &[String]) -> String {
    let mut output = String::new();

    output.push_str(
        "    /// Byte offset of the header of the `index`-th repeating group (0-based).\n",
    );
    output.push_str("    ///\n");
    output
        .push_str("    /// Walks the preceding groups on the wire: O(1) per fixed-stride group,\n");
    output
        .push_str("    /// O(entries) per group whose entries carry nested groups or var data.\n");
    output.push_str("    #[inline]\n");
    output.push_str("    fn group_offset(&self, index: usize) -> usize {\n");
    output.push_str(&format!("        let mut pos = {base_expr};\n"));
    for (i, decoder) in decoders.iter().enumerate() {
        output.push_str(&format!("        if index == {i} {{\n"));
        output.push_str("            return pos;\n");
        output.push_str("        }\n");
        output.push_str(&format!(
            "        pos = {decoder}::wrap(self.buffer, pos).end_offset();\n"
        ));
    }
    output.push_str("        pos\n");
    output.push_str("    }\n\n");

    output
}

/// Generates a group accessor method on a decoder.
///
/// # Arguments
/// * `group_name` - Schema name of the group
/// * `decoder_type` - Group decoder type, qualified as needed from the host
/// * `index` - 0-based position of the group among its siblings
pub(crate) fn generate_group_accessor(
    group_name: &str,
    decoder_type: &str,
    index: usize,
) -> String {
    let mut output = String::new();

    output.push_str(&format!("    /// Access {group_name} repeating group.\n"));
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str(&format!(
        "    pub fn {}(&self) -> {decoder_type}<'a> {{\n",
        to_snake_case(group_name)
    ));
    output.push_str(&format!(
        "        {decoder_type}::wrap(self.buffer, self.group_offset({index}))\n"
    ));
    output.push_str("    }\n\n");

    output
}

/// Generates a group decoder, its entry decoder, and the decoders of every
/// nested group.
pub(crate) fn generate_group_decoder(ir: &SchemaIr, layout: &GroupLayout<'_>) -> String {
    let mut output = String::new();
    let group = layout.group;
    let decoder_name = group.decoder_name();
    let entry_name = group.entry_decoder_name();

    // Group decoder struct
    output.push_str(&format!("/// {} Group Decoder.\n", group.name));
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str(&format!("pub struct {}<'a> {{\n", decoder_name));
    output.push_str("    buffer: &'a [u8],\n");
    output.push_str("    block_length: u16,\n");
    output.push_str("    count: u16,\n");
    output.push_str("    index: u16,\n");
    output.push_str("    offset: usize,\n");
    output.push_str("}\n\n");

    // Group decoder implementation
    output.push_str(&format!("impl<'a> {}<'a> {{\n", decoder_name));
    output.push_str("    /// Wraps a buffer at the group header position.\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub fn wrap(buffer: &'a [u8], offset: usize) -> Self {\n");
    output.push_str("        let header = GroupHeader::wrap(buffer, offset);\n");
    output.push_str("        Self {\n");
    output.push_str("            buffer,\n");
    output.push_str("            block_length: header.block_length,\n");
    output.push_str("            count: header.num_in_group,\n");
    output.push_str("            index: 0,\n");
    output.push_str("            offset: offset + GroupHeader::ENCODED_LENGTH,\n");
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

    output.push_str("    /// Byte offset just past the last entry not yet iterated.\n");
    output.push_str("    ///\n");
    if layout.is_fixed_stride() {
        output.push_str("    /// Entries are exactly `blockLength` bytes, so this is O(1).\n");
        output.push_str("    #[inline]\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub fn end_offset(self) -> usize {\n");
        output.push_str(
            "        self.offset + self.block_length as usize * (self.count - self.index) as usize\n",
        );
    } else {
        output.push_str(
            "    /// Entries carry nested groups or var data, so the remaining entries\n",
        );
        output.push_str("    /// are walked on the wire.\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub fn end_offset(mut self) -> usize {\n");
        output.push_str("        for _ in self.by_ref() {}\n");
        output.push_str("        self.offset\n");
    }
    output.push_str("    }\n");
    output.push_str("}\n\n");

    // Iterator implementation
    output.push_str(&format!("impl<'a> Iterator for {}<'a> {{\n", decoder_name));
    output.push_str(&format!("    type Item = {}<'a>;\n\n", entry_name));
    output.push_str("    fn next(&mut self) -> Option<Self::Item> {\n");
    output.push_str("        if self.index >= self.count {\n");
    output.push_str("            return None;\n");
    output.push_str("        }\n");
    output.push_str(&format!(
        "        let entry = {}::wrap(self.buffer, self.offset, self.block_length);\n",
        entry_name
    ));
    if layout.is_fixed_stride() {
        output.push_str("        self.offset += self.block_length as usize;\n");
    } else {
        output.push_str("        self.offset = entry.end_offset();\n");
    }
    output.push_str("        self.index += 1;\n");
    output.push_str("        Some(entry)\n");
    output.push_str("    }\n\n");

    output.push_str("    fn size_hint(&self) -> (usize, Option<usize>) {\n");
    output.push_str("        let remaining = (self.count - self.index) as usize;\n");
    output.push_str("        (remaining, Some(remaining))\n");
    output.push_str("    }\n");
    output.push_str("}\n\n");

    output.push_str(&format!(
        "impl<'a> ExactSizeIterator for {}<'a> {{}}\n\n",
        decoder_name
    ));

    // Entry decoder
    output.push_str(&generate_entry_decoder(ir, layout));

    // Nested groups
    for nested in &layout.nested {
        output.push_str(&generate_group_decoder(ir, nested));
    }

    output
}

/// Generates a group entry decoder.
fn generate_entry_decoder(ir: &SchemaIr, layout: &GroupLayout<'_>) -> String {
    let mut output = String::new();
    let group = layout.group;
    let entry_name = group.entry_decoder_name();

    output.push_str(&format!("/// {} Entry Decoder.\n", group.name));
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str(&format!("pub struct {}<'a> {{\n", entry_name));
    output.push_str("    buffer: &'a [u8],\n");
    output.push_str("    offset: usize,\n");
    output.push_str("    block_length: u16,\n");
    output.push_str("}\n\n");

    output.push_str(&format!("impl<'a> {}<'a> {{\n", entry_name));
    output.push_str("    fn wrap(buffer: &'a [u8], offset: usize, block_length: u16) -> Self {\n");
    output.push_str("        Self { buffer, offset, block_length }\n");
    output.push_str("    }\n\n");

    // Field getters
    for field in &group.fields {
        output.push_str(&generate_field_getter(ir, field));
    }

    // Nested group accessors
    if !layout.nested.is_empty() {
        output.push_str(&generate_group_offset_walker(
            ENTRY_BLOCK_END,
            &layout.nested_decoder_names(),
        ));
    }
    for (index, nested) in layout.nested.iter().enumerate() {
        output.push_str(&generate_group_accessor(
            &nested.group.name,
            &nested.group.decoder_name(),
            index,
        ));
    }

    // Var data accessors (after nested groups, in schema order)
    for index in 0..layout.var_data.len() {
        output.push_str(&generate_var_data_getter(
            index,
            &layout.var_data,
            layout.nested.len(),
            ENTRY_BLOCK_END,
        ));
    }

    // End of entry
    let (prelude, end_expr) =
        end_offset_parts(&layout.var_data, layout.nested.len(), ENTRY_BLOCK_END);
    output.push_str(
        "    /// Byte offset just past this entry: fixed block, nested groups and var data.\n",
    );
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub fn end_offset(&self) -> usize {\n");
    output.push_str(&prelude);
    output.push_str(&format!("        {end_expr}\n"));
    output.push_str("    }\n");

    output.push_str("}\n\n");

    output
}

/// Generates a group encoder, its entry encoder, and the encoders of every
/// nested group.
pub(crate) fn generate_group_encoder(ir: &SchemaIr, layout: &GroupLayout<'_>) -> String {
    let mut output = String::new();
    let group = layout.group;
    let encoder_name = group.encoder_name();
    let entry_name = group.entry_encoder_name();

    // Compute effective block length: use XML value if nonzero, else derive from fields
    let effective_block_length = if group.block_length > 0 {
        group.block_length
    } else {
        group
            .fields
            .iter()
            .map(|f| f.offset + f.encoded_length)
            .max()
            .unwrap_or(0) as u16
    };

    // Group encoder struct
    output.push_str(&format!("/// {} Group Encoder.\n", group.name));
    output.push_str("///\n");
    output.push_str("/// Borrows the parent's write cursor: the header and every entry are\n");
    output.push_str("/// appended at the cursor, so the parent's `encoded_length()` and the\n");
    output.push_str("/// position of whatever follows this group stay correct.\n");
    output.push_str(&format!("pub struct {}<'a> {{\n", encoder_name));
    output.push_str("    buffer: &'a mut [u8],\n");
    output.push_str("    limit: &'a mut usize,\n");
    output.push_str("    start: usize,\n");
    output.push_str("    count: u16,\n");
    output.push_str("    index: u16,\n");
    output.push_str("}\n\n");

    // Group encoder implementation
    output.push_str(&format!("impl<'a> {}<'a> {{\n", encoder_name));
    output.push_str(&format!(
        "    /// Block length of each entry.\n\
         pub const BLOCK_LENGTH: u16 = {};\n\n",
        effective_block_length
    ));

    // wrap constructor
    output.push_str(
        "    /// Writes the group header at the cursor and advances it past the header.\n",
    );
    output.push_str("    ///\n");
    output.push_str("    /// # Arguments\n");
    output.push_str("    /// * `buffer` - Mutable buffer to write to\n");
    output.push_str(
        "    /// * `limit` - Parent's write cursor, positioned where the group header goes\n",
    );
    output.push_str("    /// * `count` - Number of entries to encode\n");
    output.push_str(
        "    pub fn wrap(buffer: &'a mut [u8], limit: &'a mut usize, count: u16) -> Self {\n",
    );
    output.push_str("        let start = *limit;\n");
    output.push_str("        let header = GroupHeader::new(Self::BLOCK_LENGTH, count);\n");
    output.push_str("        header.encode(buffer, start);\n");
    output.push_str("        *limit = start + GroupHeader::ENCODED_LENGTH;\n");
    output.push_str("        Self {\n");
    output.push_str("            buffer,\n");
    output.push_str("            limit,\n");
    output.push_str("            start,\n");
    output.push_str("            count,\n");
    output.push_str("            index: 0,\n");
    output.push_str("        }\n");
    output.push_str("    }\n\n");

    output.push_str("    /// Returns the number of entries declared for this group.\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub const fn count(&self) -> u16 {\n");
    output.push_str("        self.count\n");
    output.push_str("    }\n\n");

    // next_entry
    output.push_str(
        "    /// Returns the next entry encoder, or `None` if all entries are written.\n",
    );
    output.push_str("    ///\n");
    output.push_str("    /// Advances the cursor past the entry's fixed block");
    if layout.is_fixed_stride() {
        output.push_str(".\n");
    } else {
        output.push_str("; nested groups and\n");
        output.push_str("    /// var data written through the entry advance it further.\n");
    }
    output.push_str(&format!(
        "    pub fn next_entry(&mut self) -> Option<{}<'_>> {{\n",
        entry_name
    ));
    output.push_str("        if self.index >= self.count {\n");
    output.push_str("            return None;\n");
    output.push_str("        }\n");
    output.push_str("        let offset = *self.limit;\n");
    output.push_str("        *self.limit = offset + Self::BLOCK_LENGTH as usize;\n");
    output.push_str("        self.index += 1;\n");
    if layout.is_fixed_stride() {
        output.push_str(&format!(
            "        Some({}::wrap(&mut *self.buffer, offset))\n",
            entry_name
        ));
    } else {
        output.push_str(&format!(
            "        Some({}::wrap(&mut *self.buffer, offset, &mut *self.limit))\n",
            entry_name
        ));
    }
    output.push_str("    }\n\n");

    // encoded_length
    output
        .push_str("    /// Returns the bytes written for this group so far (header + entries).\n");
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str("    pub fn encoded_length(&self) -> usize {\n");
    output.push_str("        *self.limit - self.start\n");
    output.push_str("    }\n");
    output.push_str("}\n\n");

    // Entry encoder
    output.push_str(&generate_entry_encoder(ir, layout));

    // Nested group encoders
    for nested in &layout.nested {
        output.push_str(&generate_group_encoder(ir, nested));
    }

    output
}

/// Generates a group encoder accessor (`<group>_count(count)`) on a parent
/// encoder: a message encoder or an entry encoder with nested groups.
///
/// # Arguments
/// * `group_name` - Schema name of the group
/// * `encoder_type` - Group encoder type, qualified as needed from the host
/// * `cursor_ref` - Expression lending the parent's cursor, e.g.
///   `&mut self.limit` on a message encoder or `&mut *self.limit` on an
///   entry encoder
pub(crate) fn generate_group_encoder_accessor(
    group_name: &str,
    encoder_type: &str,
    cursor_ref: &str,
) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Begin encoding the {group_name} repeating group at the write cursor.\n"
    ));
    output.push_str("    ///\n");
    output.push_str(
        "    /// The returned encoder borrows the cursor; once it is dropped the cursor\n",
    );
    output
        .push_str("    /// sits right after the last entry written. Groups and var data must be\n");
    output.push_str("    /// written in schema order.\n");
    output.push_str(&format!(
        "    pub fn {}_count(&mut self, count: u16) -> {encoder_type}<'_> {{\n",
        to_snake_case(group_name)
    ));
    output.push_str(&format!(
        "        {encoder_type}::wrap(&mut *self.buffer, {cursor_ref}, count)\n"
    ));
    output.push_str("    }\n\n");

    output
}

/// Generates a group entry encoder.
///
/// Entries of fixed-stride groups keep the `{ buffer, offset }` shape and
/// the `wrap(buffer, offset)` constructor. Entries that carry nested groups
/// or var data also borrow the group's write cursor so those parts can be
/// appended after the fixed block.
fn generate_entry_encoder(ir: &SchemaIr, layout: &GroupLayout<'_>) -> String {
    let mut output = String::new();
    let group = layout.group;
    let entry_name = group.entry_encoder_name();
    let fixed_stride = layout.is_fixed_stride();

    output.push_str(&format!("/// {} Entry Encoder.\n", group.name));
    if !fixed_stride {
        output.push_str("///\n");
        output.push_str("/// Fixed fields are written at their offsets inside the entry block;\n");
        output.push_str("/// nested groups and var data are appended at the shared write cursor\n");
        output.push_str("/// and must be written in schema order.\n");
    }
    output.push_str(&format!("pub struct {}<'a> {{\n", entry_name));
    output.push_str("    buffer: &'a mut [u8],\n");
    output.push_str("    offset: usize,\n");
    if !fixed_stride {
        output.push_str("    limit: &'a mut usize,\n");
    }
    output.push_str("}\n\n");

    output.push_str(&format!("impl<'a> {}<'a> {{\n", entry_name));
    if fixed_stride {
        output.push_str("    /// Wraps an entry whose fixed block starts at `offset`.\n");
        output.push_str("    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {\n");
        output.push_str("        Self { buffer, offset }\n");
    } else {
        output.push_str("    /// Wraps an entry whose fixed block starts at `offset`, appending\n");
        output.push_str("    /// nested groups and var data at `limit`.\n");
        output.push_str(
            "    pub fn wrap(buffer: &'a mut [u8], offset: usize, limit: &'a mut usize) -> Self {\n",
        );
        output.push_str("        Self { buffer, offset, limit }\n");
    }
    output.push_str("    }\n\n");

    // Field setters
    for field in &group.fields {
        output.push_str(&generate_entry_field_setter(ir, field));
    }

    // Nested group encoder accessors (advance the shared cursor)
    for nested in &layout.nested {
        output.push_str(&generate_group_encoder_accessor(
            &nested.group.name,
            &nested.group.encoder_name(),
            "&mut *self.limit",
        ));
    }

    // Var data setters (append at the shared cursor)
    for info in &layout.var_data {
        output.push_str(&generate_var_data_setter(info, "*self.limit"));
    }

    output.push_str("}\n\n");

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_generator_new() {
        let generator = GroupGenerator::new();
        let _ = generator;
    }

    #[test]
    fn test_group_generator_default() {
        let generator = GroupGenerator;
        let _ = generator;
    }

    #[test]
    fn test_group_offset_walker_unrolls_one_step_per_group() {
        let code = generate_group_offset_walker(
            "self.offset + 8",
            &[
                "a::LegsGroupDecoder".to_string(),
                "a::NotesGroupDecoder".to_string(),
            ],
        );
        assert!(code.contains("let mut pos = self.offset + 8;"));
        assert!(code.contains("if index == 0 {\n            return pos;\n        }"));
        assert!(code.contains("pos = a::LegsGroupDecoder::wrap(self.buffer, pos).end_offset();"));
        assert!(code.contains("if index == 1 {\n            return pos;\n        }"));
        assert!(code.contains("pos = a::NotesGroupDecoder::wrap(self.buffer, pos).end_offset();"));
        assert!(!code.contains("group_size()"));
    }

    #[test]
    fn test_group_accessor_uses_walker_index() {
        let code = generate_group_accessor("fills", "FillsGroupDecoder", 1);
        assert!(code.contains("pub fn fills(&self) -> FillsGroupDecoder<'a> {"));
        assert!(code.contains("FillsGroupDecoder::wrap(self.buffer, self.group_offset(1))"));
    }
}
