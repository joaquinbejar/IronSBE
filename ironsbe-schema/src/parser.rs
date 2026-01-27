//! SBE XML schema parser.
//!
//! This module provides functionality to parse FIX SBE XML schema files
//! into the internal schema representation.

use crate::error::ParseError;
use crate::messages::{DataFieldDef, FieldDef, GroupDef, MessageDef};
use crate::types::{
    ByteOrder, CompositeDef, CompositeField, EnumDef, EnumValue, Presence, PrimitiveDef,
    PrimitiveType, Schema, SetChoice, SetDef, TypeDef,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// Parses an SBE XML schema from a string.
///
/// # Arguments
/// * `xml` - XML schema content
///
/// # Returns
/// Parsed schema or parse error.
///
/// # Errors
/// Returns `ParseError` if the XML is malformed or contains invalid schema elements.
pub fn parse_schema(xml: &str) -> Result<Schema, ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut schema: Option<Schema> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let name = std::str::from_utf8(&name_bytes)?;
                match name {
                    "messageSchema" | "sbe:messageSchema" => {
                        schema = Some(parse_message_schema(e)?);
                    }
                    "types" if schema.is_some() => {
                        parse_types(&mut reader, schema.as_mut().unwrap())?;
                    }
                    "message" | "sbe:message" if schema.is_some() => {
                        let msg = parse_message(&mut reader, e, schema.as_ref().unwrap())?;
                        schema.as_mut().unwrap().messages.push(msg);
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    schema.ok_or_else(|| ParseError::InvalidStructure {
        message: "No messageSchema element found".to_string(),
    })
}

/// Parses the messageSchema element attributes.
fn parse_message_schema(e: &BytesStart<'_>) -> Result<Schema, ParseError> {
    let mut package = String::new();
    let mut id: u16 = 0;
    let mut version: u16 = 0;
    let mut semantic_version = String::new();
    let mut description = None;
    let mut byte_order = ByteOrder::LittleEndian;
    let mut header_type = "messageHeader".to_string();

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "package" => package = value.to_string(),
            "id" => {
                id = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("messageSchema", "id", value))?
            }
            "version" => {
                version = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("messageSchema", "version", value))?
            }
            "semanticVersion" => semantic_version = value.to_string(),
            "description" => description = Some(value.to_string()),
            "byteOrder" => {
                byte_order = ByteOrder::parse(value)
                    .ok_or_else(|| ParseError::invalid_attr("messageSchema", "byteOrder", value))?
            }
            "headerType" => header_type = value.to_string(),
            _ => {}
        }
    }

    let mut schema = Schema::new(package, id, version);
    schema.semantic_version = semantic_version;
    schema.description = description;
    schema.byte_order = byte_order;
    schema.header_type = header_type;

    Ok(schema)
}

