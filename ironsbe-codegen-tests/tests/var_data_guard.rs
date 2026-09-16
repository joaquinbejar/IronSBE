//! Tests for the variable-part guard on generated encoders.
//!
//! Regression coverage for <https://github.com/joaquinbejar/IronSBE/issues/63>:
//! a `<data>` field or a nested group the caller never wrote used to leave
//! a hole in the frame, so the decoder read the next entry's fixed block as
//! a length header. Skipped parts now encode as empty (a group header with
//! zero entries, a zero-length var data header) and out-of-order writes
//! panic instead of corrupting the wire.

use ironsbe_codegen_tests::var_data::{
    ExampleDecoder, ExampleEncoder, FlatDecoder, FlatEncoder, GroupsOnlyDecoder, GroupsOnlyEncoder,
    NestedDecoder, NestedEncoder, OnlyDataDecoder, OnlyDataEncoder, QuoteDecoder, QuoteEncoder,
};
use ironsbe_core::decoder::SbeDecoder;
use ironsbe_core::header::{GroupHeader, MessageHeader};

/// Length header width of `varStringEncoding` (uint16).
const U16_HEADER: usize = 2;
/// Length header width of `varDataEncoding8` (uint8).
const U8_HEADER: usize = 1;
/// Length header width of `varDataEncoding32` (uint32).
const U32_HEADER: usize = 4;
/// Bytes per `Example.legs` entry (`legId` u64 + `ratio` u32).
const EXAMPLE_LEG_BLOCK_LENGTH: usize = 12;
/// Bytes per `Example.notes` entry (`code` u16).
const NOTE_BLOCK_LENGTH: usize = 2;
/// Bytes per `Quote.legs` entry (`legQty` u32).
const QUOTE_LEG_BLOCK_LENGTH: usize = 4;
/// Bytes per `Nested.orders` entry (`orderId` u64).
const ORDER_BLOCK_LENGTH: usize = 8;
/// Bytes per `Nested.fills` entry (`fillId` u64).
const FILL_BLOCK_LENGTH: usize = 8;

/// Owned form of one decoded `fills` entry.
type DecodedFill = (u64, Vec<u8>);
/// Owned form of one decoded `orders` entry.
type DecodedOrder = (u64, Vec<DecodedFill>, Vec<u8>);

/// Offset of the first byte after the message header and root block.
const fn block_end(block_length: u16) -> usize {
    MessageHeader::ENCODED_LENGTH + block_length as usize
}

/// Reads the group header at `pos` as `(blockLength, numInGroup)`.
fn group_header(buf: &[u8], pos: usize) -> (u16, u16) {
    let header = GroupHeader::wrap(buf, pos);
    ({ header.block_length }, { header.num_in_group })
}

// ---------------------------------------------------------------------
// Message level
// ---------------------------------------------------------------------

