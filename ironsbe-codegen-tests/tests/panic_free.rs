//! Runtime panic-freedom of the generated codecs (issue #74).
//!
//! The codecs in `var_data_no_panic` are compiled under
//! `deny(clippy::panic, clippy::indexing_slicing, ...)`; these tests cover
//! the runtime side. Every message of the test schema is encoded, then:
//!
//! - every prefix of the frame is decoded and walked completely through
//!   both the random-access decoder and the sequential reader;
//! - deterministic byte mutations of the frame are walked the same way;
//! - the message is encoded into every buffer size from 0 up to its length.
//!
//! A panic anywhere fails the test. A truncated frame must also surface an
//! `Err` somewhere in the walk, and an encode into a short buffer must
//! either fail or produce the same bytes as the full-size encode.

use ironsbe_codegen_tests::var_data_no_panic::{
    ExampleDecoder, ExampleEncoder, ExampleReader, GroupsOnlyDecoder, GroupsOnlyEncoder,
    NestedDecoder, NestedEncoder, NestedReader, OnlyDataDecoder, OnlyDataEncoder, OnlyDataReader,
    QuoteDecoder, QuoteEncoder, QuoteReader,
};
use ironsbe_core::decoder::{DecodeError, SbeDecoder};
use ironsbe_core::error::EncodeError;

/// Buffer size large enough for every full-size encode below.
const FULL_BUFFER: usize = 512;
/// Mutated frames walked per message.
const MUTATIONS: usize = 10_000;

// ---------------------------------------------------------------------------
// Encoders: one per message, fallible end to end.
// ---------------------------------------------------------------------------

fn encode_example(buf: &mut [u8]) -> Result<usize, EncodeError> {
    let mut encoder = ExampleEncoder::wrap(buf, 0)?;
    encoder.set_qty(42);
    {
        let mut legs = encoder.legs_count(2)?;
        for (leg_id, ratio) in [(1u64, 10u32), (2, 20)] {
            if let Some(mut entry) = legs.next_entry()? {
                entry.set_leg_id(leg_id).set_ratio(ratio);
            }
        }
    }
    {
        let mut notes = encoder.notes_count(3)?;
        for code in [7u16, 8, 9] {
            if let Some(mut entry) = notes.next_entry()? {
                entry.set_code(code);
            }
        }
    }
    encoder.try_set_label(b"label")?.try_set_payload(b"pay")?;
    encoder.finish()
}

fn encode_only_data(buf: &mut [u8]) -> Result<usize, EncodeError> {
    let mut encoder = OnlyDataEncoder::wrap(buf, 0)?;
    encoder.try_set_blob(b"0123456789")?;
    encoder.finish()
}

fn encode_groups_only(buf: &mut [u8]) -> Result<usize, EncodeError> {
    let mut encoder = GroupsOnlyEncoder::wrap(buf, 0)?;
    encoder.set_request_id(5);
    {
        let mut items = encoder.items_count(3)?;
        for item_id in [10u64, 20, 30] {
            if let Some(mut entry) = items.next_entry()? {
                entry.set_item_id(item_id);
            }
        }
    }
    encoder.finish()
}

fn encode_quote(buf: &mut [u8]) -> Result<usize, EncodeError> {
    let mut encoder = QuoteEncoder::wrap(buf, 0)?;
    encoder.set_request_id(9);
    {
        let mut legs = encoder.legs_count(3)?;
        for (qty, tag) in [(1u32, &b"ab"[..]), (2, b""), (3, b"xyz")] {
            if let Some(mut entry) = legs.next_entry()? {
                entry.set_leg_qty(qty);
                if !tag.is_empty() {
                    entry.try_set_leg_tag(tag)?;
                }
            }
        }
    }
    encoder.try_set_comment(b"done")?;
    encoder.finish()
}

fn encode_nested(buf: &mut [u8]) -> Result<usize, EncodeError> {
    let mut encoder = NestedEncoder::wrap(buf, 0)?;
    {
        let mut orders = encoder.orders_count(2)?;
        for order_id in [100u64, 200] {
            if let Some(mut order) = orders.next_entry()? {
                order.set_order_id(order_id);
                {
                    let mut fills = order.fills_count(2)?;
                    for fill_id in [1u64, 2] {
                        if let Some(mut fill) = fills.next_entry()? {
                            fill.set_fill_id(fill_id);
                            fill.try_set_note(b"n")?;
                        }
                    }
                }
                order.try_set_memo(b"memo")?;
            }
        }
    }
    {
        let mut flags = encoder.flags_count(2)?;
        for flag in [1u8, 0] {
            if let Some(mut entry) = flags.next_entry()? {
                entry.set_flag(flag);
            }
        }
    }
    encoder.try_set_trailer(b"end")?;
    encoder.finish()
}

