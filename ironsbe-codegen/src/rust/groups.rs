//! Repeating group encoder/decoder code generation.
//!
//! Emits, per group, a group decoder (an iterator over entry decoders), an
//! entry decoder, a group encoder and an entry encoder. Nested groups are
//! emitted recursively into the same message-scoped module.

use ironsbe_schema::ir::{ResolvedGroup, SchemaIr};

use crate::rust::fields::{generate_entry_field_setter, generate_field_getter};

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

/// Generates a group decoder, its entry decoder, and the decoders of every
/// nested group.
pub(crate) fn generate_group_decoder(ir: &SchemaIr, group: &ResolvedGroup) -> String {
    let mut output = String::new();
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
        "        let entry = {}::wrap(self.buffer, self.offset);\n",
        entry_name
    ));
    output.push_str("        self.offset += self.block_length as usize;\n");
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
    output.push_str(&generate_entry_decoder(ir, group));

    // Nested groups
    for nested in &group.nested_groups {
        output.push_str(&generate_group_decoder(ir, nested));
    }

    output
}

/// Generates a group entry decoder.
fn generate_entry_decoder(ir: &SchemaIr, group: &ResolvedGroup) -> String {
    let mut output = String::new();
    let entry_name = group.entry_decoder_name();

    output.push_str(&format!("/// {} Entry Decoder.\n", group.name));
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str(&format!("pub struct {}<'a> {{\n", entry_name));
    output.push_str("    buffer: &'a [u8],\n");
    output.push_str("    offset: usize,\n");
    output.push_str("}\n\n");

    output.push_str(&format!("impl<'a> {}<'a> {{\n", entry_name));
    output.push_str("    fn wrap(buffer: &'a [u8], offset: usize) -> Self {\n");
    output.push_str("        Self { buffer, offset }\n");
    output.push_str("    }\n\n");

    // Field getters
    for field in &group.fields {
        output.push_str(&generate_field_getter(ir, field));
    }

    output.push_str("}\n\n");

    output
}

/// Generates a group encoder, its entry encoder, and the encoders of every
/// nested group.
pub(crate) fn generate_group_encoder(ir: &SchemaIr, group: &ResolvedGroup) -> String {
    let mut output = String::new();
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
    output.push_str(&format!("pub struct {}<'a> {{\n", encoder_name));
    output.push_str("    buffer: &'a mut [u8],\n");
    output.push_str("    count: u16,\n");
    output.push_str("    index: u16,\n");
    output.push_str("    offset: usize,\n");
    output.push_str("}\n\n");

    // Group encoder implementation
    output.push_str(&format!("impl<'a> {}<'a> {{\n", encoder_name));
    output.push_str(&format!(
        "    /// Block length of each entry.\n\
         pub const BLOCK_LENGTH: u16 = {};\n\n",
        effective_block_length
    ));

    // wrap constructor
    output.push_str("    /// Wraps a buffer at the group header position, writing the header.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Arguments\n");
    output.push_str("    /// * `buffer` - Mutable buffer to write to\n");
    output.push_str("    /// * `offset` - Offset of the group header\n");
    output.push_str("    /// * `count` - Number of entries to encode\n");
    output.push_str("    pub fn wrap(buffer: &'a mut [u8], offset: usize, count: u16) -> Self {\n");
    output.push_str("        let header = GroupHeader::new(Self::BLOCK_LENGTH, count);\n");
    output.push_str("        header.encode(buffer, offset);\n");
    output.push_str("        Self {\n");
    output.push_str("            buffer,\n");
    output.push_str("            count,\n");
    output.push_str("            index: 0,\n");
    output.push_str("            offset: offset + GroupHeader::ENCODED_LENGTH,\n");
    output.push_str("        }\n");
    output.push_str("    }\n\n");

    // next_entry
    output.push_str(
        "    /// Returns the next entry encoder, or `None` if all entries are written.\n",
    );
    output.push_str(&format!(
        "    pub fn next_entry(&mut self) -> Option<{}<'_>> {{\n",
        entry_name
    ));
    output.push_str("        if self.index >= self.count {\n");
    output.push_str("            return None;\n");
    output.push_str("        }\n");
    output.push_str("        let offset = self.offset;\n");
    output.push_str("        self.offset += Self::BLOCK_LENGTH as usize;\n");
    output.push_str("        self.index += 1;\n");
    output.push_str(&format!(
        "        Some({}::wrap(&mut *self.buffer, offset))\n",
        entry_name
    ));
    output.push_str("    }\n\n");

    // encoded_length
    output.push_str(
        "    /// Returns the total encoded length of this group (header + all entries).\n",
    );
    output.push_str("    #[must_use]\n");
    output.push_str("    pub const fn encoded_length(&self) -> usize {\n");
    output.push_str(
        "        GroupHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize * self.count as usize\n",
    );
    output.push_str("    }\n");
    output.push_str("}\n\n");

    // Entry encoder
    output.push_str(&generate_entry_encoder(ir, group));

    // Nested group encoders
    for nested in &group.nested_groups {
        output.push_str(&generate_group_encoder(ir, nested));
    }

    output
}

/// Generates a group entry encoder.
fn generate_entry_encoder(ir: &SchemaIr, group: &ResolvedGroup) -> String {
    let mut output = String::new();
    let entry_name = group.entry_encoder_name();

    output.push_str(&format!("/// {} Entry Encoder.\n", group.name));
    output.push_str(&format!("pub struct {}<'a> {{\n", entry_name));
    output.push_str("    buffer: &'a mut [u8],\n");
    output.push_str("    offset: usize,\n");
    output.push_str("}\n\n");

    output.push_str(&format!("impl<'a> {}<'a> {{\n", entry_name));
    output.push_str("    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {\n");
    output.push_str("        Self { buffer, offset }\n");
    output.push_str("    }\n\n");

    // Field setters
    for field in &group.fields {
        output.push_str(&generate_entry_field_setter(ir, field));
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
}
