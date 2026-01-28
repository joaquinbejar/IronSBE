//! Performance benchmark report generator.
//!
//! This example runs performance benchmarks and generates a formatted report
//! similar to the README performance table.
//!
//! Run with: cargo run --example benchmark_report --release
//!
//! For accurate results, run in release mode with CPU isolation if possible.

use ironsbe_channel::{mpsc, spsc};
use ironsbe_core::buffer::{AlignedBuffer, ReadBuffer, WriteBuffer};
use ironsbe_core::header::MessageHeader;
use std::time::{Duration, Instant};

const BATCH_SIZE: usize = 1000;
const NUM_BATCHES: usize = 10_000;
const WARMUP_BATCHES: usize = 100;

/// Simulated NewOrderSingle message (48 bytes block).
const NEW_ORDER_SINGLE_BLOCK_LENGTH: u16 = 48;

/// Simulated MarketData entry size (24 bytes each).
const MARKET_DATA_ENTRY_SIZE: usize = 24;

#[derive(Debug)]
struct BenchmarkResult {
    name: String,
    latency_p50_ns: f64,
    latency_p99_ns: f64,
    throughput: f64,
    throughput_unit: String,
}

impl BenchmarkResult {
    fn format_latency(ns: f64) -> String {
        if ns >= 1000.0 {
            format!("{:.1} μs", ns / 1000.0)
        } else {
            format!("{:.0} ns", ns)
        }
    }

    fn format_throughput(&self) -> String {
        if self.throughput >= 1_000_000.0 {
            format!(
                "{:.1}M {}",
                self.throughput / 1_000_000.0,
                self.throughput_unit
            )
        } else if self.throughput >= 1_000.0 {
            format!("{:.0}K {}", self.throughput / 1_000.0, self.throughput_unit)
        } else {
            format!("{:.0} {}", self.throughput, self.throughput_unit)
        }
    }

    fn print_row(&self) {
        println!(
            "| {:<35} | {:>13} | {:>13} | {:>15} |",
            self.name,
            Self::format_latency(self.latency_p50_ns),
            Self::format_latency(self.latency_p99_ns),
            self.format_throughput()
        );
    }
}

fn calculate_percentiles(mut samples: Vec<f64>) -> (f64, f64) {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = samples.len();
    let p50_idx = len / 2;
    let p99_idx = (len * 99) / 100;
    (samples[p50_idx], samples[p99_idx])
}

/// Benchmark using batch timing for sub-nanosecond resolution.
fn benchmark_batch<F>(name: &str, mut f: F) -> BenchmarkResult
where
    F: FnMut(),
{
    // Warmup
    for _ in 0..WARMUP_BATCHES {
        for _ in 0..BATCH_SIZE {
            f();
        }
    }

    // Collect batch timings
    let mut batch_times_ns = Vec::with_capacity(NUM_BATCHES);
    for _ in 0..NUM_BATCHES {
        let start = Instant::now();
        for _ in 0..BATCH_SIZE {
            f();
        }
        let elapsed_ns = start.elapsed().as_nanos() as f64;
        batch_times_ns.push(elapsed_ns / BATCH_SIZE as f64);
    }

    let (p50, p99) = calculate_percentiles(batch_times_ns);
    let throughput = if p50 > 0.0 {
        1_000_000_000.0 / p50
    } else {
        0.0
    };

    BenchmarkResult {
        name: name.to_string(),
        latency_p50_ns: p50,
        latency_p99_ns: p99,
        throughput,
        throughput_unit: "msg/sec".to_string(),
    }
}

