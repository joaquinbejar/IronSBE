//! Comprehensive performance benchmarks for IronSBE.
//!
//! This benchmark suite measures:
//! - Encode/Decode NewOrderSingle (simulated SBE message)
//! - Encode/Decode MarketData with multiple entries
//! - SPSC channel send/recv latency
//! - TCP round-trip latency (localhost)
//!
//! Run with: cargo bench -p ironsbe-bench --bench performance

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ironsbe_channel::spsc;
use ironsbe_core::buffer::{AlignedBuffer, ReadBuffer, WriteBuffer};
use ironsbe_core::header::MessageHeader;
use std::hint::black_box;

/// Simulated NewOrderSingle message structure (48 bytes block).
///
/// Layout:
/// - clOrdId: 20 bytes (char[20])
/// - symbol: 8 bytes (char[8])
/// - side: 1 byte (u8 enum)
/// - price: 8 bytes (i64)
/// - quantity: 8 bytes (u64)
/// - padding: 3 bytes
///
/// Total: 48 bytes
const NEW_ORDER_SINGLE_BLOCK_LENGTH: u16 = 48;
const NEW_ORDER_SINGLE_TEMPLATE_ID: u16 = 1;

/// Simulated MarketData entry (24 bytes each).
///
/// Layout:
/// - price: 8 bytes (i64)
/// - size: 8 bytes (u64)
/// - numOrders: 4 bytes (u32)
/// - level: 1 byte (u8)
/// - side: 1 byte (u8)
/// - padding: 2 bytes
const MARKET_DATA_ENTRY_SIZE: usize = 24;
const MARKET_DATA_TEMPLATE_ID: u16 = 2;

/// Encodes a simulated NewOrderSingle message.
#[inline(always)]
fn encode_new_order_single(buffer: &mut AlignedBuffer<256>) {
    let header = MessageHeader::new(
        NEW_ORDER_SINGLE_BLOCK_LENGTH,
        NEW_ORDER_SINGLE_TEMPLATE_ID,
        1,
        1,
    );
    header.encode(buffer, 0);

    let offset = MessageHeader::ENCODED_LENGTH;

    // clOrdId (20 bytes)
    buffer.as_mut_slice()[offset..offset + 20].copy_from_slice(b"ORDER-00000000000001");

    // symbol (8 bytes)
    buffer.as_mut_slice()[offset + 20..offset + 28].copy_from_slice(b"AAPL    ");

    // side (1 byte)
    buffer.put_u8(offset + 28, 1); // Buy

    // price (8 bytes)
    buffer.put_i64_le(offset + 29, 15050); // $150.50 as fixed-point

    // quantity (8 bytes)
    buffer.put_u64_le(offset + 37, 100);
}

/// Decodes a simulated NewOrderSingle message.
#[inline(always)]
fn decode_new_order_single(
    buffer: &AlignedBuffer<256>,
) -> (MessageHeader, [u8; 20], [u8; 8], u8, i64, u64) {
    let header = MessageHeader::wrap(buffer, 0);
    let offset = MessageHeader::ENCODED_LENGTH;

    let mut cl_ord_id = [0u8; 20];
    cl_ord_id.copy_from_slice(&buffer.as_slice()[offset..offset + 20]);

    let mut symbol = [0u8; 8];
    symbol.copy_from_slice(&buffer.as_slice()[offset + 20..offset + 28]);

    let side = buffer.get_u8(offset + 28);
    let price = buffer.get_i64_le(offset + 29);
    let quantity = buffer.get_u64_le(offset + 37);

    (header, cl_ord_id, symbol, side, price, quantity)
}

/// Encodes a simulated MarketData message with N entries.
#[inline(always)]
fn encode_market_data<const N: usize>(buffer: &mut AlignedBuffer<512>) {
    let block_length = (4 + N * MARKET_DATA_ENTRY_SIZE) as u16; // 4 bytes for group header
    let header = MessageHeader::new(block_length, MARKET_DATA_TEMPLATE_ID, 1, 1);
    header.encode(buffer, 0);

    let mut offset = MessageHeader::ENCODED_LENGTH;

    // Group header: numInGroup (u16) + blockLength (u16)
    buffer.put_u16_le(offset, N as u16);
    buffer.put_u16_le(offset + 2, MARKET_DATA_ENTRY_SIZE as u16);
    offset += 4;

    // Encode N entries
    for i in 0..N {
        // price
        buffer.put_i64_le(offset, 10000 + (i as i64) * 10);
        // size
        buffer.put_u64_le(offset + 8, 100 + (i as u64) * 10);
        // numOrders
        buffer.put_u32_le(offset + 16, 5 + i as u32);
        // level
        buffer.put_u8(offset + 20, i as u8);
        // side
        buffer.put_u8(offset + 21, if i % 2 == 0 { 1 } else { 2 });

        offset += MARKET_DATA_ENTRY_SIZE;
    }
}

/// Market data entry tuple type.
type MarketDataEntry = (i64, u64, u32, u8, u8);

/// Decodes a simulated MarketData message.
#[inline(always)]
fn decode_market_data(buffer: &AlignedBuffer<512>) -> (MessageHeader, u16, Vec<MarketDataEntry>) {
    let header = MessageHeader::wrap(buffer, 0);
    let mut offset = MessageHeader::ENCODED_LENGTH;

    // Group header
    let num_entries = buffer.get_u16_le(offset);
    let _entry_size = buffer.get_u16_le(offset + 2);
    offset += 4;

    let mut entries = Vec::with_capacity(num_entries as usize);
    for _ in 0..num_entries {
        let price = buffer.get_i64_le(offset);
        let size = buffer.get_u64_le(offset + 8);
        let num_orders = buffer.get_u32_le(offset + 16);
        let level = buffer.get_u8(offset + 20);
        let side = buffer.get_u8(offset + 21);

        entries.push((price, size, num_orders, level, side));
        offset += MARKET_DATA_ENTRY_SIZE;
    }

    (header, num_entries, entries)
}

