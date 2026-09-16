//! Compiled regression for schema names that collide with generated methods.
//!
//! A `<data name="finish">` used to emit a `finish()` getter next to the
//! reader's `finish()` finaliser and fail to compile (PR #65 review). The
//! generator now renames colliding accessors with a trailing underscore on
//! decoders and readers (`wrap_`, `decode_`, `finish_`, `end_offset_`);
//! setters and `_as_str` variants keep their plain name, so `set_wrap`,
//! `decode_count`, `set_finish` and `finish_as_str` are unchanged and
//! `finish()` still completes the message.

use ironsbe_codegen_tests::var_data::{ReservedDecoder, ReservedEncoder, ReservedReader};
use ironsbe_core::decoder::SbeDecoder;
use ironsbe_core::header::{GroupHeader, MessageHeader};

/// Length header width of `varStringEncoding` (uint16).
const U16_HEADER: usize = 2;
/// Bytes per `decode` entry (`endOffset` u16).
const DECODE_BLOCK_LENGTH: usize = 2;

/// Encodes a `Reserved` message and returns its encoded length in bytes.
fn encode_reserved(buf: &mut [u8], wrap: u32, end_offsets: &[u16], finish: &[u8]) -> usize {
    let mut encoder = ReservedEncoder::wrap(buf, 0);
    encoder.set_wrap(wrap);
    {
        let mut group = encoder.decode_count(end_offsets.len() as u16);
        for end_offset in end_offsets {
            group
                .next_entry()
                .expect("decode entry")
                .set_end_offset(*end_offset);
        }
    }
    encoder.set_finish(finish);
    encoder.finish()
}

#[test]
fn test_reserved_names_round_trip_through_decoder() {
    let mut buf = [0u8; 64];
    let len = encode_reserved(&mut buf, 7, &[10, 20], b"done");

    assert_eq!(
        len,
        MessageHeader::ENCODED_LENGTH
            + ReservedEncoder::BLOCK_LENGTH as usize
            + GroupHeader::ENCODED_LENGTH
            + 2 * DECODE_BLOCK_LENGTH
            + U16_HEADER
            + 4
    );

    let decoder = ReservedDecoder::decode(&buf[..len]).expect("decode Reserved");
    assert_eq!(decoder.wrap_(), 7);
    let entries: Vec<u16> = decoder.decode_().map(|entry| entry.end_offset_()).collect();
    assert_eq!(entries, vec![10, 20]);
    // the generated entry end walker still exists next to the renamed getter
    let first = decoder.decode_().next().expect("first entry");
    assert_eq!(
        first.end_offset(),
        MessageHeader::ENCODED_LENGTH
            + ReservedEncoder::BLOCK_LENGTH as usize
            + GroupHeader::ENCODED_LENGTH
            + DECODE_BLOCK_LENGTH
    );
    assert_eq!(decoder.finish_(), b"done");
    assert_eq!(decoder.finish_as_str(), "done");
    assert_eq!(decoder.encoded_length(), len);
}

#[test]
fn test_reserved_names_round_trip_through_reader() {
    let mut buf = [0u8; 64];
    let len = encode_reserved(&mut buf, 9, &[1], b"x");

    let mut reader = ReservedReader::decode(&buf[..len]).expect("read Reserved");
    assert_eq!(reader.wrap_(), 9);
    let entries: Vec<u16> = reader.decode_().map(|entry| entry.end_offset_()).collect();
    assert_eq!(entries, vec![1]);
    assert_eq!(reader.finish_(), b"x");
    // the finaliser keeps its name and still returns the frame length
    assert_eq!(reader.finish(), len);
}

#[test]
fn test_reserved_names_skipped_var_data_still_finishes() {
    let mut buf = [0u8; 64];
    let mut encoder = ReservedEncoder::wrap(&mut buf, 0);
    encoder.set_wrap(1);
    let len = encoder.finish();

    let decoder = ReservedDecoder::decode(&buf[..len]).expect("decode Reserved");
    assert!(decoder.decode_().is_empty());
    assert_eq!(decoder.finish_(), b"");
    assert_eq!(decoder.encoded_length(), len);
}
