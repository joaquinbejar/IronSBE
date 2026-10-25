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

/// Length header width of `varStringEncoding` (uint16).
const U16_HEADER_LEN: usize = 2;

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
    let mut encoder = QuoteEncoder::wrap(buf, 0).expect("wrap");
    encoder.set_request_id(7);
    {
        let mut group = encoder.legs_count(legs.len() as u16).expect("group");
        for (qty, tag) in legs {
            group
                .next_entry()
                .expect("entry fits")
                .expect("leg entry")
                .set_leg_qty(*qty)
                .set_leg_tag(tag);
        }
    }
    encoder.set_comment(comment);
    encoder.finish().expect("finish")
}

/// Encodes a `Nested` message and returns its encoded length in bytes.
fn encode_nested(buf: &mut [u8], orders: &[Order<'_>], flags: &[u8], trailer: &[u8]) -> usize {
    let mut encoder = NestedEncoder::wrap(buf, 0).expect("wrap");
    {
        let mut group = encoder.orders_count(orders.len() as u16).expect("group");
        for (order_id, fills, memo) in orders {
            let mut order = group
                .next_entry()
                .expect("entry fits")
                .expect("order entry");
            order.set_order_id(*order_id);
            {
                let mut fills_group = order.fills_count(fills.len() as u16).expect("group");
                for (fill_id, note) in fills.iter() {
                    fills_group
                        .next_entry()
                        .expect("entry fits")
                        .expect("fill entry")
                        .set_fill_id(*fill_id)
                        .set_note(note);
                }
            }
            order.set_memo(memo);
        }
    }
    {
        let mut group = encoder.flags_count(flags.len() as u16).expect("group");
        for flag in flags {
            group
                .next_entry()
                .expect("entry fits")
                .expect("flag entry")
                .set_flag(*flag);
        }
    }
    encoder.set_trailer(trailer);
    encoder.finish().expect("finish")
}

/// Encodes an `Example` message and returns its encoded length in bytes.
fn encode_example(buf: &mut [u8], legs: &[(u64, u32)], notes: &[u16]) -> usize {
    let mut encoder = ExampleEncoder::wrap(buf, 0).expect("wrap");
    encoder.set_qty(99);
    {
        let mut group = encoder.legs_count(legs.len() as u16).expect("group");
        for (leg_id, ratio) in legs {
            group
                .next_entry()
                .expect("entry fits")
                .expect("leg entry")
                .set_leg_id(*leg_id)
                .set_ratio(*ratio);
        }
    }
    {
        let mut group = encoder.notes_count(notes.len() as u16).expect("group");
        for code in notes {
            group
                .next_entry()
                .expect("entry fits")
                .expect("note entry")
                .set_code(*code);
        }
    }
    encoder.set_label(b"label").set_payload(&[7, 8, 9]);
    encoder.finish().expect("finish")
}

/// Reads every `orders` entry through the reader, fully.
fn read_orders(reader: &mut NestedReader<'_>) -> Vec<DecodedOrder> {
    let mut out = Vec::new();
    let mut orders = reader.orders().expect("orders");
    while let Some(mut order) = orders.next_entry().expect("entry fits") {
        let mut fills = Vec::new();
        {
            let mut fills_group = order.fills().expect("fills");
            while let Some(mut fill) = fills_group.next_entry().expect("entry fits") {
                fills.push((fill.fill_id(), fill.note().expect("note").to_vec()));
            }
        }
        out.push((
            order.order_id(),
            fills,
            order.memo().expect("memo").to_vec(),
        ));
    }
    out
}

/// Reads every `orders` entry through the random-access decoder.
fn decode_orders(decoder: &NestedDecoder<'_>) -> Vec<DecodedOrder> {
    decoder
        .orders()
        .expect("orders")
        .map(|e| e.expect("entry"))
        .map(|order| {
            let fills = order
                .fills()
                .expect("fills")
                .map(|e| e.expect("entry"))
                .map(|fill| (fill.fill_id(), fill.note().expect("note").to_vec()))
                .collect();
            (
                order.order_id(),
                fills,
                order.memo().expect("memo").to_vec(),
            )
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
        let mut group = reader.legs().expect("legs");
        assert_eq!(group.count(), 3);
        assert!(!group.is_empty());
        assert_eq!(group.remaining(), 3);
        while let Some(mut leg) = group.next_entry().expect("entry fits") {
            legs.push((leg.leg_qty(), leg.leg_tag().expect("leg_tag").to_vec()));
        }
        assert_eq!(group.remaining(), 0);
        assert!(group.next_entry().expect("entry fits").is_none());
    }
    let expected: Vec<(u32, Vec<u8>)> = decoder
        .legs()
        .expect("legs")
        .map(|e| e.expect("entry"))
        .map(|leg| (leg.leg_qty(), leg.leg_tag().expect("leg_tag").to_vec()))
        .collect();
    assert_eq!(legs, expected);

    assert_eq!(
        reader.comment().expect("comment"),
        decoder.comment().expect("comment")
    );
    assert_eq!(
        reader.finish().expect("finish"),
        decoder.encoded_length().expect("encoded length")
    );
}

#[test]
fn test_reader_nested_matches_random_access_decoder() {
    let mut buf = vec![0u8; 512];
    let trailer: Vec<u8> = (0..300u32).map(|i| (i % 7) as u8).collect();
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, &trailer);
    let decoder = NestedDecoder::decode(&buf[..len]).expect("decode Nested");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    assert_eq!(read_orders(&mut reader), decode_orders(&decoder));

    let flags: Vec<u8> = reader
        .flags()
        .expect("flags")
        .map(|e| e.expect("entry"))
        .map(|entry| entry.flag())
        .collect();
    let expected_flags: Vec<u8> = decoder
        .flags()
        .expect("flags")
        .map(|e| e.expect("entry"))
        .map(|entry| entry.flag())
        .collect();
    assert_eq!(flags, expected_flags);
    assert_eq!(flags, FLAGS.to_vec());

    assert_eq!(
        reader.trailer().expect("trailer"),
        decoder.trailer().expect("trailer")
    );
    assert_eq!(
        reader.finish().expect("finish"),
        decoder.encoded_length().expect("encoded length")
    );
    assert_eq!(decoder.encoded_length().expect("encoded length"), len);
}

#[test]
fn test_reader_fixed_stride_groups_return_the_decoder_iterators() {
    let mut buf = [0u8; 128];
    let len = encode_example(&mut buf, &[(1, 10), (2, 20)], &[5, 6, 7]);
    let decoder = ExampleDecoder::decode(&buf[..len]).expect("decode Example");
    let mut reader = ExampleReader::decode(&buf[..len]).expect("read Example");

    assert_eq!(reader.qty(), 99);
    let legs = reader.legs().expect("legs");
    assert_eq!(legs.len(), 2);
    let legs: Vec<(u64, u32)> = legs
        .map(|leg| leg.expect("leg"))
        .map(|leg| (leg.leg_id(), leg.ratio()))
        .collect();
    assert_eq!(legs, vec![(1, 10), (2, 20)]);
    let notes: Vec<u16> = reader
        .notes()
        .expect("notes")
        .map(|e| e.expect("entry"))
        .map(|note| note.code())
        .collect();
    assert_eq!(notes, vec![5, 6, 7]);
    assert_eq!(reader.label_as_str().expect("label_as_str"), "label");
    assert_eq!(
        reader.payload().expect("payload"),
        decoder.payload().expect("payload")
    );
    assert_eq!(reader.finish().expect("finish"), len);
}

// ---------------------------------------------------------------------
// Partial reads: the cursor still lands on the right byte
// ---------------------------------------------------------------------

#[test]
fn test_reader_skips_untouched_group_before_var_data() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    assert_eq!(reader.comment_as_str().expect("comment_as_str"), "after");
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_ignored_group_accessor_skips_the_group() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    assert_eq!(reader.legs().expect("legs").count(), 3);
    assert_eq!(reader.comment().expect("comment"), b"after");
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_partially_consumed_group_then_var_data() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    {
        let mut group = reader.legs().expect("legs");
        let mut first = group.next_entry().expect("entry fits").expect("leg 0");
        assert_eq!(first.leg_qty(), 1);
        assert_eq!(first.leg_tag().expect("leg_tag"), b"");
        drop(first);
        assert_eq!(group.remaining(), 2);
    }
    assert_eq!(reader.comment().expect("comment"), b"after");
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_partially_read_entry_then_next_entry() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    {
        let mut orders = reader.orders().expect("orders");
        {
            let first = orders.next_entry().expect("entry fits").expect("order 0");
            assert_eq!(first.order_id(), 1);
            // fills and memo are never read
        }
        {
            let mut second = orders.next_entry().expect("entry fits").expect("order 1");
            assert_eq!(second.order_id(), 2);
            assert!(second.fills().expect("fills").is_empty());
            assert_eq!(second.memo().expect("memo"), b"");
        }
        assert!(orders.next_entry().expect("entry fits").is_none());
    }
    let flags: Vec<u8> = reader
        .flags()
        .expect("flags")
        .map(|e| e.expect("entry"))
        .map(|entry| entry.flag())
        .collect();
    assert_eq!(flags, FLAGS.to_vec());
    assert_eq!(reader.trailer().expect("trailer"), b"t");
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_nested_group_partially_consumed_then_outer_var_data() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    {
        let mut orders = reader.orders().expect("orders");
        let mut first = orders.next_entry().expect("entry fits").expect("order 0");
        {
            let mut fills = first.fills().expect("fills");
            assert_eq!(fills.count(), 2);
            let fill = fills.next_entry().expect("entry fits").expect("fill 0");
            assert_eq!(fill.fill_id(), 11);
            // note of fill 0 and the whole fill 1 are never read
            drop(fill);
            assert_eq!(fills.remaining(), 1);
        }
        assert_eq!(first.memo().expect("memo"), b"m1");
    }
    assert_eq!(reader.trailer().expect("trailer"), b"t");
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_entry_dropped_untouched_keeps_following_entries_aligned() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");

    {
        let mut orders = reader.orders().expect("orders");
        orders.next_entry().expect("entry fits");
        let mut second = orders.next_entry().expect("entry fits").expect("order 1");
        assert_eq!(second.order_id(), 2);
        assert_eq!(second.memo().expect("memo"), b"");
    }
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_finish_without_reading_anything_equals_encoded_length() {
    let mut buf = vec![0u8; 512];

    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let reader = QuoteReader::decode(&buf[..len]).expect("read Quote");
    assert_eq!(reader.finish().expect("finish"), len);

    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"trailer");
    let reader = NestedReader::decode(&buf[..len]).expect("read Nested");
    assert_eq!(reader.finish().expect("finish"), len);

    let len = encode_example(&mut buf, &[(1, 10)], &[5]);
    let reader = ExampleReader::decode(&buf[..len]).expect("read Example");
    assert_eq!(reader.finish().expect("finish"), len);

    let mut encoder = OnlyDataEncoder::wrap(&mut buf, 0).expect("wrap");
    encoder.set_blob(&[1, 2, 3, 4, 5]);
    let len = encoder.finish().expect("finish");
    let mut reader = OnlyDataReader::decode(&buf[..len]).expect("read OnlyData");
    assert_eq!(reader.blob().expect("blob"), &[1, 2, 3, 4, 5]);
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_wrap_at_explicit_offset() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::wrap(&buf[..len], MessageHeader::ENCODED_LENGTH, SCHEMA_VERSION);

    assert_eq!(reader.request_id(), 7);
    assert_eq!(reader.legs().expect("legs").count(), 3);
    assert_eq!(reader.comment().expect("comment"), b"after");
    assert_eq!(reader.finish().expect("finish"), len);
}

