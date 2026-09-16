//! Tests for the generated sequential readers.
//!
//! Coverage for <https://github.com/joaquinbejar/IronSBE/issues/64>: the
//! `<Message>Reader` decodes a message front to back with one cursor and
//! must agree byte for byte with the random-access `<Message>Decoder`,
//! whatever subset of the message the caller actually reads.

use ironsbe_codegen_tests::var_data::{
    ExampleDecoder, ExampleEncoder, ExampleReader, NestedDecoder, NestedEncoder, NestedReader,
    OnlyDataEncoder, OnlyDataReader, QuoteDecoder, QuoteEncoder, QuoteReader, SCHEMA_VERSION,
};
use ironsbe_core::decoder::{DecodeError, SbeDecoder};
use ironsbe_core::header::{GroupHeader, MessageHeader};

/// One `Quote.legs` entry: fixed `legQty` followed by the `legTag` var string.
type Leg<'a> = (u32, &'a [u8]);
/// One `Nested.fills` entry: fixed `fillId` followed by the `note` var data.
type Fill<'a> = (u64, &'a [u8]);
/// One `Nested.orders` entry: fixed `orderId`, nested `fills`, then `memo`.
type Order<'a> = (u64, &'a [Fill<'a>], &'a [u8]);
/// Owned form of one decoded `fills` entry.
type DecodedFill = (u64, Vec<u8>);
/// Owned form of one decoded `orders` entry.
type DecodedOrder = (u64, Vec<DecodedFill>, Vec<u8>);