/// Parses the types section.
fn parse_types(reader: &mut Reader<&[u8]>, schema: &mut Schema) -> Result<(), ParseError> {
    let mut buf = Vec::new();
    let mut depth = 1;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let name_bytes = e.name().as_ref().to_vec();
                let name = std::str::from_utf8(&name_bytes)?;
                match name {
                    "type" => {
                        let type_def = parse_primitive_type(reader, e)?;
                        schema.add_type(TypeDef::Primitive(type_def));
                        depth -= 1; // parse_primitive_type consumes the end tag
                    }
                    "composite" => {
                        let composite = parse_composite(reader, e)?;
                        schema.add_type(TypeDef::Composite(composite));
                        depth -= 1;
                    }
                    "enum" => {
                        let enum_def = parse_enum(reader, e)?;
                        schema.add_type(TypeDef::Enum(enum_def));
                        depth -= 1;
                    }
                    "set" => {
                        let set_def = parse_set(reader, e)?;
                        schema.add_type(TypeDef::Set(set_def));
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let name = std::str::from_utf8(&name_bytes)?;
                if name == "type" {
                    let type_def = parse_primitive_type_empty(e)?;
                    schema.add_type(TypeDef::Primitive(type_def));
                }
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

/// Parses a primitive type definition (with content).
fn parse_primitive_type(
    reader: &mut Reader<&[u8]>,
    e: &BytesStart<'_>,
) -> Result<PrimitiveDef, ParseError> {
    let mut type_def = parse_primitive_type_empty(e)?;
    let mut buf = Vec::new();

    // Read until end tag, capturing any constant value
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(ref t)) => {
                let text = std::str::from_utf8(t.as_ref())?.trim();
                if !text.is_empty() {
                    type_def.constant_value = Some(text.to_string());
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(type_def)
}

/// Parses a primitive type definition (empty element).
fn parse_primitive_type_empty(e: &BytesStart<'_>) -> Result<PrimitiveDef, ParseError> {
    let mut name = String::new();
    let mut primitive_type: Option<PrimitiveType> = None;
    let mut length: Option<usize> = None;
    let mut null_value = None;
    let mut min_value = None;
    let mut max_value = None;
    let mut character_encoding = None;
    let mut semantic_type = None;
    let mut description = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "primitiveType" => {
                primitive_type = Some(
                    PrimitiveType::from_sbe_name(value)
                        .ok_or_else(|| ParseError::invalid_attr("type", "primitiveType", value))?,
                )
            }
            "length" => {
                length = Some(
                    value
                        .parse()
                        .map_err(|_| ParseError::invalid_attr("type", "length", value))?,
                )
            }
            "nullValue" => null_value = Some(value.to_string()),
            "minValue" => min_value = Some(value.to_string()),
            "maxValue" => max_value = Some(value.to_string()),
            "characterEncoding" => character_encoding = Some(value.to_string()),
            "semanticType" => semantic_type = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }

    let primitive_type =
        primitive_type.ok_or_else(|| ParseError::missing_attr("type", "primitiveType"))?;

    let mut type_def = PrimitiveDef::new(name, primitive_type);
    type_def.length = length;
    type_def.null_value = null_value;
    type_def.min_value = min_value;
    type_def.max_value = max_value;
    type_def.character_encoding = character_encoding;
    type_def.semantic_type = semantic_type;
    type_def.description = description;

    Ok(type_def)
}

/// Parses a composite type definition.
fn parse_composite(
    reader: &mut Reader<&[u8]>,
    e: &BytesStart<'_>,
) -> Result<CompositeDef, ParseError> {
    let mut name = String::new();
    let mut description = None;
    let mut semantic_type = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "description" => description = Some(value.to_string()),
            "semanticType" => semantic_type = Some(value.to_string()),
            _ => {}
        }
    }

    let mut composite = CompositeDef::new(name);
    composite.description = description;
    composite.semantic_type = semantic_type;

    let mut buf = Vec::new();
    let mut current_offset = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let tag_name = std::str::from_utf8(&name_bytes)?;
                if tag_name == "type" {
                    let field = parse_composite_field(e, current_offset)?;
                    current_offset += field.encoded_length;
                    composite.add_field(field);
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(composite)
}

/// Parses a field within a composite type.
fn parse_composite_field(
    e: &BytesStart<'_>,
    default_offset: usize,
) -> Result<CompositeField, ParseError> {
    let mut name = String::new();
    let mut primitive_type: Option<PrimitiveType> = None;
    let mut offset = None;
    let mut semantic_type = None;
    let mut description = None;
    let constant_value = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "primitiveType" => {
                primitive_type = Some(
                    PrimitiveType::from_sbe_name(value)
                        .ok_or_else(|| ParseError::invalid_attr("type", "primitiveType", value))?,
                )
            }
            "offset" => {
                offset = Some(
                    value
                        .parse()
                        .map_err(|_| ParseError::invalid_attr("type", "offset", value))?,
                )
            }
            "semanticType" => semantic_type = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "presence" if value == "constant" => {}
            _ => {}
        }
    }

    let prim = primitive_type.ok_or_else(|| ParseError::missing_attr("type", "primitiveType"))?;
    let type_name = prim.sbe_name().to_string();
    let encoded_length = prim.size();

    let mut field = CompositeField::new(name, type_name, encoded_length);
    field.primitive_type = Some(prim);
    field.offset = offset.or(Some(default_offset));
    field.semantic_type = semantic_type;
    field.description = description;
    field.constant_value = constant_value;

    Ok(field)
}