// ---------------------------------------------------------------------------
// Walkers: read every part of a frame, returning the first error.
// ---------------------------------------------------------------------------

fn walk_example(frame: &[u8]) -> Result<(), DecodeError> {
    let decoder = ExampleDecoder::decode(frame)?;
    let _ = decoder.qty();
    for leg in decoder.legs()? {
        let leg = leg?;
        let _ = (leg.leg_id(), leg.ratio());
    }
    for note in decoder.notes()? {
        let _ = note?.code();
    }
    let _ = (
        decoder.label()?,
        decoder.label_as_str()?,
        decoder.payload()?,
    );
    decoder.encoded_length()?;

    let mut reader = ExampleReader::decode(frame)?;
    for leg in reader.legs()? {
        let _ = leg?.leg_id();
    }
    for note in reader.notes()? {
        let _ = note?.code();
    }
    let _ = (reader.label()?, reader.payload()?);
    reader.finish()?;
    Ok(())
}

fn walk_only_data(frame: &[u8]) -> Result<(), DecodeError> {
    let decoder = OnlyDataDecoder::decode(frame)?;
    let _ = decoder.blob()?;
    decoder.encoded_length()?;
    let mut reader = OnlyDataReader::decode(frame)?;
    let _ = reader.blob_as_str()?;
    reader.finish()?;
    Ok(())
}

fn walk_groups_only(frame: &[u8]) -> Result<(), DecodeError> {
    let decoder = GroupsOnlyDecoder::decode(frame)?;
    let _ = decoder.request_id();
    for item in decoder.items()? {
        let _ = item?.item_id();
    }
    decoder.encoded_length()?;
    Ok(())
}

fn walk_quote(frame: &[u8]) -> Result<(), DecodeError> {
    let decoder = QuoteDecoder::decode(frame)?;
    let _ = decoder.request_id();
    for leg in decoder.legs()? {
        let leg = leg?;
        let _ = (leg.leg_qty(), leg.leg_tag()?);
    }
    let _ = decoder.comment()?;
    decoder.encoded_length()?;

    // Sequential reader, reading every entry's var data.
    let mut reader = QuoteReader::decode(frame)?;
    {
        let mut legs = reader.legs()?;
        while let Some(mut leg) = legs.next_entry()? {
            let _ = (leg.leg_qty(), leg.leg_tag()?);
        }
    }
    let _ = reader.comment()?;
    reader.finish()?;

    // Sequential reader skipping everything: the drop and finish walks.
    let mut skipping = QuoteReader::decode(frame)?;
    {
        let mut legs = skipping.legs()?;
        let _first = legs.next_entry()?;
    }
    skipping.finish()?;
    Ok(())
}

