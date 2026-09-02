//! Round-trip tests for generated `<data>` (variable-length) codecs.
//!
//! Regression coverage for <https://github.com/joaquinbejar/IronSBE/issues/59>:
//! the generator used to omit var data accessors entirely, and placed the
//! second repeating group right after the first group's header instead of
//! after its entries.

use ironsbe_codegen_tests::var_data::{
    ExampleDecoder, ExampleEncoder, GroupsOnlyDecoder, GroupsOnlyEncoder, OnlyDataDecoder,
    OnlyDataEncoder, SCHEMA_ID, SCHEMA_VERSION,
};
use ironsbe_core::decoder::SbeDecoder;
use ironsbe_core::header::{GroupHeader, MessageHeader};

/// Bytes per `legs` entry (`legId` u64 + `ratio` u32).
const LEG_BLOCK_LENGTH: usize = 12;
/// Bytes per `notes` entry (`code` u16).
const NOTE_BLOCK_LENGTH: usize = 2;
/// Length header width of `varStringEncoding` (uint16).
const U16_HEADER: usize = 2;
/// Length header width of `varDataEncoding8` (uint8).
const U8_HEADER: usize = 1;
/// Length header width of `varDataEncoding32` (uint32).
const U32_HEADER: usize = 4;

/// Encodes an `Example` message and returns its encoded length in bytes.
fn encode_example(
    buf: &mut [u8],
    qty: u64,
    legs: &[(u64, u32)],
    notes: &[u16],
    label: &[u8],
    payload: &[u8],
) -> usize {
    let mut encoder = ExampleEncoder::wrap(buf, 0);
    encoder.set_qty(qty);
    {
        let mut group = encoder.legs_count(legs.len() as u16);
        for (leg_id, ratio) in legs {
            let mut entry = group.next_entry().expect("leg entry");
            entry.set_leg_id(*leg_id).set_ratio(*ratio);
        }
        assert!(group.next_entry().is_none(), "legs group over-iterated");
    }
    {
        let mut group = encoder.notes_count(notes.len() as u16);
        for code in notes {
            group.next_entry().expect("note entry").set_code(*code);
        }
        assert!(group.next_entry().is_none(), "notes group over-iterated");
    }
    encoder.set_label(label).set_payload(payload);
    encoder.encoded_length()
}

/// Expected wire size of an `Example` message.
fn expected_example_length(legs: usize, notes: usize, label: usize, payload: usize) -> usize {
    MessageHeader::ENCODED_LENGTH
        + ExampleEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH
        + legs * LEG_BLOCK_LENGTH
        + GroupHeader::ENCODED_LENGTH
        + notes * NOTE_BLOCK_LENGTH
        + U16_HEADER
        + label
        + U8_HEADER
        + payload
}

#[test]
fn test_roundtrip_message_with_groups_and_var_data_matches() {
    let mut buf = [0u8; 256];
    let legs = [(1u64, 10u32), (2, 20)];
    let len = encode_example(&mut buf, 7, &legs, &[], b"hello", &[1, 2, 3]);

    assert_eq!(len, expected_example_length(2, 0, 5, 3));

    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert_eq!(decoder.qty(), 7);

    let decoded_legs: Vec<(u64, u32)> = decoder
        .legs()
        .map(|entry| (entry.leg_id(), entry.ratio()))
        .collect();
    assert_eq!(decoded_legs, legs);
    assert_eq!(decoder.notes().count(), 0);

    assert_eq!(decoder.label(), b"hello");
    assert_eq!(decoder.label_as_str(), "hello");
    assert_eq!(decoder.payload(), &[1, 2, 3]);

    assert_eq!(
        decoder.encoded_length(),
        len,
        "decoder must report header + block + groups + var data"
    );
}

