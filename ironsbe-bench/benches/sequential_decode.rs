//! Sequential vs random-access decode of a message with a large
//! variable-stride repeating group followed by several var data fields.
//!
//! Random-access accessors position each message-level var data field by
//! walking every preceding `levels` entry on the wire, so reading the eight
//! trailing fields of a `Book` costs eight walks of the group. The
//! sequential `BookReader` walks the group once and reads each var data
//! header once (issue #64).
//!
//! This bench has `harness = false` in `Cargo.toml`: every single decode is
//! timed with `Instant::now()` and recorded in an `hdrhistogram`, so the
//! table reports true per-decode p50 / p99 / p99.9 (tail latency is not
//! diluted by batch averaging) instead of criterion's mean. The cost of the
//! two clock reads (roughly 20 ns on Apple silicon) is included in every
//! sample and affects both paths equally.
//!
//! Run with: `cargo bench -p ironsbe-bench --bench sequential_decode`

use hdrhistogram::Histogram;
use ironsbe_core::decoder::SbeDecoder;
use std::hint::black_box;
use std::time::Instant;

/// Codecs generated at build time from the schema in `build.rs`.
#[allow(dead_code, unused_imports, missing_docs, clippy::all)]
mod book {
    include!(concat!(env!("OUT_DIR"), "/bench_schema.rs"));
}

use book::{BookDecoder, BookEncoder, BookReader};

/// Group sizes to measure: the random-access cost grows with each one.
const LEVEL_COUNTS: [u16; 3] = [16, 64, 256];
/// Decodes run before measuring, per path and size.
const WARMUP_DECODES: usize = 2_000;
/// Individually timed decodes per path and size.
const SAMPLES: usize = 100_000;

/// Var data written after the group; the random-access path re-walks
/// `levels` once per field.
const TRAILING_FIELDS: [&[u8]; 8] = [
    b"AAPL",
    b"ACC-000123",
    b"ORD-20260916-000001",
    b"single pass beats eight walks",
    b"gateway-a",
    b"matcher-1",
    b"bench",
    b"end",
];

/// Encodes a `Book` with `levels` entries and returns its length in bytes.
fn encode_book(buf: &mut [u8], levels: u16) -> usize {
    let mut encoder = BookEncoder::wrap(buf, 0);
    encoder.set_seq(1);
    {
        let mut group = encoder.levels_count(levels);
        for i in 0..levels {
            group
                .next_entry()
                .expect("level entry")
                .set_price(i64::from(i) * 100)
                .set_size(u64::from(i) + 1)
                .set_venue(b"XNAS");
        }
    }
    encoder
        .set_symbol(TRAILING_FIELDS[0])
        .set_account(TRAILING_FIELDS[1])
        .set_cl_ord_id(TRAILING_FIELDS[2])
        .set_text(TRAILING_FIELDS[3])
        .set_source(TRAILING_FIELDS[4])
        .set_target(TRAILING_FIELDS[5])
        .set_tag(TRAILING_FIELDS[6])
        .set_trailer(TRAILING_FIELDS[7]);
    encoder.finish()
}

/// Folds `bytes` into `acc` so the compiler cannot drop the read.
#[inline(always)]
fn fold(acc: u64, bytes: &[u8]) -> u64 {
    acc ^ (bytes.len() as u64) ^ u64::from(bytes.first().copied().unwrap_or(0))
}

/// Reads every field of the frame through the random-access decoder.
#[inline(never)]
fn decode_random_access(frame: &[u8]) -> u64 {
    let decoder = BookDecoder::decode(frame).expect("decode Book");
    let mut acc = decoder.seq();
    for level in decoder.levels() {
        acc ^= level.price() as u64;
        acc ^= level.size();
        acc = fold(acc, level.venue());
    }
    acc = fold(acc, decoder.symbol());
    acc = fold(acc, decoder.account());
    acc = fold(acc, decoder.cl_ord_id());
    acc = fold(acc, decoder.text());
    acc = fold(acc, decoder.source());
    acc = fold(acc, decoder.target());
    acc = fold(acc, decoder.tag());
    acc = fold(acc, decoder.trailer());
    acc ^ decoder.encoded_length() as u64
}

/// Reads every field of the frame through the sequential reader.
#[inline(never)]
fn decode_sequential(frame: &[u8]) -> u64 {
    let mut reader = BookReader::decode(frame).expect("read Book");
    let mut acc = reader.seq();
    {
        let mut levels = reader.levels();
        while let Some(mut level) = levels.next_entry() {
            acc ^= level.price() as u64;
            acc ^= level.size();
            acc = fold(acc, level.venue());
        }
    }
    acc = fold(acc, reader.symbol());
    acc = fold(acc, reader.account());
    acc = fold(acc, reader.cl_ord_id());
    acc = fold(acc, reader.text());
    acc = fold(acc, reader.source());
    acc = fold(acc, reader.target());
    acc = fold(acc, reader.tag());
    acc = fold(acc, reader.trailer());
    acc ^ reader.finish() as u64
}

/// Times each call of `decode` on `frame` and returns the nanoseconds per
/// decode as a histogram.
fn measure(frame: &[u8], decode: fn(&[u8]) -> u64) -> Histogram<u64> {
    for _ in 0..WARMUP_DECODES {
        black_box(decode(black_box(frame)));
    }
    let mut hist =
        Histogram::<u64>::new_with_bounds(1, 1_000_000_000, 3).expect("histogram bounds are valid");
    for _ in 0..SAMPLES {
        let start = Instant::now();
        black_box(decode(black_box(frame)));
        let elapsed = start.elapsed().as_nanos() as u64;
        hist.record(elapsed.max(1)).expect("record");
    }
    hist
}

/// Renders one row of the markdown summary table.
fn render_row(levels: u16, name: &str, hist: &Histogram<u64>) {
    println!(
        "| {levels:>6} | {name:<13} | {:>7} ns | {:>7} ns | {:>7} ns |",
        hist.value_at_quantile(0.50),
        hist.value_at_quantile(0.99),
        hist.value_at_quantile(0.999),
    );
}

fn main() {
    println!(
        "Running sequential_decode ({} trailing var data fields, {} warmup + {} individually timed decodes per path and size)",
        TRAILING_FIELDS.len(),
        WARMUP_DECODES,
        SAMPLES,
    );
    println!();
    println!("| Levels | Path          |        p50 |        p99 |      p99.9 |");
    println!("|--------|---------------|------------|------------|------------|");

    let mut buf = vec![0u8; 16 * 1024];
    for levels in LEVEL_COUNTS {
        let len = encode_book(&mut buf, levels);
        let frame = &buf[..len];
        assert_eq!(
            decode_random_access(frame),
            decode_sequential(frame),
            "both paths must read the same bytes"
        );

        let random = measure(frame, decode_random_access);
        let sequential = measure(frame, decode_sequential);
        render_row(levels, "random-access", &random);
        render_row(levels, "sequential", &sequential);
    }
}