#[inline(always)]
fn encode_new_order_single(buffer: &mut AlignedBuffer<256>) {
    let header = MessageHeader::new(NEW_ORDER_SINGLE_BLOCK_LENGTH, 1, 1, 1);
    header.encode(buffer, 0);

    let offset = MessageHeader::ENCODED_LENGTH;
    buffer.as_mut_slice()[offset..offset + 20].copy_from_slice(b"ORDER-00000000000001");
    buffer.as_mut_slice()[offset + 20..offset + 28].copy_from_slice(b"AAPL    ");
    buffer.put_u8(offset + 28, 1);
    buffer.put_i64_le(offset + 29, 15050);
    buffer.put_u64_le(offset + 37, 100);
}

#[inline(always)]
fn decode_new_order_single(buffer: &AlignedBuffer<256>) -> (MessageHeader, i64, u64) {
    let header = MessageHeader::wrap(buffer, 0);
    let offset = MessageHeader::ENCODED_LENGTH;
    let price = buffer.get_i64_le(offset + 29);
    let quantity = buffer.get_u64_le(offset + 37);
    (header, price, quantity)
}

#[inline(always)]
fn encode_market_data_10(buffer: &mut AlignedBuffer<512>) {
    let block_length = (4 + 10 * MARKET_DATA_ENTRY_SIZE) as u16;
    let header = MessageHeader::new(block_length, 2, 1, 1);
    header.encode(buffer, 0);

    let mut offset = MessageHeader::ENCODED_LENGTH;
    buffer.put_u16_le(offset, 10);
    buffer.put_u16_le(offset + 2, MARKET_DATA_ENTRY_SIZE as u16);
    offset += 4;

    for i in 0..10 {
        buffer.put_i64_le(offset, 10000 + (i as i64) * 10);
        buffer.put_u64_le(offset + 8, 100 + (i as u64) * 10);
        buffer.put_u32_le(offset + 16, 5 + i as u32);
        buffer.put_u8(offset + 20, i as u8);
        buffer.put_u8(offset + 21, if i % 2 == 0 { 1 } else { 2 });
        offset += MARKET_DATA_ENTRY_SIZE;
    }
}

#[inline(always)]
fn decode_market_data_10(buffer: &AlignedBuffer<512>) -> (MessageHeader, u16) {
    let header = MessageHeader::wrap(buffer, 0);
    let offset = MessageHeader::ENCODED_LENGTH;
    let num_entries = buffer.get_u16_le(offset);

    // Read all entries
    let mut _sum: i64 = 0;
    let mut entry_offset = offset + 4;
    for _ in 0..num_entries {
        _sum += buffer.get_i64_le(entry_offset);
        entry_offset += MARKET_DATA_ENTRY_SIZE;
    }

    (header, num_entries)
}

fn run_encode_new_order_single() -> BenchmarkResult {
    let mut buffer = AlignedBuffer::<256>::new();
    benchmark_batch("Encode NewOrderSingle", || {
        encode_new_order_single(std::hint::black_box(&mut buffer));
    })
}

fn run_decode_new_order_single() -> BenchmarkResult {
    let mut buffer = AlignedBuffer::<256>::new();
    encode_new_order_single(&mut buffer);

    benchmark_batch("Decode NewOrderSingle", || {
        std::hint::black_box(decode_new_order_single(std::hint::black_box(&buffer)));
    })
}

fn run_encode_market_data() -> BenchmarkResult {
    let mut buffer = AlignedBuffer::<512>::new();
    benchmark_batch("Encode MarketData (10 entries)", || {
        encode_market_data_10(std::hint::black_box(&mut buffer));
    })
}

fn run_decode_market_data() -> BenchmarkResult {
    let mut buffer = AlignedBuffer::<512>::new();
    encode_market_data_10(&mut buffer);

    benchmark_batch("Decode MarketData (10 entries)", || {
        std::hint::black_box(decode_market_data_10(std::hint::black_box(&buffer)));
    })
}

fn run_spsc_channel() -> BenchmarkResult {
    let (mut tx, mut rx) = spsc::channel::<u64>(4096);

    benchmark_batch("SPSC channel send", || {
        tx.send(std::hint::black_box(42)).unwrap();
        std::hint::black_box(rx.recv().unwrap());
    })
}