/// Parses an enum type definition.
fn parse_enum(reader: &mut Reader<&[u8]>, e: &BytesStart<'_>) -> Result<EnumDef, ParseError> {
    let mut name = String::new();
    let mut encoding_type: Option<PrimitiveType> = None;
    let mut null_value = None;
    let mut description = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "encodingType" => {
                encoding_type = Some(
                    PrimitiveType::from_sbe_name(value)
                        .ok_or_else(|| ParseError::invalid_attr("enum", "encodingType", value))?,
                )
            }
            "nullValue" => null_value = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }

    let encoding_type =
        encoding_type.ok_or_else(|| ParseError::missing_attr("enum", "encodingType"))?;

    let mut enum_def = EnumDef::new(name, encoding_type);
    enum_def.null_value = null_value;
    enum_def.description = description;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let tag_name = std::str::from_utf8(&name_bytes)?;
                if tag_name == "validValue" {
                    let value = parse_enum_value(reader, e)?;
                    enum_def.add_value(value);
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(enum_def)
}

/// Parses an enum valid value.
fn parse_enum_value(
    reader: &mut Reader<&[u8]>,
    e: &BytesStart<'_>,
) -> Result<EnumValue, ParseError> {
    let mut name = String::new();
    let mut description = None;
    let mut since_version = None;
    let mut deprecated = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "description" => description = Some(value.to_string()),
            "sinceVersion" => since_version = value.parse().ok(),
            "deprecated" => deprecated = value.parse().ok(),
            _ => {}
        }
    }

    // Read the value content
    let mut buf = Vec::new();
    let mut value_str = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(ref t)) => {
                value_str = std::str::from_utf8(t.as_ref())?.trim().to_string();
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    let mut enum_value = EnumValue::new(name, value_str);
    enum_value.description = description;
    enum_value.since_version = since_version;
    enum_value.deprecated = deprecated;

    Ok(enum_value)
}

/// Parses a set (bitfield) type definition.
fn parse_set(reader: &mut Reader<&[u8]>, e: &BytesStart<'_>) -> Result<SetDef, ParseError> {
    let mut name = String::new();
    let mut encoding_type: Option<PrimitiveType> = None;
    let mut description = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "encodingType" => {
                encoding_type = Some(
                    PrimitiveType::from_sbe_name(value)
                        .ok_or_else(|| ParseError::invalid_attr("set", "encodingType", value))?,
                )
            }
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }

    let encoding_type =
        encoding_type.ok_or_else(|| ParseError::missing_attr("set", "encodingType"))?;

    let mut set_def = SetDef::new(name, encoding_type);
    set_def.description = description;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let tag_name = std::str::from_utf8(&name_bytes)?;
                if tag_name == "choice" {
                    let choice = parse_set_choice(reader, e)?;
                    set_def.add_choice(choice);
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(set_def)
}

/// Parses a set choice.
fn parse_set_choice(
    reader: &mut Reader<&[u8]>,
    e: &BytesStart<'_>,
) -> Result<SetChoice, ParseError> {
    let mut name = String::new();
    let mut description = None;
    let mut since_version = None;
    let mut deprecated = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "description" => description = Some(value.to_string()),
            "sinceVersion" => since_version = value.parse().ok(),
            "deprecated" => deprecated = value.parse().ok(),
            _ => {}
        }
    }

    // Read the bit position content
    let mut buf = Vec::new();
    let mut bit_position: u8 = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(ref t)) => {
                let text = std::str::from_utf8(t.as_ref())?.trim();
                bit_position = text
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("choice", "value", text))?;
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    let mut choice = SetChoice::new(name, bit_position);
    choice.description = description;
    choice.since_version = since_version;
    choice.deprecated = deprecated;

    Ok(choice)
}

