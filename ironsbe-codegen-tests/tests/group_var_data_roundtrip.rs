//! Round-trip tests for `<data>` fields and nested groups inside repeating
//! group entries.
//!
//! Regression coverage for <https://github.com/joaquinbejar/IronSBE/issues/61>:
//! per SBE 1.0 a group entry is laid out as fixed-length fields, then nested
//! repeating groups, then variable-length data, so the entry extent is not
//! known from `blockLength` alone. These tests exercise the generated
//! variable-stride decoders and the cursor-threaded encoders end to end,
//! including exact wire bytes.

use ironsbe_codegen_tests::var_data::{NestedDecoder, NestedEncoder, QuoteDecoder, QuoteEncoder};
use ironsbe_core::decoder::SbeDecoder;
use ironsbe_core::header::{GroupHeader, MessageHeader};

/// Length header width of `varStringEncoding` (uint16).
const U16_HEADER: usize = 2;
/// Length header width of `varDataEncoding8` (uint8).
const U8_HEADER: usize = 1;
/// Length header width of `varDataEncoding32` (uint32).
const U32_HEADER: usize = 4;
/// Bytes per `legs` entry fixed block (`legQty` u32).
const LEG_BLOCK_LENGTH: usize = 4;
/// Bytes per `orders` entry fixed block (`orderId` u64).
const ORDER_BLOCK_LENGTH: usize = 8;
/// Bytes per `fills` entry fixed block (`fillId` u64).
const FILL_BLOCK_LENGTH: usize = 8;
/// Bytes per `flags` entry fixed block (`flag` u8).
const FLAG_BLOCK_LENGTH: usize = 1;

/// One `legs` entry: fixed `legQty` followed by the `legTag` var string.
type Leg<'a> = (u32, &'a [u8]);

