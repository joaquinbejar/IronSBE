//! Tests for the fallible `try_set_<field>` var data setters (issue #70).
//!
//! Exercises the codecs generated with
//! `GeneratorOptions::with_panicking_setters(false)`, where `try_set_*` is
//! the only var data setter: round trips for `uint8`, `uint16` and `uint32`
//! length headers at message and entry level, and every error case, each
//! checking that nothing was written.

use ironsbe_codegen_tests::var_data_no_panic::{
    ExampleDecoder, ExampleEncoder, OnlyDataDecoder, OnlyDataEncoder, QuoteDecoder, QuoteEncoder,
};
use ironsbe_core::decoder::SbeDecoder;
use ironsbe_core::error::EncodeError;
use ironsbe_core::header::{GroupHeader, MessageHeader};

/// Length header width of `varStringEncoding` (uint16).
const U16_HEADER: usize = 2;
/// Length header width of `varDataEncoding8` (uint8).
const U8_HEADER: usize = 1;
/// Length header width of `varDataEncoding32` (uint32).
const U32_HEADER: usize = 4;

/// Wire size of an `Example` with empty groups and the given var data.
fn example_length(label: usize, payload: usize) -> usize {
    MessageHeader::ENCODED_LENGTH
        + ExampleEncoder::BLOCK_LENGTH as usize
        + 2 * GroupHeader::ENCODED_LENGTH
        + U16_HEADER
        + label
        + U8_HEADER
        + payload
}

#[test]
fn test_try_set_roundtrip_u16_and_u8_headers_matches() {
    let mut buf = [0u8; 128];
    let len = {
        let mut encoder = ExampleEncoder::wrap(&mut buf, 0).expect("wrap");
        encoder.set_qty(7);
        // Both groups are skipped: try_set fills them with empty headers.
        encoder
            .try_set_label(b"hello")
            .expect("label fits")
            .try_set_payload(&[1, 2, 3])
            .expect("payload fits");
        encoder.encoded_length()
    };
    assert_eq!(len, example_length(5, 3));

    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert_eq!(decoder.qty(), 7);
    assert_eq!(decoder.legs().expect("legs").count(), 0);
    assert_eq!(decoder.notes().expect("notes").count(), 0);
    assert_eq!(decoder.label().expect("label"), b"hello");
    assert_eq!(decoder.payload().expect("payload"), &[1, 2, 3]);
}

#[test]
fn test_try_set_roundtrip_u32_header_matches() {
    let blob: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let mut buf = vec![0u8; 2048];
    let len = {
        let mut encoder = OnlyDataEncoder::wrap(&mut buf, 0).expect("wrap");
        encoder.try_set_blob(&blob).expect("blob fits");
        encoder.encoded_length()
    };
    assert_eq!(len, MessageHeader::ENCODED_LENGTH + U32_HEADER + blob.len());

    let decoder = OnlyDataDecoder::decode(&buf[..len]).expect("decode OnlyData");
    assert_eq!(decoder.blob().expect("blob"), blob.as_slice());
}

#[test]
fn test_try_set_roundtrip_entry_var_data_matches() {
    let mut buf = [0u8; 128];
    let len = {
        let mut encoder = QuoteEncoder::wrap(&mut buf, 0).expect("wrap");
        encoder.set_request_id(42);
        {
            let mut group = encoder.legs_count(2).expect("group");
            for (qty, tag) in [(10u32, b"ab".as_slice()), (20, b"xyz")] {
                let mut entry = group.next_entry().expect("entry fits").expect("leg entry");
                entry.set_leg_qty(qty);
                entry.try_set_leg_tag(tag).expect("leg tag fits");
            }
        }
        encoder.try_set_comment(b"ok").expect("comment fits");
        encoder.encoded_length()
    };

    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");
    assert_eq!(decoder.request_id(), 42);
    let legs: Vec<(u32, Vec<u8>)> = decoder
        .legs()
        .expect("legs")
        .map(|e| e.expect("entry"))
        .map(|entry| (entry.leg_qty(), entry.leg_tag().expect("leg_tag").to_vec()))
        .collect();
    assert_eq!(legs, vec![(10, b"ab".to_vec()), (20, b"xyz".to_vec())]);
    assert_eq!(decoder.comment().expect("comment"), b"ok");
}

#[test]
fn test_try_set_length_overflow_returns_err() {
    let mut buf = [0u8; 512];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0).expect("wrap");
    let before = encoder.encoded_length();

    let too_long = [0u8; 256];
    let err = encoder
        .try_set_payload(&too_long)
        .map(|_| ())
        .expect_err("256 bytes overflow a uint8 length header");

    assert_eq!(
        err,
        EncodeError::LengthOverflow {
            field: "var data field 'payload'",
            length: 256,
            max: 255,
        }
    );
    assert_eq!(encoder.encoded_length(), before, "cursor unchanged");
    assert!(buf[before..].iter().all(|&b| b == 0), "nothing written");
}

