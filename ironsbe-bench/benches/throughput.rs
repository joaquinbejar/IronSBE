//! Throughput benchmarks.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ironsbe_channel::spsc;
use std::hint::black_box;

fn benchmark_spsc_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc_channel");
    group.throughput(Throughput::Elements(1));

    group.bench_function("send_recv", |b| {
        let (mut tx, mut rx) = spsc::channel::<u64>(1024);

        b.iter(|| {
            tx.send(black_box(42)).unwrap();
            black_box(rx.recv().unwrap())
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_spsc_throughput);
criterion_main!(benches);