fn walk_nested(frame: &[u8]) -> Result<(), DecodeError> {
    let decoder = NestedDecoder::decode(frame)?;
    for order in decoder.orders()? {
        let order = order?;
        let _ = order.order_id();
        for fill in order.fills()? {
            let fill = fill?;
            let _ = (fill.fill_id(), fill.note()?);
        }
        let _ = order.memo_as_str()?;
        order.end_offset()?;
    }
    for flag in decoder.flags()? {
        let _ = flag?.flag();
    }
    let _ = decoder.trailer()?;
    decoder.encoded_length()?;

    let mut reader = NestedReader::decode(frame)?;
    {
        let mut orders = reader.orders()?;
        while let Some(mut order) = orders.next_entry()? {
            let _ = order.order_id();
            {
                let mut fills = order.fills()?;
                while let Some(mut fill) = fills.next_entry()? {
                    let _ = (fill.fill_id(), fill.note()?);
                }
            }
            let _ = order.memo()?;
        }
    }
    for flag in reader.flags()? {
        let _ = flag?.flag();
    }
    let _ = reader.trailer()?;
    reader.finish()?;

    // Skip-heavy path: every drop walks what was not read.
    let mut skipping = NestedReader::decode(frame)?;
    {
        let mut orders = skipping.orders()?;
        if let Some(mut order) = orders.next_entry()? {
            let _fills = order.fills()?;
        }
    }
    skipping.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sweeps.
// ---------------------------------------------------------------------------

type Encode = fn(&mut [u8]) -> Result<usize, EncodeError>;
type Walk = fn(&[u8]) -> Result<(), DecodeError>;

const MESSAGES: [(&str, Encode, Walk); 5] = [
    ("Example", encode_example, walk_example),
    ("OnlyData", encode_only_data, walk_only_data),
    ("GroupsOnly", encode_groups_only, walk_groups_only),
    ("Quote", encode_quote, walk_quote),
    ("Nested", encode_nested, walk_nested),
];

/// Encodes into a full-size buffer and returns the frame bytes.
fn full_frame(encode: Encode) -> Vec<u8> {
    let mut buf = vec![0u8; FULL_BUFFER];
    let len = encode(&mut buf).expect("full-size encode succeeds");
    buf.truncate(len);
    buf
}

#[test]
fn test_panic_free_full_frames_walk_without_error() {
    for (name, encode, walk) in MESSAGES {
        let frame = full_frame(encode);
        assert_eq!(
            walk(&frame),
            Ok(()),
            "{name}: valid frame must walk cleanly"
        );
    }
}

#[test]
fn test_panic_free_every_truncated_prefix_returns_err() {
    for (name, encode, walk) in MESSAGES {
        let frame = full_frame(encode);
        for n in 0..frame.len() {
            let prefix = frame.get(..n).expect("n < len");
            assert!(
                walk(prefix).is_err(),
                "{name}: prefix of {n}/{} bytes walked without an error",
                frame.len()
            );
        }
    }
}

/// Deterministic xorshift64 so the mutation sweep is reproducible.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

#[test]
fn test_panic_free_mutated_frames_never_panic() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for (name, encode, walk) in MESSAGES {
        let frame = full_frame(encode);
        let mut ok = 0usize;
        let mut err = 0usize;
        for _ in 0..MUTATIONS {
            let mut mutated = frame.clone();
            let flips = 1 + rng.below(3);
            for _ in 0..flips {
                let at = rng.below(mutated.len());
                let value = rng.next() as u8;
                if let Some(byte) = mutated.get_mut(at) {
                    *byte = value;
                }
            }
            // Occasionally also truncate, so lengths that now point past the
            // end meet a shorter buffer.
            if rng.below(4) == 0 {
                mutated.truncate(rng.below(mutated.len() + 1));
            }
            match walk(&mutated) {
                Ok(()) => ok += 1,
                Err(_) => err += 1,
            }
        }
        assert_eq!(ok + err, MUTATIONS, "{name}");
    }
}

#[test]
fn test_panic_free_encode_into_every_short_buffer() {
    for (name, encode, _) in MESSAGES {
        let frame = full_frame(encode);
        for size in 0..=frame.len() {
            let mut buf = vec![0u8; size];
            match encode(&mut buf) {
                Ok(len) => {
                    assert_eq!(size, frame.len(), "{name}: encode fit in {size} bytes");
                    assert_eq!(buf.get(..len), Some(frame.as_slice()), "{name}");
                }
                Err(EncodeError::BufferTooShort { available, .. }) => {
                    assert_eq!(available, size, "{name}");
                    assert!(size < frame.len(), "{name}: full-size buffer rejected");
                }
                Err(other) => panic!("{name}: unexpected error {other:?} at size {size}"),
            }
        }
    }
}

#[test]
fn test_panic_free_entry_drop_without_room_poisons_finish() {
    // Room for the header, the orders group header and one entry block, but
    // not for that entry's empty fills header and memo header written on drop.
    let mut buf = vec![0u8; 8 + 4 + 8];
    let mut encoder = NestedEncoder::wrap(&mut buf, 0).expect("header fits");
    {
        let mut orders = encoder.orders_count(1).expect("group header fits");
        let mut order = orders
            .next_entry()
            .expect("entry block fits")
            .expect("one entry");
        order.set_order_id(1);
    }
    assert!(matches!(
        encoder.finish(),
        Err(EncodeError::BufferTooShort { .. })
    ));
}

#[test]
fn test_panic_free_reader_out_of_order_returns_err() {
    let frame = full_frame(encode_quote);
    let mut reader = QuoteReader::decode(&frame).expect("decode");
    let _ = reader.comment().expect("comment");
    assert!(matches!(
        reader.legs().map(|_| ()),
        Err(DecodeError::OutOfOrder {
            part: "repeating group 'legs'",
            read: 2,
        })
    ));
}

#[test]
fn test_panic_free_group_block_shorter_than_fields_returns_err() {
    let mut frame = full_frame(encode_quote);
    // The legs group header follows the 8-byte message header and the
    // 4-byte root block; zero its blockLength.
    let at = 8 + 4;
    if let Some(bytes) = frame.get_mut(at..at + 2) {
        bytes.copy_from_slice(&0u16.to_le_bytes());
    }
    let decoder = QuoteDecoder::decode(&frame).expect("root block is intact");
    assert!(matches!(
        decoder.legs().map(|_| ()),
        Err(DecodeError::BlockLengthTooSmall {
            required: 4,
            declared: 0,
            ..
        })
    ));
}