#[test]
fn test_try_set_out_of_order_returns_err() {
    let mut buf = [0u8; 128];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0).expect("wrap");
    encoder.try_set_payload(b"p").expect("payload fits");
    let before = encoder.encoded_length();

    let err = encoder
        .try_set_label(b"late")
        .map(|_| ())
        .expect_err("label comes before payload");

    assert_eq!(
        err,
        EncodeError::OutOfOrder {
            field: "var data field 'label'",
            written: 4,
        }
    );
    assert_eq!(encoder.encoded_length(), before, "cursor unchanged");
    assert!(buf[before..].iter().all(|&b| b == 0), "nothing written");
}

#[test]
fn test_try_set_buffer_too_short_returns_err() {
    // Header + block (16) + two empty group headers (8) + label header (2)
    // leaves 4 bytes for the label in a 30-byte buffer. A 5-byte label only
    // fails if the skipped groups' headers are counted.
    let mut buf = [0u8; 30];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0).expect("wrap");
    let before = encoder.encoded_length();

    let err = encoder
        .try_set_label(b"hello")
        .map(|_| ())
        .expect_err("label does not fit");

    assert_eq!(
        err,
        EncodeError::BufferTooShort {
            field: "var data field 'label'",
            required: 31,
            available: 30,
        }
    );
    assert_eq!(encoder.encoded_length(), before, "cursor unchanged");
    assert!(buf[before..].iter().all(|&b| b == 0), "nothing written");
}

#[test]
fn test_try_set_exact_fit_succeeds() {
    let mut buf = [0u8; 30];
    let len = {
        let mut encoder = ExampleEncoder::wrap(&mut buf, 0).expect("wrap");
        encoder.try_set_label(b"four").expect("label fits exactly");
        encoder.encoded_length()
    };
    assert_eq!(len, 30);
}

/// Encodes a `SetterCollision` message with the generator that emits
/// `set_*`: the group accessors keep their names and the colliding
/// `try_set_*` var data setters get a trailing underscore.
fn encode_setter_collision(buf: &mut [u8]) -> usize {
    use ironsbe_codegen_tests::var_data::SetterCollisionEncoder;

    let mut encoder = SetterCollisionEncoder::wrap(buf, 0).expect("wrap");
    {
        let mut orders = encoder.try_set_order_count(1).expect("orders group");
        let mut entry = orders
            .next_entry()
            .expect("entry fits")
            .expect("order entry");
        entry.set_order_id(5);
        {
            let mut fills = entry.try_set_fill_count(1).expect("fills group");
            fills
                .next_entry()
                .expect("entry fits")
                .expect("fill entry")
                .set_fill_id(6);
        }
        entry.try_set_fill_count_(b"f").expect("fill note fits");
    }
    encoder.try_set_order_count_(b"o").expect("order note fits");
    encoder.encoded_length()
}

#[test]
fn test_try_set_colliding_with_group_accessor_keeps_both() {
    use ironsbe_codegen_tests::var_data::SetterCollisionDecoder;

    let mut buf = [0u8; 128];
    let len = encode_setter_collision(&mut buf);

    let decoder = SetterCollisionDecoder::decode(&buf[..len]).expect("decode SetterCollision");
    let orders: Vec<(u64, Vec<u64>, Vec<u8>)> = decoder
        .try_set_order()
        .expect("orders")
        .map(|entry| {
            let entry = entry.expect("order");
            let fills = entry
                .try_set_fill()
                .expect("fills")
                .map(|fill| fill.expect("fill").fill_id())
                .collect();
            (
                entry.order_id(),
                fills,
                entry.fill_count().expect("fill_count").to_vec(),
            )
        })
        .collect();
    assert_eq!(orders, vec![(5, vec![6], b"f".to_vec())]);
    assert_eq!(decoder.order_count().expect("order_count"), b"o");
}

#[test]
fn test_try_set_colliding_without_panicking_setters_compiles() {
    use ironsbe_codegen_tests::var_data_no_panic::SetterCollisionEncoder;

    let mut buf = [0u8; 128];
    let mut encoder = SetterCollisionEncoder::wrap(&mut buf, 0).expect("wrap");
    let _orders = encoder.try_set_order_count(0).expect("orders group");
    encoder.try_set_order_count_(b"o").expect("order note fits");
    assert!(encoder.encoded_length() > 0);
}