fn run_mpsc_channel() -> BenchmarkResult {
    let (tx, rx) = mpsc::channel::<u64>(4096);

    benchmark_batch("MPSC channel send", || {
        tx.send(std::hint::black_box(42)).unwrap();
        std::hint::black_box(rx.recv().unwrap());
    })
}

fn run_tcp_roundtrip() -> BenchmarkResult {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    // Server thread
    let server_handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 64];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    stream.write_all(&buf[..n]).unwrap();
                }
                Err(_) => break,
            }
        }
    });

    // Give server time to start
    thread::sleep(Duration::from_millis(10));

    let mut client = TcpStream::connect(addr).unwrap();
    client.set_nodelay(true).unwrap();

    let message = b"Hello, IronSBE!";
    let mut response = [0u8; 64];

    // Warmup
    for _ in 0..1000 {
        client.write_all(message).unwrap();
        client.read_exact(&mut response[..message.len()]).unwrap();
    }

    // Benchmark (fewer iterations for TCP)
    let tcp_iterations = 10_000;
    let mut samples = Vec::with_capacity(tcp_iterations);

    for _ in 0..tcp_iterations {
        let start = Instant::now();
        client.write_all(message).unwrap();
        client.read_exact(&mut response[..message.len()]).unwrap();
        samples.push(start.elapsed().as_nanos() as f64);
    }

    drop(client);
    server_handle.join().unwrap();

    let (p50, p99) = calculate_percentiles(samples);
    let throughput = 1_000_000_000.0 / p50;

    BenchmarkResult {
        name: "TCP round-trip (localhost)".to_string(),
        latency_p50_ns: p50,
        latency_p99_ns: p99,
        throughput,
        throughput_unit: "msg/sec".to_string(),
    }
}

fn main() {
    println!();
    println!(
        "╔═══════════════════════════════════════════════════════════════════════════════════╗"
    );
    println!(
        "║                         IronSBE Performance Report                                 ║"
    );
    println!(
        "╚═══════════════════════════════════════════════════════════════════════════════════╝"
    );
    println!();

    // System info
    println!("System Information:");
    println!("  - Batch size: {}", BATCH_SIZE);
    println!("  - Num batches: {}", NUM_BATCHES);
    println!("  - Warmup: {} batches", WARMUP_BATCHES);
    #[cfg(target_os = "macos")]
    println!("  - OS: macOS");
    #[cfg(target_os = "linux")]
    println!("  - OS: Linux");
    #[cfg(target_os = "windows")]
    println!("  - OS: Windows");
    println!();

    println!("Running benchmarks...");
    println!();

    let results = vec![
        run_encode_new_order_single(),
        run_decode_new_order_single(),
        run_encode_market_data(),
        run_decode_market_data(),
        run_spsc_channel(),
        run_mpsc_channel(),
        run_tcp_roundtrip(),
    ];

    println!();
    println!(
        "┌─────────────────────────────────────┬───────────────┬───────────────┬─────────────────┐"
    );
    println!(
        "│ Operation                           │ Latency (p50) │ Latency (p99) │ Throughput      │"
    );
    println!(
        "├─────────────────────────────────────┼───────────────┼───────────────┼─────────────────┤"
    );

    for result in &results {
        result.print_row();
    }

    println!(
        "└─────────────────────────────────────┴───────────────┴───────────────┴─────────────────┘"
    );
    println!();

    // Markdown format for README
    println!("## Markdown format for README:");
    println!();
    println!("| Operation | Latency (p50) | Latency (p99) | Throughput |");
    println!("|-----------|---------------|---------------|------------|");
    for result in &results {
        println!(
            "| {} | {} | {} | {} |",
            result.name,
            BenchmarkResult::format_latency(result.latency_p50_ns),
            BenchmarkResult::format_latency(result.latency_p99_ns),
            result.format_throughput()
        );
    }
    println!();
}