#[test]
fn test_finish_with_every_part_skipped_encodes_empty_groups_and_var_data() {
    let mut buf = [0xAAu8; 64];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0);
    encoder.set_qty(7);
    let len = encoder.finish();

    let legs_pos = block_end(ExampleEncoder::BLOCK_LENGTH);
    let notes_pos = legs_pos + GroupHeader::ENCODED_LENGTH;
    let label_pos = notes_pos + GroupHeader::ENCODED_LENGTH;
    let payload_pos = label_pos + U16_HEADER;
    assert_eq!(len, payload_pos + U8_HEADER);

    assert_eq!(
        group_header(&buf, legs_pos),
        (EXAMPLE_LEG_BLOCK_LENGTH as u16, 0)
    );
    assert_eq!(group_header(&buf, notes_pos), (NOTE_BLOCK_LENGTH as u16, 0));
    assert_eq!(&buf[label_pos..payload_pos], &0u16.to_le_bytes());
    assert_eq!(buf[payload_pos], 0);
    assert_eq!(buf[len], 0xAA, "nothing written past the frame");

    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert_eq!(decoder.qty(), 7);
    assert!(decoder.legs().is_empty());
    assert!(decoder.notes().is_empty());
    assert_eq!(decoder.label(), b"");
    assert_eq!(decoder.payload(), b"");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_finish_fills_trailing_var_data_only() {
    let mut buf = [0u8; 64];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0);
    encoder.set_qty(1);
    encoder
        .legs_count(1)
        .next_entry()
        .expect("leg")
        .set_leg_id(5)
        .set_ratio(2);
    encoder.notes_count(0);
    encoder.set_label(b"lbl");
    let len = encoder.finish();

    assert_eq!(
        len,
        block_end(ExampleEncoder::BLOCK_LENGTH)
            + GroupHeader::ENCODED_LENGTH
            + EXAMPLE_LEG_BLOCK_LENGTH
            + GroupHeader::ENCODED_LENGTH
            + U16_HEADER
            + 3
            + U8_HEADER
    );
    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert_eq!(decoder.label(), b"lbl");
    assert_eq!(decoder.payload(), b"");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_var_data_after_skipped_groups_writes_empty_groups_first() {
    let mut buf = [0u8; 64];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0);
    encoder.set_qty(1);
    encoder.set_label(b"x").set_payload(&[9]);
    let len = encoder.finish();

    let legs_pos = block_end(ExampleEncoder::BLOCK_LENGTH);
    let notes_pos = legs_pos + GroupHeader::ENCODED_LENGTH;
    let label_pos = notes_pos + GroupHeader::ENCODED_LENGTH;
    assert_eq!(group_header(&buf, legs_pos).1, 0);
    assert_eq!(group_header(&buf, notes_pos).1, 0);
    assert_eq!(&buf[label_pos..label_pos + U16_HEADER], &1u16.to_le_bytes());
    assert_eq!(buf[label_pos + U16_HEADER], b'x');

    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert!(decoder.legs().is_empty());
    assert!(decoder.notes().is_empty());
    assert_eq!(decoder.label(), b"x");
    assert_eq!(decoder.payload(), &[9]);
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_group_after_skipped_group_writes_empty_group_first() {
    let mut buf = [0u8; 64];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0);
    encoder.set_qty(1);
    encoder
        .notes_count(1)
        .next_entry()
        .expect("note")
        .set_code(0x1234);
    let len = encoder.finish();

    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    assert!(decoder.legs().is_empty());
    let codes: Vec<u16> = decoder.notes().map(|entry| entry.code()).collect();
    assert_eq!(codes, vec![0x1234]);
    assert_eq!(decoder.label(), b"");
    assert_eq!(decoder.payload(), b"");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_only_data_message_skipped_blob_finishes_with_zero_length_header() {
    let mut buf = [0u8; 16];
    let encoder = OnlyDataEncoder::wrap(&mut buf, 0);
    let len = encoder.finish();

    assert_eq!(len, MessageHeader::ENCODED_LENGTH + U32_HEADER);
    let decoder = OnlyDataDecoder::decode(&buf[..len]).expect("decode OnlyData");
    assert_eq!(decoder.blob(), b"");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_groups_only_message_skipped_group_finishes_with_empty_header() {
    let mut buf = [0u8; 32];
    let mut encoder = GroupsOnlyEncoder::wrap(&mut buf, 0);
    encoder.set_request_id(3);
    let len = encoder.finish();

    assert_eq!(
        len,
        block_end(GroupsOnlyEncoder::BLOCK_LENGTH) + GroupHeader::ENCODED_LENGTH
    );
    let decoder = GroupsOnlyDecoder::decode(&buf[..len]).expect("decode GroupsOnly");
    assert_eq!(decoder.request_id(), 3);
    assert!(decoder.items().is_empty());
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_flat_message_finish_equals_encoded_length() {
    let mut buf = [0u8; 32];
    let mut encoder = FlatEncoder::wrap(&mut buf, 0);
    encoder.set_ts(42);
    let before = encoder.encoded_length();
    let len = encoder.finish();

    assert_eq!(len, before);
    assert_eq!(len, block_end(FlatEncoder::BLOCK_LENGTH));
    let decoder = FlatDecoder::decode(&buf[..len]).expect("decode Flat");
    assert_eq!(decoder.ts(), 42);
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_all_parts_written_output_is_byte_identical_and_finish_adds_nothing() {
    let mut buf = [0u8; 64];
    let mut encoder = QuoteEncoder::wrap(&mut buf, 0);
    encoder.set_request_id(7);
    encoder
        .legs_count(1)
        .next_entry()
        .expect("leg")
        .set_leg_qty(10)
        .set_leg_tag(b"ab");
    encoder.set_comment(b"");
    let before = encoder.encoded_length();
    let len = encoder.finish();

    // same bytes as the 0.6.0 wire-layout test in group_var_data_roundtrip.rs
    let expected: Vec<u8> = [
        &[0x04, 0x00, 0x04, 0x00, 0x2A, 0x00, 0x01, 0x00][..],
        &[0x07, 0x00, 0x00, 0x00],
        &[0x04, 0x00, 0x01, 0x00],
        &[0x0A, 0x00, 0x00, 0x00],
        &[0x02, 0x00, b'a', b'b'],
        &[0x00, 0x00],
    ]
    .concat();
    assert_eq!(
        len, before,
        "finish() must not add bytes once every part is written"
    );
    assert_eq!(&buf[..len], expected.as_slice());
}

// ---------------------------------------------------------------------
// Inside group entries
// ---------------------------------------------------------------------

#[test]
fn test_entry_skipped_var_data_does_not_corrupt_following_entries() {
    let mut buf = [0u8; 128];
    let mut encoder = QuoteEncoder::wrap(&mut buf, 0);
    encoder.set_request_id(1);
    {
        let mut group = encoder.legs_count(3);
        group.next_entry().expect("leg 0").set_leg_qty(1);
        group
            .next_entry()
            .expect("leg 1")
            .set_leg_qty(2)
            .set_leg_tag(b"two");
        group.next_entry().expect("leg 2").set_leg_qty(3);
        assert!(group.next_entry().is_none());
    }
    encoder.set_comment(b"c");
    let len = encoder.finish();

    assert_eq!(
        len,
        block_end(QuoteEncoder::BLOCK_LENGTH)
            + GroupHeader::ENCODED_LENGTH
            + (QUOTE_LEG_BLOCK_LENGTH + U16_HEADER)
            + (QUOTE_LEG_BLOCK_LENGTH + U16_HEADER + 3)
            + (QUOTE_LEG_BLOCK_LENGTH + U16_HEADER)
            + U16_HEADER
            + 1
    );

    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");
    let legs: Vec<(u32, Vec<u8>)> = decoder
        .legs()
        .map(|leg| (leg.leg_qty(), leg.leg_tag().to_vec()))
        .collect();
    assert_eq!(
        legs,
        vec![(1, Vec::new()), (2, b"two".to_vec()), (3, Vec::new())]
    );
    assert_eq!(decoder.comment(), b"c");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_entry_skipped_nested_group_writes_empty_group_header() {
    let mut buf = [0u8; 128];
    let mut encoder = NestedEncoder::wrap(&mut buf, 0);
    {
        let mut group = encoder.orders_count(1);
        let mut order = group.next_entry().expect("order");
        order.set_order_id(9);
        order.set_memo(b"m");
    }
    let len = encoder.finish();

    let fills_pos =
        block_end(NestedEncoder::BLOCK_LENGTH) + GroupHeader::ENCODED_LENGTH + ORDER_BLOCK_LENGTH;
    assert_eq!(group_header(&buf, fills_pos), (FILL_BLOCK_LENGTH as u16, 0));

    let decoder = NestedDecoder::decode(&buf[..len]).expect("decode Nested");
    let order = decoder.orders().next().expect("order");
    assert_eq!(order.order_id(), 9);
    assert!(order.fills().is_empty());
    assert_eq!(order.memo(), b"m");
    assert!(
        decoder.flags().is_empty(),
        "skipped flags group encoded empty"
    );
    assert_eq!(decoder.trailer(), b"", "skipped trailer encoded empty");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_entry_dropped_immediately_encodes_every_part_empty() {
    let mut buf = [0u8; 128];
    let mut encoder = NestedEncoder::wrap(&mut buf, 0);
    {
        let mut group = encoder.orders_count(2);
        group.next_entry();
        group.next_entry();
        assert!(group.next_entry().is_none());
        assert_eq!(
            group.encoded_length(),
            GroupHeader::ENCODED_LENGTH
                + 2 * (ORDER_BLOCK_LENGTH + GroupHeader::ENCODED_LENGTH + U16_HEADER)
        );
    }
    let len = encoder.finish();

    let decoder = NestedDecoder::decode(&buf[..len]).expect("decode Nested");
    let orders: Vec<(u64, usize, Vec<u8>)> = decoder
        .orders()
        .map(|order| {
            (
                order.order_id(),
                order.fills().count(),
                order.memo().to_vec(),
            )
        })
        .collect();
    assert_eq!(orders, vec![(0, 0, Vec::new()), (0, 0, Vec::new())]);
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_inner_entry_skipped_var_data_keeps_outer_entries_aligned() {
    let mut buf = [0u8; 256];
    let mut encoder = NestedEncoder::wrap(&mut buf, 0);
    {
        let mut orders = encoder.orders_count(2);
        {
            let mut order = orders.next_entry().expect("order 0");
            order.set_order_id(1);
            {
                let mut fills = order.fills_count(2);
                fills
                    .next_entry()
                    .expect("fill 0")
                    .set_fill_id(11)
                    .set_note(&[1, 2]);
                fills.next_entry().expect("fill 1").set_fill_id(12);
            }
            order.set_memo(b"first");
        }
        {
            let mut order = orders.next_entry().expect("order 1");
            order.set_order_id(2);
            order
                .fills_count(1)
                .next_entry()
                .expect("fill")
                .set_fill_id(21)
                .set_note(&[3]);
            order.set_memo(b"second");
        }
    }
    encoder
        .flags_count(1)
        .next_entry()
        .expect("flag")
        .set_flag(0xFF);
    encoder.set_trailer(b"t");
    let len = encoder.finish();

    let decoder = NestedDecoder::decode(&buf[..len]).expect("decode Nested");
    let orders: Vec<DecodedOrder> = decoder
        .orders()
        .map(|order| {
            let fills = order
                .fills()
                .map(|fill| (fill.fill_id(), fill.note().to_vec()))
                .collect();
            (order.order_id(), fills, order.memo().to_vec())
        })
        .collect();
    assert_eq!(
        orders,
        vec![
            (
                1,
                vec![(11, vec![1, 2]), (12, Vec::new())],
                b"first".to_vec()
            ),
            (2, vec![(21, vec![3])], b"second".to_vec()),
        ]
    );
    let flags: Vec<u8> = decoder.flags().map(|entry| entry.flag()).collect();
    assert_eq!(flags, vec![0xFF]);
    assert_eq!(decoder.trailer(), b"t");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_entry_drop_while_panicking_writes_nothing() {
    let mut buf = [0xAAu8; 64];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut encoder = QuoteEncoder::wrap(&mut buf, 0);
        let mut group = encoder.legs_count(1);
        let _entry = group.next_entry().expect("leg");
        panic!("caller panics while an entry encoder is alive");
    }));
    assert!(result.is_err());

    // the legTag header slot right after the entry's fixed block is untouched
    let tag_pos = block_end(QuoteEncoder::BLOCK_LENGTH)
        + GroupHeader::ENCODED_LENGTH
        + QUOTE_LEG_BLOCK_LENGTH;
    assert_eq!(&buf[tag_pos..tag_pos + U16_HEADER], &[0xAA, 0xAA]);
}

// ---------------------------------------------------------------------
// Out-of-order writes
// ---------------------------------------------------------------------

#[test]
#[should_panic(expected = "var data field 'label' written out of schema order")]
fn test_message_var_data_written_out_of_order_panics() {
    let mut buf = [0u8; 64];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0);
    encoder.set_payload(&[1]).set_label(b"late");
}

#[test]
#[should_panic(expected = "var data field 'label' written out of schema order")]
fn test_message_var_data_written_twice_panics() {
    let mut buf = [0u8; 64];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0);
    encoder.set_label(b"once").set_label(b"twice");
}

#[test]
#[should_panic(expected = "repeating group 'legs' written out of schema order")]
fn test_group_begun_after_var_data_panics() {
    let mut buf = [0u8; 64];
    let mut encoder = QuoteEncoder::wrap(&mut buf, 0);
    encoder.set_comment(b"c");
    encoder.legs_count(0);
}

#[test]
#[should_panic(expected = "repeating group 'legs' written out of schema order")]
fn test_group_begun_twice_panics() {
    let mut buf = [0u8; 64];
    let mut encoder = ExampleEncoder::wrap(&mut buf, 0);
    encoder.legs_count(0);
    encoder.legs_count(0);
}

#[test]
#[should_panic(expected = "var data field 'legTag' written out of schema order")]
fn test_entry_var_data_written_twice_panics() {
    let mut buf = [0u8; 64];
    let mut encoder = QuoteEncoder::wrap(&mut buf, 0);
    let mut group = encoder.legs_count(1);
    let mut entry = group.next_entry().expect("leg");
    entry.set_leg_tag(b"a").set_leg_tag(b"b");
}

#[test]
#[should_panic(expected = "repeating group 'fills' written out of schema order")]
fn test_entry_nested_group_after_var_data_panics() {
    let mut buf = [0u8; 64];
    let mut encoder = NestedEncoder::wrap(&mut buf, 0);
    let mut group = encoder.orders_count(1);
    let mut order = group.next_entry().expect("order");
    order.set_memo(b"m");
    order.fills_count(0);
}
