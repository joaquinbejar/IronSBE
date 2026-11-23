//! SBE XML schema parser.
//!
//! This module provides functionality to parse FIX SBE XML schema files
//! into the internal schema representation.

use crate::error::ParseError;
use crate::messages::{DataFieldDef, FieldDef, GroupDef, MessageDef};
use crate::types::{
    ByteOrder, CompositeDef, CompositeField, EnumDef, EnumValue, InlineType, Presence,
    PrimitiveDef, PrimitiveType, Schema, SetChoice, SetDef, TypeDef,
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
                let name_owned = e.name().as_ref().to_owned();
                let name = name_owned.as_str();
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

    let mut schema = schema.ok_or_else(|| ParseError::InvalidStructure {
        message: "No messageSchema element found".to_string(),
    })?;
    register_inline_types(&mut schema);
    resolve_layout(&mut schema)?;
    Ok(schema)
}

/// Registers the types declared inside composites. Each keeps its own name
/// unless a type of that name already exists (a top-level type, or another
/// composite's inline type), in which case it gets the owning composite's
/// name as a prefix (`Status` in `Order` becomes `OrderStatus`), and the
/// owning member refers to it by that name. Inline types come before their
/// owner in the list, so an owner still waiting is updated in the list.
fn register_inline_types(schema: &mut Schema) {
    let mut pending = std::mem::take(&mut schema.inline_types);
    while !pending.is_empty() {
        let (owner, member, mut def) = pending.remove(0);
        let name = def.name().to_owned();
        let mut final_name = name.clone();
        if schema.has_type(&final_name) {
            final_name = format!("{owner}{}", capitalise(&name));
            let mut suffix = 2usize;
            while schema.has_type(&final_name) {
                final_name = format!("{owner}{}{suffix}", capitalise(&name));
                suffix += 1;
            }
            def.set_name(final_name.clone());
        }
        let owner_in_schema = schema
            .types
            .iter_mut()
            .find(|t| matches!(t, TypeDef::Composite(c) if c.name == owner));
        let owner_def = match owner_in_schema {
            Some(def) => Some(def),
            None => pending
                .iter_mut()
                .map(|(_, _, def)| def)
                .find(|t| matches!(t, TypeDef::Composite(c) if c.name == owner)),
        };
        if let Some(TypeDef::Composite(c)) = owner_def
            && let Some(field) = c.fields.get_mut(member)
        {
            field.type_name = final_name;
        }
        schema.add_type(def);
    }
}