/// Parses a message definition.
fn parse_message(
    reader: &mut Reader<&[u8]>,
    e: &BytesStart<'_>,
    schema: &Schema,
) -> Result<MessageDef, ParseError> {
    let mut name = String::new();
    let mut id: u16 = 0;
    let mut block_length: u16 = 0;
    let mut semantic_type = None;
    let mut description = None;
    let mut since_version = None;
    let mut deprecated = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "id" => {
                id = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("message", "id", value))?
            }
            "blockLength" => {
                block_length = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("message", "blockLength", value))?
            }
            "semanticType" => semantic_type = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "sinceVersion" => since_version = value.parse().ok(),
            "deprecated" => deprecated = value.parse().ok(),
            _ => {}
        }
    }

    let mut msg = MessageDef::new(name, id, block_length);
    msg.semantic_type = semantic_type;
    msg.description = description;
    msg.since_version = since_version;
    msg.deprecated = deprecated;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let tag_name = std::str::from_utf8(&name_bytes)?;

                match tag_name {
                    "field" => {
                        let field = parse_field(e, schema)?;
                        msg.add_field(field);
                    }
                    "group" => {
                        let group = parse_group(reader, e, schema)?;
                        msg.add_group(group);
                    }
                    "data" => {
                        let data = parse_data_field(e)?;
                        msg.add_data_field(data);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(msg)
}

/// Parses a field definition.
fn parse_field(e: &BytesStart<'_>, schema: &Schema) -> Result<FieldDef, ParseError> {
    let mut name = String::new();
    let mut id: u16 = 0;
    let mut type_name = String::new();
    let mut offset: usize = 0;
    let mut presence = Presence::Required;
    let mut semantic_type = None;
    let mut description = None;
    let mut since_version = None;
    let mut deprecated = None;
    let mut value_ref = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "id" => {
                id = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("field", "id", value))?
            }
            "type" => type_name = value.to_string(),
            "offset" => {
                offset = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("field", "offset", value))?
            }
            "presence" => {
                presence = Presence::parse(value)
                    .ok_or_else(|| ParseError::invalid_attr("field", "presence", value))?
            }
            "semanticType" => semantic_type = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "sinceVersion" => since_version = value.parse().ok(),
            "deprecated" => deprecated = value.parse().ok(),
            "valueRef" => value_ref = Some(value.to_string()),
            _ => {}
        }
    }

    let mut field = FieldDef::new(name, id, type_name.clone(), offset);
    field.presence = presence;
    field.semantic_type = semantic_type;
    field.description = description;
    field.since_version = since_version;
    field.deprecated = deprecated;
    field.value_ref = value_ref;

    // Resolve encoded length from type
    if let Some(type_def) = schema.get_type(&type_name) {
        field.encoded_length = type_def.encoded_length();
    }

    Ok(field)
}

