//! Encoding/decoding benchmarks.

use ironsbe_core::buffer::{AlignedBuffer, WriteBuffer};

/// Benchmark helper for encoding operations.
pub fn benchmark_encode<F>(iterations: usize, mut encode_fn: F) -> std::time::Duration
where
    F: FnMut(&mut [u8]),
{
    let mut buffer = AlignedBuffer::<1024>::new();
    let start = std::time::Instant::now();

    for _ in 0..iterations {
        encode_fn(buffer.as_mut_slice());
    }

    start.elapsed()
}

/// Benchmark helper for decoding operations.
pub fn benchmark_decode<F, T>(
    iterations: usize,
    data: &[u8],
    mut decode_fn: F,
) -> std::time::Duration
where
    F: FnMut(&[u8]) -> T,
{
    let start = std::time::Instant::now();

    for _ in 0..iterations {
        let _ = decode_fn(data);
    }

    start.elapsed()
}