const QUOTE_LEGS: [Leg<'_>; 3] = [(1, b""), (2, b"xyz"), (3, b"0123456789")];
const FILLS_A: [Fill<'_>; 2] = [(11, &[1]), (12, &[2, 3])];
const FILLS_B: [Fill<'_>; 0] = [];
const ORDERS: [Order<'_>; 2] = [(1, &FILLS_A, b"m1"), (2, &FILLS_B, b"")];
const FLAGS: [u8; 3] = [0x0A, 0x0B, 0x0C];

/// Encodes a `Quote` message and returns its encoded length in bytes.
fn encode_quote(buf: &mut [u8], legs: &[Leg<'_>], comment: &[u8]) -> usize {
    let mut encoder = QuoteEncoder::wrap(buf, 0);
    encoder.set_request_id(7);
    {
        let mut group = encoder.legs_count(legs.len() as u16);
        for (qty, tag) in legs {
            group
                .next_entry()
                .expect("leg entry")
                .set_leg_qty(*qty)
                .set_leg_tag(tag);
        }
    }
    encoder.set_comment(comment);
    encoder.finish()
}

/// Encodes a `Nested` message and returns its encoded length in bytes.
fn encode_nested(buf: &mut [u8], orders: &[Order<'_>], flags: &[u8], trailer: &[u8]) -> usize {
    let mut encoder = NestedEncoder::wrap(buf, 0);
    {
        let mut group = encoder.orders_count(orders.len() as u16);
        for (order_id, fills, memo) in orders {
            let mut order = group.next_entry().expect("order entry");
            order.set_order_id(*order_id);
            {
                let mut fills_group = order.fills_count(fills.len() as u16);
                for (fill_id, note) in fills.iter() {
                    fills_group
                        .next_entry()
                        .expect("fill entry")
                        .set_fill_id(*fill_id)
                        .set_note(note);
                }
            }
            order.set_memo(memo);
        }
    }
    {
        let mut group = encoder.flags_count(flags.len() as u16);
        for flag in flags {
            group.next_entry().expect("flag entry").set_flag(*flag);
        }
    }
    encoder.set_trailer(trailer);
    encoder.finish()
}

/// Encodes an `Example` message and returns its encoded length in bytes.
fn encode_example(buf: &mut [u8], legs: &[(u64, u32)], notes: &[u16]) -> usize {
    let mut encoder = ExampleEncoder::wrap(buf, 0);
    encoder.set_qty(99);
    {
        let mut group = encoder.legs_count(legs.len() as u16);
        for (leg_id, ratio) in legs {
            group
                .next_entry()
                .expect("leg entry")
                .set_leg_id(*leg_id)
                .set_ratio(*ratio);
        }
    }
    {
        let mut group = encoder.notes_count(notes.len() as u16);
        for code in notes {
            group.next_entry().expect("note entry").set_code(*code);
        }
    }
    encoder.set_label(b"label").set_payload(&[7, 8, 9]);
    encoder.finish()
}

/// Reads every `orders` entry through the reader, fully.
fn read_orders(reader: &mut NestedReader<'_>) -> Vec<DecodedOrder> {
    let mut out = Vec::new();
    let mut orders = reader.orders();
    while let Some(mut order) = orders.next_entry() {
        let mut fills = Vec::new();
        {
            let mut fills_group = order.fills();
            while let Some(mut fill) = fills_group.next_entry() {
                fills.push((fill.fill_id(), fill.note().to_vec()));
            }
        }
        out.push((order.order_id(), fills, order.memo().to_vec()));
    }
    out
}

/// Reads every `orders` entry through the random-access decoder.
fn decode_orders(decoder: &NestedDecoder<'_>) -> Vec<DecodedOrder> {
    decoder
        .orders()
        .map(|order| {
            let fills = order
                .fills()
                .map(|fill| (fill.fill_id(), fill.note().to_vec()))
                .collect();
            (order.order_id(), fills, order.memo().to_vec())
        })
        .collect()
}

// ---------------------------------------------------------------------
// Agreement with the random-access decoders
// ---------------------------------------------------------------------

#[test]
fn test_reader_quote_matches_random_access_decoder() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    assert_eq!(reader.acting_version(), SCHEMA_VERSION);
    assert_eq!(reader.request_id(), decoder.request_id());

    let mut legs = Vec::new();
    {
        let mut group = reader.legs();
        assert_eq!(group.count(), 3);
        assert!(!group.is_empty());
        assert_eq!(group.remaining(), 3);
        while let Some(mut leg) = group.next_entry() {
            legs.push((leg.leg_qty(), leg.leg_tag().to_vec()));
        }
        assert_eq!(group.remaining(), 0);
        assert!(group.next_entry().is_none());
    }
    let expected: Vec<(u32, Vec<u8>)> = decoder
        .legs()
        .map(|leg| (leg.leg_qty(), leg.leg_tag().to_vec()))
        .collect();
    assert_eq!(legs, expected);

    assert_eq!(reader.comment(), decoder.comment());
    assert_eq!(reader.finish(), decoder.encoded_length());
}

#[test]
fn test_reader_nested_matches_random_access_decoder() {
    let mut buf = vec![0u8; 512];
    let trailer: Vec<u8> = (0..300u32).map(|i| (i % 7) as u8).collect();
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, &trailer);
    let decoder = NestedDecoder::decode(&buf[..len]).expect("decode Nested");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    assert_eq!(read_orders(&mut reader), decode_orders(&decoder));

    let flags: Vec<u8> = reader.flags().map(|entry| entry.flag()).collect();
    let expected_flags: Vec<u8> = decoder.flags().map(|entry| entry.flag()).collect();
    assert_eq!(flags, expected_flags);
    assert_eq!(flags, FLAGS.to_vec());

    assert_eq!(reader.trailer(), decoder.trailer());
    assert_eq!(reader.finish(), decoder.encoded_length());
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_reader_fixed_stride_groups_return_the_decoder_iterators() {
    let mut buf = [0u8; 128];
    let len = encode_example(&mut buf, &[(1, 10), (2, 20)], &[5, 6, 7]);
    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    let mut reader = ExampleReader::decode(&buf[..len]).expect("read Example");

    assert_eq!(reader.qty(), 99);
    let legs = reader.legs();
    assert_eq!(legs.len(), 2);
    let legs: Vec<(u64, u32)> = legs.map(|leg| (leg.leg_id(), leg.ratio())).collect();
    assert_eq!(legs, vec![(1, 10), (2, 20)]);
    let notes: Vec<u16> = reader.notes().map(|note| note.code()).collect();
    assert_eq!(notes, vec![5, 6, 7]);
    assert_eq!(reader.label_as_str(), "label");
    assert_eq!(reader.payload(), decoder.payload());
    assert_eq!(reader.finish(), len);
}

// ---------------------------------------------------------------------
// Partial reads: the cursor still lands on the right byte
// ---------------------------------------------------------------------

#[test]
fn test_reader_skips_untouched_group_before_var_data() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    assert_eq!(reader.comment_as_str(), "after");
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_ignored_group_accessor_skips_the_group() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    assert_eq!(reader.legs().count(), 3);
    assert_eq!(reader.comment(), b"after");
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_partially_consumed_group_then_var_data() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    {
        let mut group = reader.legs();
        let mut first = group.next_entry().expect("leg 0");
        assert_eq!(first.leg_qty(), 1);
        assert_eq!(first.leg_tag(), b"");
        drop(first);
        assert_eq!(group.remaining(), 2);
    }
    assert_eq!(reader.comment(), b"after");
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_partially_read_entry_then_next_entry() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    {
        let mut orders = reader.orders();
        {
            let first = orders.next_entry().expect("order 0");
            assert_eq!(first.order_id(), 1);
            // fills and memo are never read
        }
        {
            let mut second = orders.next_entry().expect("order 1");
            assert_eq!(second.order_id(), 2);
            assert!(second.fills().is_empty());
            assert_eq!(second.memo(), b"");
        }
        assert!(orders.next_entry().is_none());
    }
    let flags: Vec<u8> = reader.flags().map(|entry| entry.flag()).collect();
    assert_eq!(flags, FLAGS.to_vec());
    assert_eq!(reader.trailer(), b"t");
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_nested_group_partially_consumed_then_outer_var_data() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    {
        let mut orders = reader.orders();
        let mut first = orders.next_entry().expect("order 0");
        {
            let mut fills = first.fills();
            assert_eq!(fills.count(), 2);
            let fill = fills.next_entry().expect("fill 0");
            assert_eq!(fill.fill_id(), 11);
            // note of fill 0 and the whole fill 1 are never read
            drop(fill);
            assert_eq!(fills.remaining(), 1);
        }
        assert_eq!(first.memo(), b"m1");
    }
    assert_eq!(reader.trailer(), b"t");
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_entry_dropped_untouched_keeps_following_entries_aligned() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    {
        let mut orders = reader.orders();
        orders.next_entry();
        let mut second = orders.next_entry().expect("order 1");
        assert_eq!(second.order_id(), 2);
        assert_eq!(second.memo(), b"");
    }
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_finish_without_reading_anything_equals_encoded_length() {
    let mut buf = vec![0u8; 512];

    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let reader = QuoteReader::decode(&buf[..len]).expect("read Quote");
    assert_eq!(reader.finish(), len);

    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"trailer");
    let reader = NestedReader::decode(&buf[..len]).expect("read Nested");
    assert_eq!(reader.finish(), len);

    let len = encode_example(&mut buf, &[(1, 10)], &[5]);
    let reader = ExampleReader::decode(&buf[..len]).expect("read Example");
    assert_eq!(reader.finish(), len);

    let mut encoder = OnlyDataEncoder::wrap(&mut buf, 0);
    encoder.set_blob(&[1, 2, 3, 4, 5]);
    let len = encoder.finish();
    let mut reader = OnlyDataReader::decode(&buf[..len]).expect("read OnlyData");
    assert_eq!(reader.blob(), &[1, 2, 3, 4, 5]);
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_wrap_at_explicit_offset() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::wrap(&buf[..len], MessageHeader::ENCODED_LENGTH, SCHEMA_VERSION);

    assert_eq!(reader.request_id(), 7);
    assert_eq!(reader.legs().count(), 3);
    assert_eq!(reader.comment(), b"after");
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reader_empty_group_and_empty_var_data() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, &[], b"");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    {
        let mut group = reader.legs();
        assert!(group.is_empty());
        assert!(group.next_entry().is_none());
    }
    assert_eq!(reader.comment(), b"");
    assert_eq!(
        reader.finish(),
        MessageHeader::ENCODED_LENGTH
            + QuoteEncoder::BLOCK_LENGTH as usize
            + GroupHeader::ENCODED_LENGTH
            + 2
    );
}

#[test]
fn test_reader_invalid_utf8_as_str_returns_empty() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, &[(1, &[0xFF, 0xFE])], &[0xC0]);
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    {
        let mut group = reader.legs();
        let mut leg = group.next_entry().expect("leg");
        assert_eq!(leg.leg_tag_as_str(), "");
    }
    assert_eq!(reader.comment_as_str(), "");
}

// ---------------------------------------------------------------------
// Errors and misuse
// ---------------------------------------------------------------------

#[test]
fn test_reader_decode_rejects_the_same_frames_as_the_decoder() {
    let mut buf = [0u8; 128];
    let len = encode_example(&mut buf, &[], &[]);

    // Example frame handed to the Quote reader: template mismatch
    let expected = QuoteDecoder::decode(&buf[..len]).expect_err("decoder rejects");
    let actual = QuoteReader::decode(&buf[..len]).expect_err("reader rejects");
    assert_eq!(actual, expected);
    assert!(matches!(
        actual,
        DecodeError::TemplateMismatch {
            expected: QuoteReader::TEMPLATE_ID,
            actual: ExampleEncoder::TEMPLATE_ID
        }
    ));

    // header only, no root block
    let short = &buf[..MessageHeader::ENCODED_LENGTH];
    let expected = ExampleDecoder::decode(short).expect_err("decoder rejects");
    let actual = ExampleReader::decode(short).expect_err("reader rejects");
    assert_eq!(actual, expected);
    assert!(matches!(actual, DecodeError::BufferTooShort { .. }));
}

#[test]
#[should_panic(expected = "repeating group 'legs' read out of schema order")]
fn test_reader_group_after_var_data_panics() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");
    reader.comment();
    reader.legs();
}

