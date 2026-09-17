//! Version-aware block validation and `Option` getters for versioned fields.
//!
//! `decode` validates the header's declared block length against the fields
//! the message's *own* version carries: an older message with a shorter block
//! decodes and its newer fields read as `None`; a header claiming the current
//! version with a truncated block is rejected before any accessor can read
//! past it.

use ironsbe_codegen_tests::versioned::{
    CollideDecoder, CollideReader, EvolvedDecoder, EvolvedEncoder, EvolvedReader, SCHEMA_ID,
    TaggedLegsDecoder, TaggedLegsReader,
};
use ironsbe_core::decoder::{DecodeError, SbeDecoder};
use ironsbe_core::header::MessageHeader;

const V1_BLOCK: u16 = 8;
const V2_BLOCK: u16 = 16;

fn header_for(template_id: u16, block_length: u16, version: u16) -> Vec<u8> {
    let mut h = vec![0u8; MessageHeader::ENCODED_LENGTH];
    h[0..2].copy_from_slice(&block_length.to_le_bytes());
    h[2..4].copy_from_slice(&template_id.to_le_bytes());
    h[4..6].copy_from_slice(&SCHEMA_ID.to_le_bytes());
    h[6..8].copy_from_slice(&version.to_le_bytes());
    h
}

fn header(block_length: u16, version: u16) -> Vec<u8> {
    header_for(EvolvedDecoder::TEMPLATE_ID, block_length, version)
}

#[test]
fn test_required_block_length_follows_since_version() {
    assert_eq!(EvolvedDecoder::required_block_length(0), V1_BLOCK);
    assert_eq!(EvolvedDecoder::required_block_length(1), V1_BLOCK);
    assert_eq!(EvolvedDecoder::required_block_length(2), V2_BLOCK);
    assert_eq!(EvolvedDecoder::required_block_length(9), V2_BLOCK);
}

#[test]
fn test_decode_older_version_reads_original_field_and_none_for_newer() {
    // A version-1 encoder wrote an 8-byte block: `ts` only, no group.
    let mut bytes = header(V1_BLOCK, 1);
    bytes.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0]); // legs: blockLength 0, numInGroup 0
    bytes.extend_from_slice(&2u16.to_le_bytes()); // memo length
    bytes.extend_from_slice(b"ok");

    let decoder = EvolvedDecoder::decode(&bytes).expect("older version decodes");
    assert_eq!(decoder.ts(), 0x1122_3344_5566_7788);
    assert_eq!(decoder.extra(), None);
    assert_eq!(decoder.acting_version(), 1);
    assert_eq!(decoder.acting_block_length(), V1_BLOCK);
    // The group header sits right after the 8-byte v1 block, not after the
    // 16-byte current block: walking it must not read past the buffer.
    assert!(decoder.legs().next().is_none());
    assert_eq!(decoder.memo(), b"ok");
    assert_eq!(decoder.encoded_length(), bytes.len());

    let mut reader = EvolvedReader::decode(&bytes).expect("older version reads");
    assert_eq!(reader.ts(), 0x1122_3344_5566_7788);
    assert_eq!(reader.extra(), None);
    assert!(reader.legs().next().is_none());
    assert_eq!(reader.memo(), b"ok");
}

#[test]
fn test_decode_truncated_block_claiming_current_version_is_rejected() {
    let bytes = header(0, 2);
    assert_eq!(
        EvolvedDecoder::decode(&bytes).err(),
        Some(DecodeError::BlockLengthTooSmall {
            required: V2_BLOCK,
            declared: 0,
            version: 2,
        })
    );

    let mut short = header(V1_BLOCK, 2);
    short.extend_from_slice(&[0u8; 8]);
    assert!(matches!(
        EvolvedDecoder::decode(&short),
        Err(DecodeError::BlockLengthTooSmall {
            required: V2_BLOCK,
            declared: V1_BLOCK,
            version: 2,
        })
    ));
}

#[test]
fn test_decode_truncated_block_claiming_older_version_is_rejected() {
    // Version 1 needs 8 bytes; a header declaring 0 is still malformed.
    let bytes = header(0, 1);
    assert!(matches!(
        EvolvedDecoder::decode(&bytes),
        Err(DecodeError::BlockLengthTooSmall {
            required: V1_BLOCK,
            declared: 0,
            version: 1,
        })
    ));
}

#[test]
fn test_current_version_roundtrip_reads_versioned_fields_as_some() {
    let mut buf = vec![0u8; 256];
    let len = {
        let mut enc = EvolvedEncoder::wrap(&mut buf, 0);
        enc.set_ts(7).set_extra(9);
        {
            let mut group = enc.legs_count(1);
            group
                .next_entry()
                .expect("one leg")
                .set_leg_id(42)
                .set_ratio(3);
        }
        enc.set_memo(b"hi");
        enc.encoded_length()
    };

    let decoder = EvolvedDecoder::decode(&buf[..len]).expect("current version decodes");
    assert_eq!(decoder.ts(), 7);
    assert_eq!(decoder.extra(), Some(9));
    let mut legs = decoder.legs();
    let leg = legs.next().expect("one leg");
    assert_eq!(leg.leg_id(), 42);
    assert_eq!(leg.ratio(), Some(3));
    assert!(legs.next().is_none());
    assert_eq!(decoder.memo(), b"hi");
}