/// Encodes a `Quote` message and returns its encoded length in bytes.
fn encode_quote(buf: &mut [u8], request_id: u32, legs: &[Leg<'_>], comment: &[u8]) -> usize {
    let mut encoder = QuoteEncoder::wrap(buf, 0);
    encoder.set_request_id(request_id);
    {
        let mut group = encoder.legs_count(legs.len() as u16);
        for (qty, tag) in legs {
            let mut entry = group.next_entry().expect("leg entry");
            entry.set_leg_qty(*qty).set_leg_tag(tag);
        }
        assert!(group.next_entry().is_none(), "legs group over-iterated");
    }
    encoder.set_comment(comment);
    encoder.encoded_length()
}

/// Expected wire size of a `Quote` message.
fn expected_quote_length(legs: &[Leg<'_>], comment: usize) -> usize {
    MessageHeader::ENCODED_LENGTH
        + QuoteEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH
        + legs
            .iter()
            .map(|(_, tag)| LEG_BLOCK_LENGTH + U16_HEADER + tag.len())
            .sum::<usize>()
        + U16_HEADER
        + comment
}

/// Decodes the `legs` group of a `Quote` message into owned pairs.
fn decode_legs(decoder: &QuoteDecoder<'_>) -> Vec<(u32, Vec<u8>)> {
    decoder
        .legs()
        .map(|entry| (entry.leg_qty(), entry.leg_tag().to_vec()))
        .collect()
}

#[test]
fn test_roundtrip_group_with_single_var_data_matches() {
    let legs: [Leg<'_>; 2] = [(1, b"A"), (2, b"long-tag")];
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, 7, &legs, b"c");

    assert_eq!(len, expected_quote_length(&legs, 1));

    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");
    assert_eq!(decoder.request_id(), 7);
    assert_eq!(decoder.legs().count(), 2);
    assert_eq!(
        decode_legs(&decoder),
        vec![(1, b"A".to_vec()), (2, b"long-tag".to_vec())]
    );
    assert_eq!(decoder.comment(), b"c");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_wire_layout_fixed_field_then_var_data_in_entry_exact_bytes() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, 7, &[(10, b"ab")], b"");

    let expected: Vec<u8> = [
        // message header: blockLength=4, templateId=4, schemaId=42, version=1
        &[0x04, 0x00, 0x04, 0x00, 0x2A, 0x00, 0x01, 0x00][..],
        // requestId = 7
        &[0x07, 0x00, 0x00, 0x00],
        // legs group header: blockLength=4, numInGroup=1
        &[0x04, 0x00, 0x01, 0x00],
        // entry 0: legQty = 10, then legTag length=2, "ab"
        &[0x0A, 0x00, 0x00, 0x00],
        &[0x02, 0x00, b'a', b'b'],
        // message-level comment: length=0
        &[0x00, 0x00],
    ]
    .concat();

    assert_eq!(&buf[..len], expected.as_slice());
    assert_eq!(len, 26);
}

#[test]
fn test_roundtrip_message_var_data_after_variable_stride_group() {
    let legs: [Leg<'_>; 3] = [(1, b""), (2, b"xyz"), (3, b"0123456789")];
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, 1, &legs, b"after");

    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");
    assert_eq!(
        decoder.comment(),
        b"after",
        "message var data must start after the last entry's var data, not after blockLength * count"
    );
    assert_eq!(decoder.comment_as_str(), "after");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_roundtrip_empty_group_with_var_data_entries() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, 3, &[], b"x");

    assert_eq!(len, expected_quote_length(&[], 1));

    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");
    assert!(decoder.legs().is_empty());
    assert_eq!(decoder.legs().count(), 0);
    assert_eq!(decoder.comment(), b"x");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_entry_end_offset_matches_next_entry_start() {
    let legs: [Leg<'_>; 2] = [(1, b"A"), (2, b"long-tag")];
    let mut buf = [0u8; 128];
    let len = encode_quote(&mut buf, 7, &legs, b"hello");
    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");

    let entries_start = MessageHeader::ENCODED_LENGTH
        + QuoteEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH;

    let mut legs_iter = decoder.legs();
    let first = legs_iter.next().expect("first leg");
    assert_eq!(
        first.end_offset(),
        entries_start + LEG_BLOCK_LENGTH + U16_HEADER + 1
    );
    let second = legs_iter.next().expect("second leg");
    assert_eq!(
        second.end_offset(),
        first.end_offset() + LEG_BLOCK_LENGTH + U16_HEADER + 8
    );
    assert!(legs_iter.next().is_none());

    // group end == last entry end == where the message-level comment starts
    let comment_pos = decoder.legs().end_offset();
    assert_eq!(comment_pos, second.end_offset());
    assert_eq!(
        &buf[comment_pos..comment_pos + U16_HEADER],
        &5u16.to_le_bytes()
    );
}

#[test]
fn test_group_encoder_encoded_length_tracks_written_bytes() {
    let mut buf = [0u8; 128];
    let mut encoder = QuoteEncoder::wrap(&mut buf, 0);
    encoder.set_request_id(1);

    let after_block = MessageHeader::ENCODED_LENGTH + QuoteEncoder::BLOCK_LENGTH as usize;
    assert_eq!(encoder.encoded_length(), after_block);

    let group_len = {
        let mut group = encoder.legs_count(2);
        assert_eq!(group.count(), 2);
        assert_eq!(group.encoded_length(), GroupHeader::ENCODED_LENGTH);

        group
            .next_entry()
            .expect("leg 0")
            .set_leg_qty(1)
            .set_leg_tag(b"A");
        assert_eq!(
            group.encoded_length(),
            GroupHeader::ENCODED_LENGTH + LEG_BLOCK_LENGTH + U16_HEADER + 1
        );

        group
            .next_entry()
            .expect("leg 1")
            .set_leg_qty(2)
            .set_leg_tag(b"long-tag");
        group.encoded_length()
    };
    assert_eq!(
        group_len,
        GroupHeader::ENCODED_LENGTH
            + (LEG_BLOCK_LENGTH + U16_HEADER + 1)
            + (LEG_BLOCK_LENGTH + U16_HEADER + 8)
    );

    // the parent's cursor moved with the group
    assert_eq!(encoder.encoded_length(), after_block + group_len);
    encoder.set_comment(b"c");
    assert_eq!(
        encoder.encoded_length(),
        after_block + group_len + U16_HEADER + 1
    );
}

/// One `fills` entry: fixed `fillId` followed by the `note` var data (uint8 header).
type Fill<'a> = (u64, &'a [u8]);
/// One `orders` entry: fixed `orderId`, nested `fills`, then the `memo` var string.
type Order<'a> = (u64, &'a [Fill<'a>], &'a [u8]);
/// Owned form of one decoded `fills` entry.
type DecodedFill = (u64, Vec<u8>);
/// Owned form of one decoded `orders` entry.
type DecodedOrder = (u64, Vec<DecodedFill>, Vec<u8>);

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
                assert!(fills_group.next_entry().is_none());
            }
            order.set_memo(memo);
        }
        assert!(group.next_entry().is_none());
    }
    {
        let mut group = encoder.flags_count(flags.len() as u16);
        for flag in flags {
            group.next_entry().expect("flag entry").set_flag(*flag);
        }
    }
    encoder.set_trailer(trailer);
    encoder.encoded_length()
}

