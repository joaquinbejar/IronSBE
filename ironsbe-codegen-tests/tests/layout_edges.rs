//! Layout edge cases of schemas laid out without `offset` attributes, from
//! the review of the implicit offset fix: explicit offsets that leave a
//! gap, same-named inline types, inline members at explicit offsets and
//! signed or whole-number constant spellings. The codecs compile and
//! encode each field where SBE 1.0 puts it.

use ironsbe_codegen_tests::layout_edges::*;
use ironsbe_core::decoder::SbeDecoder;
use ironsbe_core::header::MessageHeader;

/// Block: `pad` 0..8 (a gap at 1..4), `after` 8, `order` 9, `fill` 10..12,
/// `spaced` 12..15 (`Side` at 2), `limits` 15 (only `v`).
#[test]
fn test_fields_follow_the_composites_extents() {
    assert_eq!(Pad::ENCODED_LENGTH, 8);
    assert_eq!(Order::ENCODED_LENGTH, 1);
    assert_eq!(Fill::ENCODED_LENGTH, 2);
    assert_eq!(Spaced::ENCODED_LENGTH, 3);
    assert_eq!(Limits::ENCODED_LENGTH, 1);
    assert_eq!(EdgesEncoder::BLOCK_LENGTH, 16);
}

#[test]
fn test_each_field_lands_at_its_offset_and_reads_back() {
    let mut buf = [0u8; 64];
    let mut encoder = EdgesEncoder::wrap(&mut buf, 0).expect("wrap");
    encoder.set_pad().set_a(0xAA).set_b(0x1122_3344);
    encoder.set_after(0x55);
    encoder.set_order().set_status(Status::Done);
    encoder.set_fill().set_status(FillStatus::Full);
    encoder.set_spaced().set_x(7).set_side(Side::Sell);
    encoder.set_limits().set_v(9);
    let len = encoder.encoded_length();
    assert_eq!(len, MessageHeader::ENCODED_LENGTH + 16);

    let block = &buf[MessageHeader::ENCODED_LENGTH..len];
    assert_eq!(
        block,
        [
            0xAA, 0, 0, 0, 0x44, 0x33, 0x22, 0x11, // pad, its gap at 1..4
            0x55, // after
            2,    // order.Status (uint8)
            0x90, 0x01, // fill.Status (uint16, 400)
            7, 0, 2, // spaced: x, a gap, Side at 2
            9, // limits.v, its constants take no bytes
        ]
    );

    let decoder = EdgesDecoder::decode(&buf[..len]).expect("decode");
    assert_eq!((decoder.pad().a(), decoder.pad().b()), (0xAA, 0x1122_3344));
    assert_eq!(decoder.after(), 0x55);
    assert_eq!(decoder.order().status(), Status::Done);
    assert_eq!(decoder.fill().status(), FillStatus::Full);
    assert_eq!(decoder.spaced().side(), Side::Sell);
    let limits = decoder.limits();
    assert_eq!(limits.v(), 9);
    assert_eq!(limits.plus_one(), 1_u16);
    assert_eq!(limits.minus_five(), -5_i8);
    assert_eq!(limits.plus_half(), 1.5_f32);
    assert_eq!(limits.two(), 2.0_f64);
}