#[test]
fn test_wire_layout_var_data_follows_last_group() {
    let mut buf = [0u8; 256];
    let len = encode_example(&mut buf, 1, &[(1, 10), (2, 20)], &[0xBEEF], b"hello", &[9]);

    let block_end = MessageHeader::ENCODED_LENGTH + ExampleEncoder::BLOCK_LENGTH as usize;

    // legs header right after the fixed block
    let legs_header = GroupHeader::wrap(&buf[..], block_end);
    assert_eq!({ legs_header.block_length } as usize, LEG_BLOCK_LENGTH);
    assert_eq!({ legs_header.num_in_group }, 2);

    // notes header after the two legs entries, not after the legs header
    let notes_pos = block_end + GroupHeader::ENCODED_LENGTH + 2 * LEG_BLOCK_LENGTH;
    let notes_header = GroupHeader::wrap(&buf[..], notes_pos);
    assert_eq!({ notes_header.block_length } as usize, NOTE_BLOCK_LENGTH);
    assert_eq!({ notes_header.num_in_group }, 1);

    // label right after the notes entry
    let label_pos = notes_pos + GroupHeader::ENCODED_LENGTH + NOTE_BLOCK_LENGTH;
    assert_eq!(&buf[label_pos..label_pos + U16_HEADER], &5u16.to_le_bytes());
    assert_eq!(
        &buf[label_pos + U16_HEADER..label_pos + U16_HEADER + 5],
        b"hello"
    );

    // payload right after the label
    let payload_pos = label_pos + U16_HEADER + 5;
    assert_eq!(buf[payload_pos], 1);
    assert_eq!(buf[payload_pos + U8_HEADER], 9);
    assert_eq!(payload_pos + U8_HEADER + 1, len);

    // the decoder reads the second group from the same place
    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    let codes: Vec<u16> = decoder.notes().map(|entry| entry.code()).collect();
    assert_eq!(codes, vec![0xBEEF]);
    assert_eq!(decoder.payload(), &[9]);
}

#[test]
fn test_roundtrip_empty_var_data_returns_empty_slice() {
    let mut buf = [0u8; 64];
    let len = encode_example(&mut buf, 3, &[], &[], b"", b"");

    assert_eq!(len, expected_example_length(0, 0, 0, 0));

    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert!(decoder.label().is_empty());
    assert_eq!(decoder.label_as_str(), "");
    assert!(decoder.payload().is_empty());
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_roundtrip_message_without_groups_only_var_data() {
    let blob: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let mut buf = vec![0u8; 2048];

    let len = {
        let mut encoder = OnlyDataEncoder::wrap(&mut buf, 0);
        encoder.set_blob(&blob);
        encoder.encoded_length()
    };
    assert_eq!(len, MessageHeader::ENCODED_LENGTH + U32_HEADER + blob.len());

    let decoder = OnlyDataDecoder::decode(&buf[..len]).expect("decode OnlyData");
    assert_eq!(decoder.blob(), blob.as_slice());
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_roundtrip_groups_only_encoded_length_includes_entries() {
    let mut buf = [0u8; 128];

    let len = {
        let mut encoder = GroupsOnlyEncoder::wrap(&mut buf, 0);
        encoder.set_request_id(99);
        {
            let mut group = encoder.items_count(3);
            for item_id in [10u64, 20, 30] {
                group.next_entry().expect("item entry").set_item_id(item_id);
            }
        }
        encoder.encoded_length()
    };
    assert_eq!(
        len,
        MessageHeader::ENCODED_LENGTH + 4 + GroupHeader::ENCODED_LENGTH + 3 * 8
    );

    let decoder = GroupsOnlyDecoder::decode(&buf[..len]).expect("decode GroupsOnly");
    assert_eq!(decoder.request_id(), 99);
    let items: Vec<u64> = decoder.items().map(|entry| entry.item_id()).collect();
    assert_eq!(items, vec![10, 20, 30]);
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
#[should_panic(expected = "exceeds u8::MAX")]
fn test_encoder_var_data_u8_header_over_255_bytes_panics() {
    let mut buf = [0u8; 512];
    let too_long = [0u8; 256];
    encode_example(&mut buf, 1, &[], &[], b"", &too_long);
}

#[test]
fn test_decoder_invalid_utf8_as_str_returns_empty() {
    let mut buf = [0u8; 64];
    let len = encode_example(&mut buf, 1, &[], &[], &[0xFF, 0xFE], b"");

    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert_eq!(decoder.label(), &[0xFF, 0xFE]);
    assert_eq!(decoder.label_as_str(), "");
}

#[test]
fn test_generated_constants_match_schema() {
    assert_eq!(SCHEMA_ID, 42);
    assert_eq!(SCHEMA_VERSION, 1);
    assert_eq!(ExampleDecoder::TEMPLATE_ID, 1);
    assert_eq!(OnlyDataDecoder::TEMPLATE_ID, 2);
    assert_eq!(GroupsOnlyDecoder::TEMPLATE_ID, 3);
}