#[test]
#[should_panic(expected = "var data field 'label' read out of schema order")]
fn test_reader_var_data_read_twice_panics() {
    let mut buf = [0u8; 128];
    let len = encode_example(&mut buf, &[], &[]);
    let mut reader = ExampleReader::decode(&buf[..len]).expect("read Example");
    reader.label();
    reader.label();
}

#[test]
#[should_panic(expected = "repeating group 'fills' read out of schema order")]
fn test_reader_entry_nested_group_after_var_data_panics() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");
    let mut orders = reader.orders();
    let mut order = orders.next_entry().expect("order 0");
    order.memo();
    order.fills();
}

#[test]
fn test_reader_drop_while_panicking_does_not_walk_a_truncated_buffer() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    // keep the header, root block and legs group header (count = 3), drop the entries
    let cut = MessageHeader::ENCODED_LENGTH
        + QuoteEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH;
    assert!(cut < len);

    let result = std::panic::catch_unwind(|| {
        let mut reader = QuoteReader::wrap(&buf[..cut], MessageHeader::ENCODED_LENGTH, 1);
        let _legs = reader.legs();
        panic!("caller panics while a group reader is alive");
    });
    // a walking drop would index past `cut` and abort the process on the
    // second panic; the guard turns it into an ordinary unwind
    assert!(result.is_err());
}