#[test]
fn test_reader_empty_group_and_empty_var_data() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, &[], b"");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");

    {
        let mut group = reader.legs().expect("legs");
        assert!(group.is_empty());
        assert!(group.next_entry().expect("entry fits").is_none());
    }
    assert_eq!(reader.comment().expect("comment"), b"");
    assert_eq!(
        reader.finish().expect("finish"),
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
        let mut group = reader.legs().expect("legs");
        let mut leg = group.next_entry().expect("entry fits").expect("leg");
        assert_eq!(leg.leg_tag_as_str().expect("leg_tag_as_str"), "");
    }
    assert_eq!(reader.comment_as_str().expect("comment_as_str"), "");
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
fn test_reader_group_after_var_data_returns_err() {
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, &QUOTE_LEGS, b"after");
    let mut reader = QuoteReader::decode(&buf[..len]).expect("read Quote");
    reader.comment().expect("comment");
    assert!(matches!(
        reader.legs().map(|_| ()),
        Err(DecodeError::OutOfOrder {
            part: "repeating group 'legs'",
            read: 2,
        })
    ));
}

#[test]
fn test_reader_var_data_read_twice_returns_err() {
    let mut buf = [0u8; 128];
    let len = encode_example(&mut buf, &[], &[]);
    let mut reader = ExampleReader::decode(&buf[..len]).expect("read Example");
    reader.label().expect("label");
    assert_eq!(
        reader.label(),
        Err(DecodeError::OutOfOrder {
            part: "var data field 'label'",
            read: 3,
        })
    );
}