/// Parses a group definition.
fn parse_group(
    reader: &mut Reader<&[u8]>,
    e: &BytesStart<'_>,
    schema: &Schema,
) -> Result<GroupDef, ParseError> {
    let mut name = String::new();
    let mut id: u16 = 0;
    let mut block_length: u16 = 0;
    let mut dimension_type = "groupSizeEncoding".to_string();
    let mut description = None;
    let mut since_version = None;
    let mut deprecated = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "id" => {
                id = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("group", "id", value))?
            }
            "blockLength" => {
                block_length = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("group", "blockLength", value))?
            }
            "dimensionType" => dimension_type = value.to_string(),
            "description" => description = Some(value.to_string()),
            "sinceVersion" => since_version = value.parse().ok(),
            "deprecated" => deprecated = value.parse().ok(),
            _ => {}
        }
    }

    let mut group = GroupDef::new(name, id, block_length);
    group.dimension_type = dimension_type;
    group.description = description;
    group.since_version = since_version;
    group.deprecated = deprecated;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let tag_name = std::str::from_utf8(&name_bytes)?;
                match tag_name {
                    "field" => {
                        let field = parse_field(e, schema)?;
                        group.add_field(field);
                    }
                    "group" => {
                        let nested = parse_group(reader, e, schema)?;
                        group.add_nested_group(nested);
                    }
                    "data" => {
                        let data = parse_data_field(e)?;
                        group.add_data_field(data);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(group)
}

/// Parses a data (variable-length) field definition.
fn parse_data_field(e: &BytesStart<'_>) -> Result<DataFieldDef, ParseError> {
    let mut name = String::new();
    let mut id: u16 = 0;
    let mut type_name = String::new();
    let mut description = None;
    let mut since_version = None;
    let mut deprecated = None;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref())?;
        let value = std::str::from_utf8(&attr.value)?;

        match key {
            "name" => name = value.to_string(),
            "id" => {
                id = value
                    .parse()
                    .map_err(|_| ParseError::invalid_attr("data", "id", value))?
            }
            "type" => type_name = value.to_string(),
            "description" => description = Some(value.to_string()),
            "sinceVersion" => since_version = value.parse().ok(),
            "deprecated" => deprecated = value.parse().ok(),
            _ => {}
        }
    }

    let mut data = DataFieldDef::new(name, id, type_name);
    data.description = description;
    data.since_version = since_version;
    data.deprecated = deprecated;

    Ok(data)
}

/// Skips to the end of the current element.
#[allow(dead_code)]
fn skip_to_end(reader: &mut Reader<&[u8]>, _tag_name: &str) -> Result<(), ParseError> {
    let mut buf = Vec::new();
    let mut depth = 1;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_SCHEMA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test"
                   id="1"
                   version="1"
                   semanticVersion="1.0.0"
                   byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="Symbol" primitiveType="char" length="8"/>
        <enum name="Side" encodingType="uint8">
            <validValue name="Buy">1</validValue>
            <validValue name="Sell">2</validValue>
        </enum>
    </types>
    <sbe:message name="TestMessage" id="1" blockLength="16">
        <field name="price" id="1" type="uint64" offset="0"/>
        <field name="symbol" id="2" type="Symbol" offset="8"/>
    </sbe:message>
</sbe:messageSchema>"#;

    #[test]
    fn test_parse_simple_schema() {
        let schema = parse_schema(SIMPLE_SCHEMA).expect("Failed to parse schema");

        assert_eq!(schema.package, "test");
        assert_eq!(schema.id, 1);
        assert_eq!(schema.version, 1);
        assert_eq!(schema.byte_order, ByteOrder::LittleEndian);
    }

    #[test]
    fn test_parse_types() {
        let schema = parse_schema(SIMPLE_SCHEMA).expect("Failed to parse schema");

        assert!(schema.has_type("uint64"));
        assert!(schema.has_type("Symbol"));
        assert!(schema.has_type("Side"));

        let symbol = schema.get_type("Symbol").unwrap();
        assert!(symbol.is_primitive());
        assert_eq!(symbol.encoded_length(), 8);

        let side = schema.get_type("Side").unwrap();
        assert!(side.is_enum());
    }

    #[test]
    fn test_parse_message() {
        let schema = parse_schema(SIMPLE_SCHEMA).expect("Failed to parse schema");

        assert_eq!(schema.messages.len(), 1);
        let msg = &schema.messages[0];
        assert_eq!(msg.name, "TestMessage");
        assert_eq!(msg.id, 1);
        assert_eq!(msg.block_length, 16);
        assert_eq!(msg.fields.len(), 2);
    }
}
