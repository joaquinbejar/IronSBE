//! Message definitions for SBE schemas.
//!
//! This module contains the data structures representing SBE message definitions
//! including fields, groups, and variable-length data.

use crate::types::Presence;

/// Message definition.
#[derive(Debug, Clone)]
pub struct MessageDef {
    /// Message name.
    pub name: String,
    /// Message template ID.
    pub id: u16,
    /// Block length (fixed portion size in bytes).
    pub block_length: u16,
    /// Semantic type.
    pub semantic_type: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Since version.
    pub since_version: Option<u16>,
    /// Deprecated since version.
    pub deprecated: Option<u16>,
    /// Fixed fields in the root block.
    pub fields: Vec<FieldDef>,
    /// Repeating groups.
    pub groups: Vec<GroupDef>,
    /// Variable-length data fields.
    pub data_fields: Vec<DataFieldDef>,
}

impl MessageDef {
    /// Creates a new message definition.
    #[must_use]
    pub fn new(name: String, id: u16, block_length: u16) -> Self {
        Self {
            name,
            id,
            block_length,
            semantic_type: None,
            description: None,
            since_version: None,
            deprecated: None,
            fields: Vec::new(),
            groups: Vec::new(),
            data_fields: Vec::new(),
        }
    }

    /// Adds a field to the message.
    pub fn add_field(&mut self, field: FieldDef) {
        self.fields.push(field);
    }

    /// Adds a group to the message.
    pub fn add_group(&mut self, group: GroupDef) {
        self.groups.push(group);
    }

    /// Adds a data field to the message.
    pub fn add_data_field(&mut self, data_field: DataFieldDef) {
        self.data_fields.push(data_field);
    }

    /// Returns true if the message has any repeating groups.
    #[must_use]
    pub fn has_groups(&self) -> bool {
        !self.groups.is_empty()
    }

    /// Returns true if the message has any variable-length data.
    #[must_use]
    pub fn has_var_data(&self) -> bool {
        !self.data_fields.is_empty()
    }

    /// Calculates the minimum encoded length (header + block).
    #[must_use]
    pub fn min_encoded_length(&self) -> usize {
        8 + self.block_length as usize // MessageHeader + block
    }
}

/// Field definition within a message or group.
#[derive(Debug, Clone)]
pub struct FieldDef {
    /// Field name.
    pub name: String,
    /// Field ID (tag).
    pub id: u16,
    /// Type name (references a type definition).
    pub type_name: String,
    /// Offset within the block.
    pub offset: usize,
    /// Field presence.
    pub presence: Presence,
    /// Semantic type.
    pub semantic_type: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Since version.
    pub since_version: Option<u16>,
    /// Deprecated since version.
    pub deprecated: Option<u16>,
    /// Constant value (if presence is Constant).
    pub value_ref: Option<String>,
    /// Encoded length in bytes (resolved from type).
    pub encoded_length: usize,
}

impl FieldDef {
    /// Creates a new field definition.
    #[must_use]
    pub fn new(name: String, id: u16, type_name: String, offset: usize) -> Self {
        Self {
            name,
            id,
            type_name,
            offset,
            presence: Presence::Required,
            semantic_type: None,
            description: None,
            since_version: None,
            deprecated: None,
            value_ref: None,
            encoded_length: 0,
        }
    }

    /// Returns true if the field is optional.
    #[must_use]
    pub fn is_optional(&self) -> bool {
        self.presence == Presence::Optional
    }

    /// Returns true if the field has a constant value.
    #[must_use]
    pub fn is_constant(&self) -> bool {
        self.presence == Presence::Constant
    }

    /// Returns the end offset (offset + length).
    #[must_use]
    pub fn end_offset(&self) -> usize {
        self.offset + self.encoded_length
    }
}

/// Repeating group definition.
#[derive(Debug, Clone)]
pub struct GroupDef {
    /// Group name.
    pub name: String,
    /// Group ID.
    pub id: u16,
    /// Block length of each entry.
    pub block_length: u16,
    /// Dimension type (usually groupSizeEncoding).
    pub dimension_type: String,
    /// Description.
    pub description: Option<String>,
    /// Since version.
    pub since_version: Option<u16>,
    /// Deprecated since version.
    pub deprecated: Option<u16>,
    /// Fields within each group entry.
    pub fields: Vec<FieldDef>,
    /// Nested groups within each entry.
    pub nested_groups: Vec<GroupDef>,
    /// Variable-length data within each entry.
    pub data_fields: Vec<DataFieldDef>,
}