/// Expected wire size of a `Nested` message.
fn expected_nested_length(orders: &[Order<'_>], flags: usize, trailer: usize) -> usize {
    let orders_len: usize = orders
        .iter()
        .map(|(_, fills, memo)| {
            let fills_len: usize = fills
                .iter()
                .map(|(_, note)| FILL_BLOCK_LENGTH + U8_HEADER + note.len())
                .sum();
            ORDER_BLOCK_LENGTH + GroupHeader::ENCODED_LENGTH + fills_len + U16_HEADER + memo.len()
        })
        .sum();
    MessageHeader::ENCODED_LENGTH
        + NestedEncoder::BLOCK_LENGTH as usize
        + GroupHeader::ENCODED_LENGTH
        + orders_len
        + GroupHeader::ENCODED_LENGTH
        + flags * FLAG_BLOCK_LENGTH
        + U32_HEADER
        + trailer
}

#[test]
fn test_roundtrip_nested_group_with_var_data_in_inner_entries() {
    let fills_a: [Fill<'_>; 2] = [(11, &[1]), (12, &[2, 3])];
    let fills_b: [Fill<'_>; 0] = [];
    let orders: [Order<'_>; 2] = [(1, &fills_a, b"m1"), (2, &fills_b, b"")];
    let flags = [0x0A, 0x0B];
    let trailer: Vec<u8> = (0..300u32).map(|i| (i % 7) as u8).collect();

    let mut buf = vec![0u8; 1024];
    let len = encode_nested(&mut buf, &orders, &flags, &trailer);
    assert_eq!(
        len,
        expected_nested_length(&orders, flags.len(), trailer.len())
    );

    let decoder = NestedDecoder::decode(&buf[..len]).expect("decode Nested");

    let decoded_orders: Vec<DecodedOrder> = decoder
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
        decoded_orders,
        vec![
            (1, vec![(11, vec![1]), (12, vec![2, 3])], b"m1".to_vec()),
            (2, vec![], Vec::new()),
        ]
    );

    // the flat group after the variable one is read from the right place
    let decoded_flags: Vec<u8> = decoder.flags().map(|entry| entry.flag()).collect();
    assert_eq!(decoded_flags, vec![0x0A, 0x0B]);

    assert_eq!(decoder.trailer(), trailer.as_slice());
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_wire_layout_second_group_follows_variable_group_end() {
    let fills_a: [Fill<'_>; 1] = [(11, &[9])];
    let orders: [Order<'_>; 1] = [(1, &fills_a, b"memo")];
    let mut buf = [0u8; 128];
    let len = encode_nested(&mut buf, &orders, &[0xFF], &[]);

    let orders_header = MessageHeader::ENCODED_LENGTH + NestedEncoder::BLOCK_LENGTH as usize;
    let order_entry = orders_header + GroupHeader::ENCODED_LENGTH;
    let fills_header = order_entry + ORDER_BLOCK_LENGTH;
    let fill_entry = fills_header + GroupHeader::ENCODED_LENGTH;
    let memo_pos = fill_entry + FILL_BLOCK_LENGTH + U8_HEADER + 1;
    let flags_header = memo_pos + U16_HEADER + 4;
    let trailer_pos = flags_header + GroupHeader::ENCODED_LENGTH + FLAG_BLOCK_LENGTH;

    assert_eq!(
        { GroupHeader::wrap(&buf[..], orders_header).num_in_group },
        1
    );
    assert_eq!(
        { GroupHeader::wrap(&buf[..], fills_header).num_in_group },
        1
    );
    assert_eq!(&buf[memo_pos..memo_pos + U16_HEADER], &4u16.to_le_bytes());
    assert_eq!(
        &buf[memo_pos + U16_HEADER..memo_pos + U16_HEADER + 4],
        b"memo"
    );
    let flags = GroupHeader::wrap(&buf[..], flags_header);
    assert_eq!({ flags.block_length } as usize, FLAG_BLOCK_LENGTH);
    assert_eq!({ flags.num_in_group }, 1);
    assert_eq!(buf[flags_header + GroupHeader::ENCODED_LENGTH], 0xFF);
    assert_eq!(
        &buf[trailer_pos..trailer_pos + U32_HEADER],
        &0u32.to_le_bytes()
    );
    assert_eq!(trailer_pos + U32_HEADER, len);

    // decoder-side positions agree with the encoder-side layout
    let decoder = NestedDecoder::decode(&buf[..len]).expect("decode Nested");
    assert_eq!(decoder.orders().end_offset(), flags_header);
    let order = decoder.orders().next().expect("order");
    assert_eq!(order.fills().end_offset(), memo_pos);
    assert_eq!(order.end_offset(), flags_header);
}

#[test]
#[should_panic(expected = "exceeds u8::MAX")]
fn test_entry_var_data_over_u8_max_panics() {
    let too_long = [0u8; 256];
    let fills: [Fill<'_>; 1] = [(1, &too_long)];
    let orders: [Order<'_>; 1] = [(1, &fills, b"")];
    let mut buf = [0u8; 1024];
    encode_nested(&mut buf, &orders, &[], &[]);
}

#[test]
fn test_entry_invalid_utf8_as_str_returns_empty() {
    let mut buf = [0u8; 64];
    let len = encode_quote(&mut buf, 1, &[(1, &[0xFF, 0xFE])], b"");

    let decoder = QuoteDecoder::decode(&buf[..len]).expect("decode Quote");
    let leg = decoder.legs().next().expect("leg");
    assert_eq!(leg.leg_tag(), &[0xFF, 0xFE]);
    assert_eq!(leg.leg_tag_as_str(), "");
}