/// `name` with its first character in upper case.
fn capitalise(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                let name_owned = e.name().as_ref().to_owned();
                let name = name_owned.as_str();
                match name {
                    "type" => {
                        let type_def = parse_primitive_type(reader, e)?;
                        schema.add_type(TypeDef::Primitive(type_def));
                        depth -= 1; // parse_primitive_type consumes the end tag
                    }
                    "composite" => {
                        let (composite, nested) = parse_composite(reader, e)?;
                        schema.inline_types.extend(nested);
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
                let name_owned = e.name().as_ref().to_owned();
                let name = name_owned.as_str();
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
                let text: &str = t.as_ref();
                let text = text.trim();
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
///
/// Returns the composite and the types declared inside it (an `<enum>`, a
/// `<set>` or a `<composite>` nested in a composite is also a type of the
/// schema, named by its own `name`). Every child element consumes its own end
/// tag, so the loop ends at the composite's end tag and never at a nested
/// one. Member offsets and the lengths of members that refer to other types
/// are resolved after the whole schema is read (see [`resolve_layout`]),
/// because SBE lets a composite refer to a type declared after it.
fn parse_composite(
    reader: &mut Reader<&[u8]>,
    e: &BytesStart<'_>,
) -> Result<(CompositeDef, Vec<InlineType>), ParseError> {
    let mut name = String::new();
    let mut description = None;
    let mut semantic_type = None;

    for attr in e.attributes().flatten() {
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
    let mut nested_types = Vec::new();

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) => {
                let name_owned = e.name().as_ref().to_owned();
                match name_owned.as_str() {
                    "type" => composite.add_field(parse_composite_field(e, None)?),
                    "ref" => composite.add_field(parse_composite_ref(e)?),
                    _ => {}
                }
            }
            Ok(Event::Start(ref e)) => {
                let name_owned = e.name().as_ref().to_owned();
                match name_owned.as_str() {
                    "type" => {
                        let text = read_element_text(reader)?;
                        composite.add_field(parse_composite_field(e, text)?);
                    }
                    "ref" => {
                        let _ = read_element_text(reader)?;
                        composite.add_field(parse_composite_ref(e)?);
                    }
                    "enum" => {
                        let offset = offset_attr(e)?;
                        let enum_def = parse_enum(reader, e)?;
                        let mut member = CompositeField::new(
                            enum_def.name.clone(),
                            enum_def.name.clone(),
                            enum_def.encoding_type.size(),
                        );
                        member.offset = offset;
                        nested_types.push((
                            composite.name.clone(),
                            composite.fields.len(),
                            TypeDef::Enum(enum_def),
                        ));
                        composite.add_field(member);
                    }
                    "set" => {
                        let offset = offset_attr(e)?;
                        let set_def = parse_set(reader, e)?;
                        let mut member = CompositeField::new(
                            set_def.name.clone(),
                            set_def.name.clone(),
                            set_def.encoding_type.size(),
                        );
                        member.offset = offset;
                        nested_types.push((
                            composite.name.clone(),
                            composite.fields.len(),
                            TypeDef::Set(set_def),
                        ));
                        composite.add_field(member);
                    }
                    "composite" => {
                        let offset = offset_attr(e)?;
                        let (inner, inner_nested) = parse_composite(reader, e)?;
                        let mut member =
                            CompositeField::new(inner.name.clone(), inner.name.clone(), 0);
                        member.offset = offset;
                        nested_types.extend(inner_nested);
                        nested_types.push((
                            composite.name.clone(),
                            composite.fields.len(),
                            TypeDef::Composite(inner),
                        ));
                        composite.add_field(member);
                    }
                    _ => {
                        let _ = read_element_text(reader)?;
                    }
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok((composite, nested_types))
}

/// The `offset` attribute of an element, when given.
fn offset_attr(e: &BytesStart<'_>) -> Result<Option<usize>, ParseError> {
    for attr in e.attributes().flatten() {
        let key: &str = attr.key.as_ref();
        if key == "offset" {
            let value: &str = attr.value.as_ref();
            return value
                .parse()
                .map(Some)
                .map_err(|_| ParseError::invalid_attr("composite member", "offset", value));
        }
    }
    Ok(None)
}

/// Reads the text of the element just opened up to its end tag, skipping
/// any child element, and returns it trimmed (`None` when empty).
fn read_element_text(reader: &mut Reader<&[u8]>) -> Result<Option<String>, ParseError> {
    let mut buf = Vec::new();
    let mut depth = 1usize;
    let mut text = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(ref t)) if depth == 1 => {
                let chunk: &str = t.as_ref();
                text.push_str(chunk.trim());
            }
            Ok(Event::Start(_)) => depth = depth.saturating_add(1),
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
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
    Ok((!text.is_empty()).then_some(text))
}

/// Parses a `<type>` member of a composite; `text` is its content (the value
/// of a constant member).
fn parse_composite_field(
    e: &BytesStart<'_>,
    text: Option<String>,
) -> Result<CompositeField, ParseError> {
    let mut name = String::new();
    let mut primitive_type: Option<PrimitiveType> = None;
    let mut offset = None;
    let mut length = None;
    let mut is_constant = false;
    let mut semantic_type = None;
    let mut description = None;

    for attr in e.attributes().flatten() {
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
            "length" => {
                length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| ParseError::invalid_attr("type", "length", value))?,
                )
            }
            "presence" => is_constant = value == "constant",
            "semanticType" => semantic_type = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }

    let prim = primitive_type.ok_or_else(|| ParseError::missing_attr("type", "primitiveType"))?;
    let type_name = prim.sbe_name().to_string();
    // A constant takes no bytes; an array takes `length` elements.
    let encoded_length = if is_constant {
        0
    } else {
        length.map_or(prim.size(), |len| prim.size() * len)
    };

    let mut field = CompositeField::new(name, type_name, encoded_length);
    field.primitive_type = Some(prim);
    field.offset = offset;
    field.length = length;
    field.is_constant = is_constant;
    field.semantic_type = semantic_type;
    field.description = description;
    field.constant_value = if is_constant { text } else { None };

    Ok(field)
}

/// Parses a `<ref>` member of a composite: a member of a type declared
/// elsewhere in the schema, whose length is resolved after parsing.
fn parse_composite_ref(e: &BytesStart<'_>) -> Result<CompositeField, ParseError> {
    let mut name = String::new();
    let mut type_name = String::new();
    let mut offset = None;
    for attr in e.attributes().flatten() {
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();
        match key {
            "name" => name = value.to_string(),
            "type" => type_name = value.to_string(),
            "offset" => {
                offset = Some(
                    value
                        .parse()
                        .map_err(|_| ParseError::invalid_attr("ref", "offset", value))?,
                )
            }
            _ => {}
        }
    }
    if type_name.is_empty() {
        return Err(ParseError::missing_attr("ref", "type"));
    }
    let mut field = CompositeField::new(name, type_name, 0);
    field.offset = offset;
    Ok(field)
}

/// Parses an enum type definition.
fn parse_enum(reader: &mut Reader<&[u8]>, e: &BytesStart<'_>) -> Result<EnumDef, ParseError> {
    let mut name = String::new();
    let mut encoding_type: Option<PrimitiveType> = None;
    let mut null_value = None;
    let mut description = None;

    for attr in e.attributes().flatten() {
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                let name_owned = e.name().as_ref().to_owned();
                let tag_name = name_owned.as_str();
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                let text: &str = t.as_ref();
                value_str = text.trim().to_string();
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                let name_owned = e.name().as_ref().to_owned();
                let tag_name = name_owned.as_str();
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                let text: &str = t.as_ref();
                let text = text.trim();
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                let name_owned = e.name().as_ref().to_owned();
                let tag_name = name_owned.as_str();

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
    let mut explicit_offset = false;
    let mut presence = Presence::Required;
    let mut semantic_type = None;
    let mut description = None;
    let mut since_version = None;
    let mut deprecated = None;
    let mut value_ref = None;

    for attr in e.attributes().flatten() {
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                    .map_err(|_| ParseError::invalid_attr("field", "offset", value))?;
                explicit_offset = true;
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
    field.explicit_offset = explicit_offset;
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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
                let name_owned = e.name().as_ref().to_owned();
                let tag_name = name_owned.as_str();
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

/// How deep a chain of composites referring to composites may go before
/// the schema is refused as circular.
const MAX_TYPE_DEPTH: usize = 32;

/// Resolves the layout of the whole schema once every type is known, as
/// SBE 1.0 lays it out: each composite member's length (a `<ref>`, or an
/// enum, set or composite nested in it, takes its type's length; a constant
/// takes none) and offset (its `offset` attribute, else right after the
/// previous member), then each message and group field's length (a
/// primitive named by its SBE name, `uint64` for instance, needs no
/// `<type>` declaration; a constant field takes no bytes) and offset (the
/// same rule), then each `blockLength` the schema leaves out, as the end of
/// the last field.
fn resolve_layout(schema: &mut Schema) -> Result<(), ParseError> {
    // Composite members, against an immutable view of the types.
    let mut composite_layouts = Vec::new();
    for (index, type_def) in schema.types.iter().enumerate() {
        if let TypeDef::Composite(composite) = type_def {
            let mut lengths = Vec::with_capacity(composite.fields.len());
            for member in &composite.fields {
                let length = if member.is_ref() {
                    if schema.get_type(&member.type_name).is_none()
                        && PrimitiveType::from_sbe_name(&member.type_name).is_none()
                    {
                        return Err(ParseError::UnknownType {
                            type_name: member.type_name.clone(),
                            field: format!("{}.{}", composite.name, member.name),
                        });
                    }
                    type_length(schema, &member.type_name, 0)?
                } else {
                    member.encoded_length
                };
                lengths.push(length);
            }
            composite_layouts.push((index, lengths));
        }
    }
    for (index, lengths) in composite_layouts {
        if let Some(TypeDef::Composite(composite)) = schema.types.get_mut(index) {
            let mut running = 0usize;
            for (member, length) in composite.fields.iter_mut().zip(lengths) {
                member.encoded_length = length;
                let offset = member.offset.unwrap_or(running);
                member.offset = Some(offset);
                running = offset + length;
            }
        }
    }

    let mut messages = std::mem::take(&mut schema.messages);
    for msg in &mut messages {
        let end = lay_out_fields(schema, &mut msg.fields)?;
        if msg.block_length == 0 {
            msg.block_length = block_length_of(end, &msg.name)?;
        }
        for group in &mut msg.groups {
            lay_out_group(schema, group)?;
        }
    }
    schema.messages = messages;
    Ok(())
}

/// Lays out a group's fields and nested groups, and computes its
/// `blockLength` when the schema leaves it out.
fn lay_out_group(schema: &Schema, group: &mut GroupDef) -> Result<(), ParseError> {
    let end = lay_out_fields(schema, &mut group.fields)?;
    if group.block_length == 0 {
        group.block_length = block_length_of(end, &group.name)?;
    }
    for nested in &mut group.nested_groups {
        lay_out_group(schema, nested)?;
    }
    Ok(())
}

/// Sets each field's length and offset, returns the end of the last field.
fn lay_out_fields(schema: &Schema, fields: &mut [FieldDef]) -> Result<usize, ParseError> {
    let mut running = 0usize;
    let mut end = 0usize;
    for field in fields.iter_mut() {
        field.encoded_length = if field.is_constant() {
            0
        } else {
            type_length(schema, &field.type_name, 0)?
        };
        if !field.explicit_offset {
            field.offset = running;
        }
        running = field.offset + field.encoded_length;
        end = end.max(running);
    }
    Ok(end)
}

/// A `blockLength` for a fixed block ending at `end`.
fn block_length_of(end: usize, name: &str) -> Result<u16, ParseError> {
    u16::try_from(end).map_err(|_| ParseError::InvalidStructure {
        message: format!("block of '{name}' is {end} bytes, more than a u16 blockLength"),
    })
}

/// The encoded length of the type named `name`: a type of the schema, or a
/// primitive named by its SBE name. An unknown name has length 0 and is left
/// to validation.
fn type_length(schema: &Schema, name: &str, depth: usize) -> Result<usize, ParseError> {
    if depth > MAX_TYPE_DEPTH {
        return Err(ParseError::InvalidStructure {
            message: format!("type '{name}' refers to itself through its composites"),
        });
    }
    match schema.get_type(name) {
        Some(TypeDef::Composite(composite)) => {
            // The extent of the members: an explicit offset's gap counts.
            let mut running = 0usize;
            let mut extent = 0usize;
            for member in &composite.fields {
                let length = if member.is_ref() {
                    if schema.get_type(&member.type_name).is_none()
                        && PrimitiveType::from_sbe_name(&member.type_name).is_none()
                    {
                        return Err(ParseError::UnknownType {
                            type_name: member.type_name.clone(),
                            field: format!("{}.{}", composite.name, member.name),
                        });
                    }
                    type_length(schema, &member.type_name, depth + 1)?
                } else {
                    member.encoded_length
                };
                let offset = member.offset.unwrap_or(running);
                running = offset + length;
                extent = extent.max(running);
            }
            Ok(extent)
        }
        Some(type_def) => Ok(type_def.encoded_length()),
        None => Ok(PrimitiveType::from_sbe_name(name).map_or(0, |prim| prim.size())),
    }
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
        let key: &str = attr.key.as_ref();
        let value: &str = attr.value.as_ref();

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

    #[test]
    fn test_group_field_offsets_auto_computed() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
    </types>
    <sbe:message name="TestMsg" id="1" blockLength="0">
        <group name="entries" id="100" dimensionType="groupSizeEncoding" blockLength="20">
            <field name="orderId" id="1" type="uint64" offset="0"/>
            <field name="instrumentId" id="2" type="uint32"/>
            <field name="quantity" id="3" type="uint64"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse schema");
        let group = &schema.messages[0].groups[0];
        assert_eq!(group.fields[0].name, "orderId");
        assert_eq!(group.fields[0].offset, 0);
        assert_eq!(group.fields[1].name, "instrumentId");
        assert_eq!(group.fields[1].offset, 8);
        assert_eq!(group.fields[2].name, "quantity");
        assert_eq!(group.fields[2].offset, 12);
    }

    /// A schema with no `offset` and no `blockLength` attributes, fields of
    /// builtin primitives, enums, a set, arrays and a composite that nests an
    /// enum, refers to a type declared after it and carries constants, laid
    /// out as SBE 1.0 does.
    const IMPLICIT_LAYOUT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="layout" id="1" version="0" byteOrder="littleEndian">
    <types>
        <type name="Codes" primitiveType="char" length="3"/>
        <composite name="Booster">
            <enum name="BoostType" encodingType="char">
                <validValue name="TURBO">T</validValue>
            </enum>
            <type name="horsePower" primitiveType="uint8"/>
        </composite>
        <composite name="Engine">
            <type name="capacity" primitiveType="uint16"/>
            <type name="maxRpm" primitiveType="uint16" presence="constant">9000</type>
            <type name="code" primitiveType="char" length="3"/>
            <ref name="enabled" type="Flag"/>
            <ref name="booster" type="Booster"/>
        </composite>
        <enum name="Flag" encodingType="uint8">
            <validValue name="F">0</validValue>
            <validValue name="T">1</validValue>
        </enum>
        <set name="Extras" encodingType="uint8">
            <choice name="sunRoof">0</choice>
        </set>
    </types>
    <sbe:message name="Car" id="1">
        <field name="serial" id="1" type="uint64"/>
        <field name="flag" id="2" type="Flag"/>
        <field name="codes" id="3" type="Codes"/>
        <field name="extras" id="4" type="Extras"/>
        <field name="model" id="5" type="Flag" presence="constant" valueRef="Flag.T"/>
        <field name="engine" id="6" type="Engine"/>
        <group name="figures" id="7" dimensionType="groupSizeEncoding">
            <field name="speed" id="8" type="uint16"/>
            <field name="mpg" id="9" type="float"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

    #[test]
    fn test_implicit_offsets_follow_encoded_lengths() {
        let schema = parse_schema(IMPLICIT_LAYOUT).expect("parse");
        let msg = &schema.messages[0];
        let layout: Vec<_> = msg
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.offset, f.encoded_length))
            .collect();
        assert_eq!(
            layout,
            [
                ("serial", 0, 8),
                ("flag", 8, 1),
                ("codes", 9, 3),
                ("extras", 12, 1),
                ("model", 13, 0),
                ("engine", 13, 8),
            ]
        );
        assert_eq!(msg.block_length, 21, "computed when the schema omits it");
        let group = &msg.groups[0];
        assert_eq!((group.fields[1].offset, group.block_length), (2, 6));
    }

    #[test]
    fn test_composite_members_nest_refer_and_skip_constants() {
        let schema = parse_schema(IMPLICIT_LAYOUT).expect("parse");
        // The enum nested in a composite is a type of the schema too.
        assert!(schema.get_type("BoostType").is_some());
        let Some(TypeDef::Composite(engine)) = schema.get_type("Engine") else {
            panic!("Engine is a composite");
        };
        let members: Vec<_> = engine
            .fields
            .iter()
            .map(|m| (m.name.as_str(), m.offset, m.encoded_length))
            .collect();
        assert_eq!(
            members,
            [
                ("capacity", Some(0), 2),
                ("maxRpm", Some(2), 0),
                ("code", Some(2), 3),
                ("enabled", Some(5), 1),
                ("booster", Some(6), 2),
            ]
        );
        assert_eq!(engine.fields[1].constant_value.as_deref(), Some("9000"));
        assert_eq!(engine.encoded_length(), 8);
    }

    #[test]
    fn test_declared_block_length_is_kept() {
        let xml = IMPLICIT_LAYOUT.replace(
            r#"<sbe:message name="Car" id="1">"#,
            r#"<sbe:message name="Car" id="1" blockLength="32">"#,
        );
        let schema = parse_schema(&xml).expect("parse");
        assert_eq!(schema.messages[0].block_length, 32);
    }

    /// Wraps `types` and `fields` in a schema with one message `M`.
    fn schema_with(types: &str, fields: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="edge" id="1" version="0" byteOrder="littleEndian">
    <types>{types}</types>
    <sbe:message name="M" id="1">{fields}</sbe:message>
</sbe:messageSchema>"#
        )
    }

    /// Codex review of #79, P1: a gap an explicit member offset leaves is
    /// part of the composite, so the next field starts after it.
    #[test]
    fn test_composite_extent_counts_explicit_offset_gaps() {
        let xml = schema_with(
            r#"<composite name="Pad">
                <type name="a" primitiveType="uint8" offset="0"/>
                <type name="b" primitiveType="uint32" offset="4"/>
            </composite>"#,
            r#"<field name="p" id="1" type="Pad"/>
               <field name="q" id="2" type="uint8"/>"#,
        );
        let schema = parse_schema(&xml).expect("parse");
        assert_eq!(schema.get_type("Pad").map(TypeDef::encoded_length), Some(8));
        let fields = &schema.messages[0].fields;
        assert_eq!((fields[0].encoded_length, fields[1].offset), (8, 8));
        assert_eq!(schema.messages[0].block_length, 9);
    }

    /// Codex review of #79, P2: two composites each declaring an inline enum
    /// `Status` keep their own: the second is registered under its owner's
    /// name and its member refers to it.
    #[test]
    fn test_same_named_inline_types_stay_apart() {
        let xml = schema_with(
            r#"<composite name="Order">
                <enum name="Status" encodingType="uint8"><validValue name="A">1</validValue></enum>
            </composite>
            <composite name="Fill">
                <enum name="Status" encodingType="uint16"><validValue name="B">2</validValue></enum>
            </composite>"#,
            r#"<field name="o" id="1" type="Order"/>
               <field name="f" id="2" type="Fill"/>"#,
        );
        let schema = parse_schema(&xml).expect("parse");
        let Some(TypeDef::Composite(order)) = schema.get_type("Order") else {
            panic!("Order");
        };
        let Some(TypeDef::Composite(fill)) = schema.get_type("Fill") else {
            panic!("Fill");
        };
        assert_eq!(order.fields[0].type_name, "Status");
        assert_eq!(fill.fields[0].type_name, "FillStatus");
        assert_eq!(
            fill.fields[0].name, "Status",
            "the member keeps its local name"
        );
        assert_eq!(
            schema.get_type("Status").map(TypeDef::encoded_length),
            Some(1)
        );
        assert_eq!(
            schema.get_type("FillStatus").map(TypeDef::encoded_length),
            Some(2)
        );
        assert_eq!((order.encoded_length(), fill.encoded_length()), (1, 2));
    }

    /// Codex review of #79, P2: an inline enum, set or composite keeps its
    /// explicit offset.
    #[test]
    fn test_inline_members_keep_their_offsets() {
        let xml = schema_with(
            r#"<composite name="C">
                <type name="x" primitiveType="uint8"/>
                <enum name="E" encodingType="uint8" offset="4"><validValue name="A">1</validValue></enum>
                <set name="S" encodingType="uint8" offset="6"><choice name="a">0</choice></set>
                <composite name="Inner" offset="8"><type name="y" primitiveType="uint16"/></composite>
            </composite>"#,
            r#"<field name="c" id="1" type="C"/>"#,
        );
        let schema = parse_schema(&xml).expect("parse");
        let Some(TypeDef::Composite(c)) = schema.get_type("C") else {
            panic!("C");
        };
        let offsets: Vec<_> = c.fields.iter().map(|m| m.offset).collect();
        assert_eq!(offsets, [Some(0), Some(4), Some(6), Some(8)]);
        assert_eq!(c.encoded_length(), 10);
    }

    /// Codex review of #79, P2: a `<ref>` to a type the schema does not
    /// declare is an error, not a member of 0 bytes.
    #[test]
    fn test_unknown_ref_is_an_error() {
        let xml = schema_with(
            r#"<composite name="C">
                <type name="x" primitiveType="uint8"/>
                <ref name="y" type="Missing"/>
            </composite>"#,
            r#"<field name="c" id="1" type="C"/>"#,
        );
        match parse_schema(&xml) {
            Err(ParseError::UnknownType { type_name, field }) => {
                assert_eq!((type_name.as_str(), field.as_str()), ("Missing", "C.y"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn test_group_field_offsets_explicit_preserved() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
    </types>
    <sbe:message name="TestMsg" id="1" blockLength="0">
        <group name="entries" id="100" dimensionType="groupSizeEncoding" blockLength="24">
            <field name="orderId" id="1" type="uint64" offset="0"/>
            <field name="instrumentId" id="2" type="uint32" offset="8"/>
            <field name="quantity" id="3" type="uint64" offset="16"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse schema");
        let group = &schema.messages[0].groups[0];
        assert_eq!(group.fields[0].offset, 0);
        assert_eq!(group.fields[1].offset, 8);
        assert_eq!(group.fields[2].offset, 16);
    }
}
