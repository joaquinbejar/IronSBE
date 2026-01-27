//! Encoding benchmarks.

use criterion::{Criterion, criterion_group, criterion_main};
use ironsbe_core::buffer::{AlignedBuffer, ReadBuffer, WriteBuffer};
use ironsbe_core::header::MessageHeader;
use std::hint::black_box;

fn benchmark_header_encode(c: &mut Criterion) {
    let mut buffer = AlignedBuffer::<64>::new();
    let header = MessageHeader::new(56, 1, 100, 1);

    c.bench_function("header_encode", |b| {
        b.iter(|| {
            header.encode(black_box(&mut buffer), 0);
        })
    });
}

fn benchmark_header_decode(c: &mut Criterion) {
    let mut buffer = AlignedBuffer::<64>::new();
    let header = MessageHeader::new(56, 1, 100, 1);
    header.encode(&mut buffer, 0);

    c.bench_function("header_decode", |b| {
        b.iter(|| MessageHeader::wrap(black_box(&buffer), 0))
    });
}

fn benchmark_primitive_writes(c: &mut Criterion) {
    let mut buffer = AlignedBuffer::<64>::new();

    c.bench_function("write_u64_le", |b| {
        b.iter(|| {
            buffer.put_u64_le(0, black_box(0x123456789ABCDEF0));
        })
    });

    c.bench_function("write_u32_le", |b| {
        b.iter(|| {
            buffer.put_u32_le(0, black_box(0x12345678));
        })
    });
}

fn benchmark_primitive_reads(c: &mut Criterion) {
    let mut buffer = AlignedBuffer::<64>::new();
    buffer.put_u64_le(0, 0x123456789ABCDEF0);
    buffer.put_u32_le(8, 0x12345678);

    c.bench_function("read_u64_le", |b| {
        b.iter(|| black_box(buffer.get_u64_le(0)))
    });

    c.bench_function("read_u32_le", |b| {
        b.iter(|| black_box(buffer.get_u32_le(8)))
    });
}

criterion_group!(
    benches,
    benchmark_header_encode,
    benchmark_header_decode,
    benchmark_primitive_writes,
    benchmark_primitive_reads,
);
criterion_main!(benches);