impl GroupDef {
    /// Creates a new group definition.
    #[must_use]
    pub fn new(name: String, id: u16, block_length: u16) -> Self {
        Self {
            name,
            id,
            block_length,
            dimension_type: "groupSizeEncoding".to_string(),
            description: None,
            since_version: None,
            deprecated: None,
            fields: Vec::new(),
            nested_groups: Vec::new(),
            data_fields: Vec::new(),
        }
    }

    /// Adds a field to the group.
    pub fn add_field(&mut self, field: FieldDef) {
        self.fields.push(field);
    }

    /// Adds a nested group.
    pub fn add_nested_group(&mut self, group: GroupDef) {
        self.nested_groups.push(group);
    }

    /// Adds a data field to the group.
    pub fn add_data_field(&mut self, data_field: DataFieldDef) {
        self.data_fields.push(data_field);
    }

    /// Returns true if the group has nested groups.
    #[must_use]
    pub fn has_nested_groups(&self) -> bool {
        !self.nested_groups.is_empty()
    }

    /// Returns true if the group has variable-length data.
    #[must_use]
    pub fn has_var_data(&self) -> bool {
        !self.data_fields.is_empty()
    }

    /// Returns the header size (typically 4 bytes for groupSizeEncoding).
    #[must_use]
    pub const fn header_size(&self) -> usize {
        4 // blockLength (u16) + numInGroup (u16)
    }
}

/// Variable-length data field definition.
#[derive(Debug, Clone)]
pub struct DataFieldDef {
    /// Field name.
    pub name: String,
    /// Field ID.
    pub id: u16,
    /// Type name (e.g., "varDataEncoding").
    pub type_name: String,
    /// Description.
    pub description: Option<String>,
    /// Since version.
    pub since_version: Option<u16>,
    /// Deprecated since version.
    pub deprecated: Option<u16>,
}

impl DataFieldDef {
    /// Creates a new data field definition.
    #[must_use]
    pub fn new(name: String, id: u16, type_name: String) -> Self {
        Self {
            name,
            id,
            type_name,
            description: None,
            since_version: None,
            deprecated: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_def_creation() {
        let mut msg = MessageDef::new("NewOrderSingle".to_string(), 1, 56);
        msg.add_field(FieldDef::new(
            "clOrdId".to_string(),
            11,
            "ClOrdId".to_string(),
            0,
        ));
        msg.add_field(FieldDef::new(
            "symbol".to_string(),
            55,
            "Symbol".to_string(),
            20,
        ));

        assert_eq!(msg.name, "NewOrderSingle");
        assert_eq!(msg.id, 1);
        assert_eq!(msg.block_length, 56);
        assert_eq!(msg.fields.len(), 2);
        assert_eq!(msg.min_encoded_length(), 64); // 8 + 56
    }

    #[test]
    fn test_field_def() {
        let mut field = FieldDef::new("price".to_string(), 44, "decimal".to_string(), 30);
        field.encoded_length = 9;
        field.presence = Presence::Optional;

        assert!(field.is_optional());
        assert!(!field.is_constant());
        assert_eq!(field.end_offset(), 39);
    }

    #[test]
    fn test_group_def() {
        let mut group = GroupDef::new("mdEntries".to_string(), 268, 31);
        group.add_field(FieldDef::new(
            "securityId".to_string(),
            48,
            "uint64".to_string(),
            0,
        ));
        group.add_field(FieldDef::new(
            "rptSeq".to_string(),
            83,
            "uint32".to_string(),
            8,
        ));

        assert_eq!(group.name, "mdEntries");
        assert_eq!(group.fields.len(), 2);
        assert_eq!(group.header_size(), 4);
        assert!(!group.has_nested_groups());
        assert!(!group.has_var_data());
    }

    #[test]
    fn test_data_field_def() {
        let data = DataFieldDef::new("rawData".to_string(), 96, "varDataEncoding".to_string());
        assert_eq!(data.name, "rawData");
        assert_eq!(data.type_name, "varDataEncoding");
    }

    #[test]
    fn test_message_with_groups_and_data() {
        let mut msg = MessageDef::new("MarketDataRefresh".to_string(), 3, 16);
        msg.add_group(GroupDef::new("mdEntries".to_string(), 268, 31));
        msg.add_data_field(DataFieldDef::new(
            "rawData".to_string(),
            96,
            "varDataEncoding".to_string(),
        ));

        assert!(msg.has_groups());
        assert!(msg.has_var_data());
    }
}