fn benchmark_new_order_single_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("new_order_single");
    group.throughput(Throughput::Elements(1));

    group.bench_function("encode", |b| {
        let mut buffer = AlignedBuffer::<256>::new();
        b.iter(|| {
            encode_new_order_single(black_box(&mut buffer));
        })
    });

    group.finish();
}

fn benchmark_new_order_single_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("new_order_single");
    group.throughput(Throughput::Elements(1));

    let mut buffer = AlignedBuffer::<256>::new();
    encode_new_order_single(&mut buffer);

    group.bench_function("decode", |b| {
        b.iter(|| black_box(decode_new_order_single(black_box(&buffer))))
    });

    group.finish();
}

fn benchmark_market_data_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("market_data");

    for num_entries in [1, 5, 10, 20] {
        group.throughput(Throughput::Elements(num_entries as u64));

        group.bench_with_input(
            BenchmarkId::new("encode", num_entries),
            &num_entries,
            |b, &n| {
                let mut buffer = AlignedBuffer::<512>::new();
                b.iter(|| match n {
                    1 => encode_market_data::<1>(black_box(&mut buffer)),
                    5 => encode_market_data::<5>(black_box(&mut buffer)),
                    10 => encode_market_data::<10>(black_box(&mut buffer)),
                    20 => encode_market_data::<20>(black_box(&mut buffer)),
                    _ => unreachable!(),
                })
            },
        );
    }

    group.finish();
}

fn benchmark_market_data_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("market_data");

    for num_entries in [1, 5, 10, 20] {
        group.throughput(Throughput::Elements(num_entries as u64));

        // Pre-encode the buffer
        let mut buffer = AlignedBuffer::<512>::new();
        match num_entries {
            1 => encode_market_data::<1>(&mut buffer),
            5 => encode_market_data::<5>(&mut buffer),
            10 => encode_market_data::<10>(&mut buffer),
            20 => encode_market_data::<20>(&mut buffer),
            _ => unreachable!(),
        }

        group.bench_with_input(
            BenchmarkId::new("decode", num_entries),
            &buffer,
            |b, buf| b.iter(|| black_box(decode_market_data(black_box(buf)))),
        );
    }

    group.finish();
}

fn benchmark_spsc_channel(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc_channel");
    group.throughput(Throughput::Elements(1));

    // Benchmark send only (producer perspective)
    group.bench_function("send", |b| {
        let (mut tx, mut rx) = spsc::channel::<u64>(4096);

        // Drain in background to prevent blocking
        std::thread::spawn(move || {
            loop {
                let _ = rx.recv();
            }
        });

        b.iter(|| {
            let _ = tx.send(black_box(42));
        })
    });

    // Benchmark send + recv round-trip
    group.bench_function("send_recv", |b| {
        let (mut tx, mut rx) = spsc::channel::<u64>(4096);

        b.iter(|| {
            tx.send(black_box(42)).unwrap();
            black_box(rx.recv().unwrap())
        })
    });

    group.finish();
}

fn benchmark_mpsc_channel(c: &mut Criterion) {
    use ironsbe_channel::mpsc;

    let mut group = c.benchmark_group("mpsc_channel");
    group.throughput(Throughput::Elements(1));

    group.bench_function("send_recv", |b| {
        let (tx, rx) = mpsc::channel::<u64>(4096);

        b.iter(|| {
            tx.send(black_box(42)).unwrap();
            black_box(rx.recv().unwrap())
        })
    });

    group.finish();
}

fn benchmark_header_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_header");
    group.throughput(Throughput::Elements(1));

    group.bench_function("encode", |b| {
        let mut buffer = AlignedBuffer::<64>::new();
        let header = MessageHeader::new(56, 1, 100, 1);

        b.iter(|| {
            header.encode(black_box(&mut buffer), 0);
        })
    });

    group.bench_function("decode", |b| {
        let mut buffer = AlignedBuffer::<64>::new();
        let header = MessageHeader::new(56, 1, 100, 1);
        header.encode(&mut buffer, 0);

        b.iter(|| black_box(MessageHeader::wrap(black_box(&buffer), 0)))
    });

    group.finish();
}

fn benchmark_buffer_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer");
    group.throughput(Throughput::Elements(1));

    let mut buffer = AlignedBuffer::<64>::new();
    buffer.put_u64_le(0, 0x123456789ABCDEF0);

    group.bench_function("read_u64_le", |b| {
        b.iter(|| black_box(buffer.get_u64_le(0)))
    });

    group.bench_function("write_u64_le", |b| {
        b.iter(|| {
            buffer.put_u64_le(0, black_box(0x123456789ABCDEF0));
        })
    });

    group.bench_function("read_u32_le", |b| {
        b.iter(|| black_box(buffer.get_u32_le(0)))
    });

    group.bench_function("write_u32_le", |b| {
        b.iter(|| {
            buffer.put_u32_le(0, black_box(0x12345678));
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_new_order_single_encode,
    benchmark_new_order_single_decode,
    benchmark_market_data_encode,
    benchmark_market_data_decode,
    benchmark_spsc_channel,
    benchmark_mpsc_channel,
    benchmark_header_operations,
    benchmark_buffer_operations,
);
criterion_main!(benches);