#[test]
fn test_reader_entry_nested_group_after_var_data_returns_err() {
    let mut buf = [0u8; 256];
    let len = encode_nested(&mut buf, &ORDERS, &FLAGS, b"t");
    let mut reader = NestedReader::decode(&buf[..len]).expect("read Nested");
    let mut orders = reader.orders().expect("orders");
    let mut order = orders.next_entry().expect("entry fits").expect("order 0");
    order.memo().expect("memo");
    assert!(matches!(
        order.fills().map(|_| ()),
        Err(DecodeError::OutOfOrder {
            part: "repeating group 'fills'",
            read: 2,
        })
    ));
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
        let _legs = reader.legs().expect("legs");
        panic!("caller panics while a group reader is alive");
    });
    // a drop that panicked while walking past `cut` would abort the process
    // on the second panic; the checked walk turns it into an ordinary unwind
    assert!(result.is_err());
}

// ---------------------------------------------------------------------
// Truncated frames: a skipped part is checked like a read one
// ---------------------------------------------------------------------

#[test]
fn test_reader_finish_returns_err_on_truncated_final_payload() {
    let mut buf = [0u8; 32];
    let mut encoder = OnlyDataEncoder::wrap(&mut buf, 0).expect("wrap");
    encoder.set_blob(&[1, 2, 3, 4, 5]);
    let len = encoder.finish().expect("finish");
    // keep the message header and the uint32 length header, drop the payload
    let cut = MessageHeader::ENCODED_LENGTH + 4;
    assert!(cut < len);

    let reader = OnlyDataReader::decode(&buf[..cut]).expect("header and root block are present");
    assert_eq!(
        reader.finish(),
        Err(DecodeError::VarPartOutOfBounds {
            part: "var data field 'blob'",
            required: cut + 5,
            available: cut,
        })
    );
}