#[test]
fn test_group_entry_versioned_field_is_none_when_entry_block_is_older() {
    // Current-version root block, but a group header declaring the v1 entry
    // size (8 bytes): `ratio` at offset 8..12 is outside the entry.
    let mut bytes = header(V2_BLOCK, 2);
    bytes.extend_from_slice(&[0u8; 16]);
    bytes.extend_from_slice(&8u16.to_le_bytes()); // entry blockLength (v1)
    bytes.extend_from_slice(&1u16.to_le_bytes()); // numInGroup
    bytes.extend_from_slice(&42u64.to_le_bytes()); // legId
    bytes.extend_from_slice(&0u16.to_le_bytes()); // memo length

    let decoder = EvolvedDecoder::decode(&bytes).expect("decodes");
    let mut legs = decoder.legs();
    let leg = legs.next().expect("one leg");
    assert_eq!(leg.leg_id(), 42);
    assert_eq!(leg.ratio(), None);
}

#[test]
fn test_group_entry_versioned_field_is_none_when_older_version_reserved_padding() {
    // A version-1 encoder wrote 12-byte entries: `legId` plus four bytes of
    // reserved padding (SBE 1.0 §5.4.3). Version 2 placed `ratio` in that
    // space. The bytes are inside the entry, but the field is absent because
    // the acting version predates it: presence is decided by version, not by
    // whether the bytes happen to fit.
    let mut bytes = header(V1_BLOCK, 1);
    bytes.extend_from_slice(&7u64.to_le_bytes());
    bytes.extend_from_slice(&12u16.to_le_bytes()); // entry blockLength (v1 with padding)
    bytes.extend_from_slice(&1u16.to_le_bytes()); // numInGroup
    bytes.extend_from_slice(&42u64.to_le_bytes()); // legId
    bytes.extend_from_slice(&[0, 0, 0, 0]); // padding
    bytes.extend_from_slice(&0u16.to_le_bytes()); // memo length

    let decoder = EvolvedDecoder::decode(&bytes).expect("decodes");
    let leg = decoder.legs().next().expect("one leg");
    assert_eq!(leg.leg_id(), 42);
    assert_eq!(leg.ratio(), None);

    let mut reader = EvolvedReader::decode(&bytes).expect("reads");
    let leg = reader.legs().next().expect("one leg");
    assert_eq!(leg.leg_id(), 42);
    assert_eq!(leg.ratio(), None);
}

#[test]
fn test_entry_reader_gates_versioned_field_next_to_entry_var_data() {
    // Root block (8) + legs: v1 entries of 8 bytes, each followed by a tag.
    let mut bytes = header_for(TaggedLegsDecoder::TEMPLATE_ID, 8, 1);
    bytes.extend_from_slice(&1u64.to_le_bytes()); // ts
    bytes.extend_from_slice(&8u16.to_le_bytes()); // entry blockLength (v1)
    bytes.extend_from_slice(&1u16.to_le_bytes()); // numInGroup
    bytes.extend_from_slice(&5u64.to_le_bytes()); // legId
    bytes.extend_from_slice(&2u16.to_le_bytes()); // tag length
    bytes.extend_from_slice(b"ab");

    let mut reader = TaggedLegsReader::decode(&bytes).expect("reads");
    let mut legs = reader.legs();
    let mut leg = legs.next_entry().expect("one leg");
    assert_eq!(leg.leg_id(), 5);
    assert_eq!(leg.ratio(), None);
    assert_eq!(leg.tag(), b"ab");
}

#[test]
fn test_schema_field_names_colliding_with_generated_methods_are_renamed() {
    let mut bytes = header_for(CollideDecoder::TEMPLATE_ID, 12, 2);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes()); // note length

    let decoder = CollideDecoder::decode(&bytes).expect("decodes");
    // Generator's own accessors keep their names...
    assert_eq!(decoder.acting_block_length(), 12);
    assert_eq!(decoder.acting_version(), 2);
    // ...and the schema fields get the trailing underscore.
    assert_eq!(decoder.acting_block_length_(), 1);
    assert_eq!(decoder.acting_version_(), 2);
    assert_eq!(decoder.wrap_acting_(), 3);

    let reader = CollideReader::decode(&bytes).expect("reads");
    assert_eq!(reader.acting_block_length_(), 1);
    assert_eq!(reader.wrap_acting_(), 3);
}
