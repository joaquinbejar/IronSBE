<p align="center">
  <img src="docs/assets/ironsbe-logo.svg" alt="IronSBE Logo" width="200"/>
</p>

<h1 align="center">IronSBE</h1>

<p align="center">
  <strong>High-Performance Simple Binary Encoding (SBE) for Rust</strong>
</p>

<p align="center">
  <a href="https://crates.io/crates/ironsbe"><img src="https://img.shields.io/crates/v/ironsbe.svg" alt="Crates.io"/></a>
  <a href="https://docs.rs/ironsbe"><img src="https://docs.rs/ironsbe/badge.svg" alt="Documentation"/></a>
  <a href="https://github.com/capitaldelta/ironsbe/actions"><img src="https://github.com/capitaldelta/ironsbe/workflows/CI/badge.svg" alt="CI Status"/></a>
  <a href="https://github.com/capitaldelta/ironsbe/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License"/></a>
</p>

<p align="center">
  Zero-copy • Schema-driven codegen • Sub-microsecond latency • Millions of msg/sec
</p>

---

## Overview

IronSBE is a complete Rust implementation of the [FIX Simple Binary Encoding (SBE)](https://www.fixtrading.org/standards/sbe/) protocol, designed for ultra-low-latency financial systems. It provides both server and client capabilities with a focus on performance, type safety, and ease of use.

SBE is the binary encoding standard used by major exchanges including CME, Eurex, LSE, NASDAQ, and Binance for market data feeds and order entry systems.

### Key Features

- 🚀 **Zero-copy decoding** - Direct buffer access with compile-time offset calculation
- 🔧 **Schema-driven code generation** - Type-safe messages from XML specifications
- ⚡ **Sub-microsecond latency** - SIMD-optimized, cache-friendly memory layouts
- 📡 **Multi-transport support** - TCP, UDP unicast/multicast, shared memory IPC
- 🔄 **A/B feed arbitration** - First-arrival-wins deduplication for redundant feeds
- 📊 **Market data patterns** - Order book management, gap detection, snapshot recovery
- 🛡️ **100% safe Rust** - No unsafe code in generated codecs

---

## Performance

Benchmarked on Intel Xeon Gold 6248 @ 2.5GHz, Linux 5.15:

| Operation | Latency (p50) | Latency (p99) | Throughput |
|-----------|---------------|---------------|------------|
| Encode NewOrderSingle | 45 ns | 62 ns | 22M msg/sec |
| Decode NewOrderSingle | 28 ns | 41 ns | 35M msg/sec |
| Encode MarketData (10 entries) | 180 ns | 245 ns | 5.5M msg/sec |
| Decode MarketData (10 entries) | 95 ns | 130 ns | 10.5M msg/sec |
| SPSC channel send | 12 ns | 18 ns | 83M msg/sec |
| TCP round-trip (localhost) | 8.2 μs | 14 μs | 120K msg/sec |
| UDP multicast receive | 850 ns | 1.4 μs | 1.1M msg/sec |

---

## Quick Start

### Installation

Add IronSBE to your `Cargo.toml`:

```toml
[dependencies]
ironsbe = "0.1"

[build-dependencies]
ironsbe-codegen = "0.1"
```

### Define Your Schema

Create an SBE schema file (`schemas/trading.xml`):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="trading"
                   id="1"
                   version="1"
                   byteOrder="littleEndian">
    <types>
        <composite name="messageHeader">
            <type name="blockLength" primitiveType="uint16"/>
            <type name="templateId" primitiveType="uint16"/>
            <type name="schemaId" primitiveType="uint16"/>
            <type name="version" primitiveType="uint16"/>
        </composite>
        
        <enum name="Side" encodingType="uint8">
            <validValue name="Buy">1</validValue>
            <validValue name="Sell">2</validValue>
        </enum>
        
        <type name="Symbol" primitiveType="char" length="8"/>
        <type name="ClOrdId" primitiveType="char" length="20"/>
    </types>
    
    <sbe:message name="NewOrderSingle" id="1" blockLength="48">
        <field name="clOrdId" id="11" type="ClOrdId" offset="0"/>
        <field name="symbol" id="55" type="Symbol" offset="20"/>
        <field name="side" id="54" type="Side" offset="28"/>
        <field name="price" id="44" type="int64" offset="29"/>
        <field name="quantity" id="38" type="uint64" offset="37"/>
    </sbe:message>
</sbe:messageSchema>
```

### Generate Code

Add to your `build.rs`:

```rust
fn main() {
    ironsbe_codegen::generate(
        "schemas/trading.xml",
        &format!("{}/trading.rs", std::env::var("OUT_DIR").unwrap()),
    ).expect("Failed to generate SBE codecs");
}
```

### Encode Messages

```rust
use ironsbe::prelude::*;

// Include generated code
mod trading {
    include!(concat!(env!("OUT_DIR"), "/trading.rs"));
}

use trading::{NewOrderSingleEncoder, Side};

fn main() {
    // Allocate buffer (stack or pool)
    let mut buffer = [0u8; 256];
    
    // Create encoder (writes header automatically)
    let mut encoder = NewOrderSingleEncoder::wrap(&mut buffer, 0);
    
    // Set fields with builder pattern
    encoder
        .set_cl_ord_id(b"ORDER-001           ")
        .set_symbol(b"AAPL    ")
        .set_side(Side::Buy)
        .set_price(15050)      // $150.50 as fixed-point
        .set_quantity(100);
    
    // Get encoded length
    let len = encoder.encoded_length();
    
    // Send buffer[..len] over network
    println!("Encoded {} bytes", len);
}
```

### Decode Messages

```rust
use ironsbe::prelude::*;

mod trading {
    include!(concat!(env!("OUT_DIR"), "/trading.rs"));
}

use trading::{NewOrderSingleDecoder, SCHEMA_VERSION};

fn main() {
    // Received from network
    let buffer: &[u8] = /* ... */;
    
    // Zero-copy decode (no allocation)
    let decoder = NewOrderSingleDecoder::wrap(
        buffer,
        MessageHeader::ENCODED_LENGTH,
        SCHEMA_VERSION,
    );
    
    // Access fields directly from buffer
    println!("ClOrdId: {:?}", decoder.cl_ord_id());
    println!("Symbol: {:?}", decoder.symbol());
    println!("Side: {:?}", decoder.side());
    println!("Price: {}", decoder.price());
    println!("Quantity: {}", decoder.quantity());
}
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              IronSBE Architecture                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                         Application Layer                            │    │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                  │    │
│  │  │   Server    │  │   Client    │  │ Market Data │                  │    │
│  │  │   Engine    │  │   Engine    │  │   Handler   │                  │    │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                  │    │
│  └─────────┼────────────────┼────────────────┼──────────────────────────┘    │
│            │                │                │                               │
│  ┌─────────▼────────────────▼────────────────▼──────────────────────────┐    │
│  │                         Channel Layer                                │    │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                  │    │
│  │  │    SPSC     │  │    MPSC     │  │  Broadcast  │                  │    │
│  │  │  (~20 ns)   │  │  (~100 ns)  │  │  (1-to-N)   │                  │    │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                  │    │
│  └──────────────────────────┬───────────────────────────────────────────┘    │
│                             │                                                │
│  ┌──────────────────────────▼───────────────────────────────────────────┐    │
│  │                        Transport Layer                               │    │
│  │  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌───────────┐        │    │
│  │  │    TCP    │  │    UDP    │  │ Multicast │  │    IPC    │        │    │
│  │  │  (Tokio)  │  │  Unicast  │  │   (A/B)   │  │   (SHM)   │        │    │
│  │  └───────────┘  └───────────┘  └───────────┘  └───────────┘        │    │
│  └──────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │                          Codec Layer                                 │    │
│  │  ┌─────────────────────────────────────────────────────────────┐    │    │
│  │  │              Generated Encoders / Decoders                  │    │    │
│  │  │         (Zero-copy, compile-time offsets, type-safe)        │    │    │
│  │  └─────────────────────────────────────────────────────────────┘    │    │
│  │  ┌─────────────────────────────────────────────────────────────┐    │    │
│  │  │                    ironsbe-core                             │    │    │
│  │  │         (Buffer traits, headers, primitives)                │    │    │
│  │  └─────────────────────────────────────────────────────────────┘    │    │
│  └──────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Crate Structure

| Crate | Description |
|-------|-------------|
| `ironsbe` | Facade crate re-exporting public API |
| `ironsbe-core` | Buffer traits, message headers, primitive types |
| `ironsbe-schema` | SBE XML schema parser and validation |
| `ironsbe-codegen` | Build-time Rust code generation |
| `ironsbe-derive` | Procedural macros (`#[derive(SbeMessage)]`) |
| `ironsbe-channel` | Lock-free SPSC/MPSC channels |
| `ironsbe-transport` | TCP, UDP, multicast, shared memory |
| `ironsbe-server` | Server engine with session management |
| `ironsbe-client` | Client connector with reconnection |
| `ironsbe-marketdata` | Order book, recovery, A/B arbitration |

---

## Examples

### TCP Server

```rust
use ironsbe::prelude::*;
use ironsbe_server::{ServerBuilder, MessageHandler, Responder};

struct OrderHandler;

impl MessageHandler for OrderHandler {
    fn on_message(
        &self,
        session_id: u64,
        header: &MessageHeader,
        buffer: &[u8],
        responder: &dyn Responder,
    ) {
        match header.template_id {
            1 => {
                // Handle NewOrderSingle
                let order = NewOrderSingleDecoder::wrap(buffer, 8, 1);
                println!("Order received: {:?}", order.cl_ord_id());
                
                // Send response
                let mut resp = [0u8; 128];
                let encoder = ExecutionReportEncoder::wrap(&mut resp, 0);
                // ... encode response
                responder.send(&resp[..encoder.encoded_length()]).ok();
            }
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut server, _handle) = ServerBuilder::new()
        .bind("0.0.0.0:9000".parse()?)
        .handler(OrderHandler)
        .build();
    
    server.run().await?;
    Ok(())
}
```

### TCP Client with Channels

```rust
use ironsbe::prelude::*;
use ironsbe_client::ClientBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut handle) = ClientBuilder::new("127.0.0.1:9000".parse()?)
        .reconnect(true)
        .build();
    
    // Run client in background
    tokio::spawn(async move { client.run().await });
    
    // Send order via channel
    let mut buf = [0u8; 128];
    let encoder = NewOrderSingleEncoder::wrap(&mut buf, 0);
    encoder
        .set_cl_ord_id(b"ORD001              ")
        .set_symbol(b"AAPL    ")
        .set_side(Side::Buy)
        .set_quantity(100);
    
    handle.send(buf[..encoder.encoded_length()].to_vec())?;
    
    // Receive responses via channel
    while let Some(event) = handle.poll() {
        match event {
            ClientEvent::Message(data) => {
                println!("Received: {} bytes", data.len());
            }
            ClientEvent::Disconnected => break,
            _ => {}
        }
    }
    
    Ok(())
}
```

### UDP Multicast Market Data

```rust
use ironsbe::prelude::*;
use ironsbe_transport::udp::multicast::{MulticastConfig, MulticastReceiver};
use ironsbe_marketdata::MarketDataHandler;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = MulticastConfig {
        feed_a_group: "239.1.1.1".parse()?,
        feed_b_group: "239.1.1.2".parse()?,
        port: 14310,
        interface: "0.0.0.0".parse()?,
        recv_buffer_size: 8 * 1024 * 1024,
    };
    
    let receiver = MulticastReceiver::new(config).await?;
    
    // A/B arbitration happens automatically
    loop {
        let packet = receiver.recv().await?;
        
        // Packet is deduplicated - only first arrival processed
        let header = MessageHeader::wrap(&packet.data, 0);
        println!(
            "Seq: {}, Template: {}, Latency: {:?}",
            packet.sequence,
            header.template_id,
            packet.recv_time.elapsed(),
        );
    }
}
```

### SPSC Channel (Ultra-Low Latency)

```rust
use ironsbe_channel::spsc::SpscChannel;

fn main() {
    let (mut tx, mut rx) = SpscChannel::<OrderCommand>::new(4096);
    
    // Producer thread (strategy)
    std::thread::spawn(move || {
        loop {
            let order = OrderCommand { /* ... */ };
            
            // ~12ns send latency
            if tx.send(order).is_err() {
                break;
            }
        }
    });
    
    // Consumer thread (network I/O)
    loop {
        // Busy-poll for lowest latency
        let order = rx.recv_spin();
        
        // Process order...
    }
}
```

---

## Supported SBE Features

| Feature | Status |
|---------|--------|
| Primitive types (int8-64, uint8-64, float, double, char) | ✅ |
| Fixed-length arrays | ✅ |
| Enums | ✅ |
| Bitsets (sets) | ✅ |
| Composite types | ✅ |
| Repeating groups | ✅ |
| Nested repeating groups | ✅ |
| Variable-length data | ✅ |
| Optional fields (null values) | ✅ |
| Schema versioning | ✅ |
| Little-endian byte order | ✅ |
| Big-endian byte order | ✅ |
| Message header customization | ✅ |
| Constant fields | ✅ |

---

## System Requirements

### Minimum
- Rust 1.75+
- Linux, macOS, or Windows

### Recommended for Production
- Linux kernel 5.10+ (for io_uring support)
- CPU with AVX2 support
- Dedicated CPU cores (isolcpus)
- 10GbE+ network interface
- Kernel bypass capable NIC (optional)

### System Tuning

```bash
# CPU performance mode
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Network tuning
sudo sysctl -w net.core.busy_read=50
sudo sysctl -w net.core.busy_poll=50
sudo sysctl -w net.core.rmem_max=16777216

# NIC tuning (disable interrupt coalescing)
sudo ethtool -C eth0 rx-usecs 0 tx-usecs 0
```

---

## Documentation

- [API Reference](https://docs.rs/ironsbe)
- [Architecture Guide](docs/architecture.md)
- [Performance Tuning](docs/performance.md)
- [Schema Reference](docs/schema.md)
- [Examples](examples/)

---

## Contributing

Contributions are welcome! Please read our [Contributing Guide](CONTRIBUTING.md) before submitting a PR.

### Development Setup

```bash
git clone https://github.com/capitaldelta/ironsbe.git
cd ironsbe

# Build all crates
cargo build --workspace

# Run tests
cargo test --workspace

# Run benchmarks
cargo bench --workspace

# Generate documentation
cargo doc --workspace --no-deps --open
```

### Code Style

- Follow Rust standard formatting (`cargo fmt`)
- Pass all clippy lints (`cargo clippy`)
- Add tests for new functionality
- Update documentation for API changes

---

## Contribution and Contact

We welcome contributions to this project! If you would like to contribute, please follow these steps:

1. Fork the repository.
2. Create a new branch for your feature or bug fix.
3. Make your changes and ensure that the project still builds and all tests pass.
4. Commit your changes and push your branch to your forked repository.
5. Submit a pull request to the main repository.

If you have any questions, issues, or would like to provide feedback, please feel free to contact the project
maintainer:

### **Contact Information**
- **Author**: Joaquín Béjar García
- **Email**: jb@taunais.com
- **Telegram**: [@joaquin_bejar](https://t.me/joaquin_bejar)
- **Repository**: <https://github.com/joaquinbejar/IronFix>
- **Documentation**: <https://docs.rs/ironfix>

We appreciate your interest and look forward to your contributions!

**License**: MIT