#[test]
fn test_reader_entry_drop_on_truncated_var_data_makes_later_reads_return_err() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, &[(1, b"abc")], b"c");
    // keep everything up to and including the legTag length header
    let cut = MessageHeader::ENCODED_LENGTH
        + QuoteEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH
        + 4
        + U16_HEADER_LEN;
    assert!(cut < len);

    let mut reader = QuoteReader::wrap(&buf[..cut], MessageHeader::ENCODED_LENGTH, SCHEMA_VERSION);
    {
        let mut legs = reader.legs().expect("legs");
        let entry = legs.next_entry().expect("entry fits").expect("entry");
        assert_eq!(entry.leg_qty(), 1);
        drop(entry); // walks legTag: header present, payload missing
    }
    assert!(matches!(
        reader.comment(),
        Err(DecodeError::VarPartOutOfBounds {
            part: "var data field 'comment'",
            ..
        })
    ));
    assert!(matches!(
        reader.finish(),
        Err(DecodeError::VarPartOutOfBounds { .. })
    ));
}

#[test]
fn test_reader_group_drop_on_truncated_entries_makes_later_reads_return_err() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, &[(1, b"abc")], b"c");
    let cut = MessageHeader::ENCODED_LENGTH
        + QuoteEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH
        + 4
        + U16_HEADER_LEN;
    assert!(cut < len);

    let mut reader = QuoteReader::wrap(&buf[..cut], MessageHeader::ENCODED_LENGTH, SCHEMA_VERSION);
    // dropped untouched: walks the entry, whose payload is missing
    drop(reader.legs().expect("legs header is present"));
    assert!(matches!(
        reader.comment(),
        Err(DecodeError::VarPartOutOfBounds {
            part: "var data field 'comment'",
            ..
        })
    ));
}

#[test]
fn test_reader_skipped_group_returns_err_when_entries_are_missing() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, &[(1, b"abc")], b"c");
    let cut = MessageHeader::ENCODED_LENGTH
        + QuoteEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH
        + 4
        + U16_HEADER_LEN;
    assert!(cut < len);

    let mut reader = QuoteReader::wrap(&buf[..cut], MessageHeader::ENCODED_LENGTH, SCHEMA_VERSION);
    // skips legs first: the entry's legTag payload is missing
    assert_eq!(
        reader.comment(),
        Err(DecodeError::VarPartOutOfBounds {
            part: "var data field 'legTag'",
            required: cut + 3,
            available: cut,
        })
    );
}

#[test]
fn test_reader_fixed_group_accessor_returns_err_on_truncated_group() {
    let mut buf = [0u8; 128];
    let len = encode_example(&mut buf, &[(1, 10), (2, 20)], &[]);
    // keep the group header (count = 2) and one 12-byte entry
    let cut = MessageHeader::ENCODED_LENGTH
        + ExampleEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH
        + 12;
    assert!(cut < len);

    let mut reader =
        ExampleReader::wrap(&buf[..cut], MessageHeader::ENCODED_LENGTH, SCHEMA_VERSION);
    assert!(matches!(
        reader.legs().map(|_| ()),
        Err(DecodeError::VarPartOutOfBounds {
            part: "repeating group 'legs'",
            required,
            available,
        }) if required == cut + 12 && available == cut
    ));
}

#[test]
fn test_reader_exact_frame_is_not_rejected() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, &[(1, b"abc")], b"c");
    let reader = QuoteReader::decode(&buf[..len]).expect("read Quote");
    assert_eq!(
        reader.finish().expect("finish"),
        len,
        "a frame that ends exactly on its last byte is complete"
    );
}
