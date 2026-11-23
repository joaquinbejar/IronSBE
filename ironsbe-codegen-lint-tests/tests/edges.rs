//! Review edges of the lint-clean output (IronSBE#80). Both schemas are
//! compiled under this crate's strict bar by `src/lib.rs`, which is the
//! check that `edges` imports nothing it does not use.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use ironsbe_codegen_lint_tests::{edges, metadata_names};
use ironsbe_core::decoder::SbeDecoder;

#[test]
fn a_fixed_only_message_named_like_a_header_round_trips() {
    let mut buf = [0u8; 64];
    let mut snapshot = edges::GroupHeaderSnapshotEncoder::wrap(&mut buf, 0).expect("wrap");
    snapshot.set_sequence(42);
    let len = snapshot.encoded_length();
    let decoder = edges::GroupHeaderSnapshotDecoder::decode(&buf[..len]).expect("decode");
    assert_eq!(decoder.sequence(), 42);
}

#[test]
fn group_fields_named_like_entry_metadata_read_their_own_values() {
    let mut buf = [0u8; 128];
    let mut batch = metadata_names::BatchEncoder::wrap(&mut buf, 0).expect("wrap");
    batch.set_sequence(1);
    {
        let mut items = batch.items_count(2).expect("items");
        for (version, length) in [(7u16, 9u16), (11, 13)] {
            items
                .next_entry()
                .expect("entry fits")
                .expect("entry")
                .set_acting_version(version)
                .set_acting_block_length(length);
        }
    }
    let len = batch.encoded_length();
    let decoder = metadata_names::BatchDecoder::decode(&buf[..len]).expect("decode");
    let read: Vec<(u16, u16)> = decoder
        .items()
        .expect("items")
        .map(|entry| entry.expect("entry"))
        .map(|entry| (entry.acting_version(), entry.acting_block_length()))
        .collect();
    assert_eq!(read, [(7, 9), (11, 13)]);
}
