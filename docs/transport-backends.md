# Transport Backends

IronSBE uses a pluggable transport architecture. The `ironsbe-transport` crate
defines two parallel trait families:

- **`Transport` / `Listener` / `Connection`** — multi-threaded backends
  (`Send + Sync`).  Used by `tcp-tokio` and any future backend whose handle
  types are safe to share across threads.
- **`LocalTransport` / `LocalListener` / `LocalConnection`** — single-threaded
  thread-per-core backends (`!Send`).  Used by `tcp-uring` (and any future
  `monoio`/`io_uring`-style backend whose handle types contain `Rc<...>`).

Both families share the same `BindConfig` / `ConnectConfig` plumbing and the
same wire format (4-byte little-endian length prefix), so a server using one
family can talk to a client using the other.

Concrete backends are selected at compile time via Cargo feature flags.

## Default backend: `tcp-tokio`

The `tcp-tokio` feature (enabled by default) provides a Tokio-based TCP
transport with length-prefixed framing (`SbeFrameCodec`).

| Type                  | Trait it implements |
|-----------------------|---------------------|
| `TokioTcpTransport`  | `Transport`         |
| `TcpServer`           | `Listener`          |
| `TcpConnection`       | `Connection`        |

When the feature is active, `ironsbe_transport::DefaultTransport` is aliased to
`TokioTcpTransport`, and both `ServerBuilder` and `ClientBuilder` default their
generic parameter `T` to that type.

```rust
// These two are equivalent when tcp-tokio is enabled:
let (server, handle) = ServerBuilder::<MyHandler>::with_default_transport()
    .bind(addr)
    .handler(handler)
    .build();

let (server, handle) = ServerBuilder::<MyHandler, TokioTcpTransport>::new()
    .bind(addr)
    .handler(handler)
    .build();
```

## Trait overview

### `Transport`

Top-level factory for creating listeners and connections.

```rust
pub trait Transport: Send + Sync + 'static {
    type Listener: Listener;
    type Connection: Connection;
    type Error: std::error::Error + Send + Sync + 'static;
    type BindConfig: From<SocketAddr> + Clone + Send + Sync + 'static;
    type ConnectConfig: From<SocketAddr> + Clone + Send + Sync + 'static;

    // Backends must implement these.
    fn bind_with(config: Self::BindConfig)    -> impl Future<Output = Result<Self::Listener,   Self::Error>> + Send;
    fn connect_with(config: Self::ConnectConfig) -> impl Future<Output = Result<Self::Connection, Self::Error>> + Send;

    // Provided defaults: build a config from the address only.
    fn bind(addr: SocketAddr)    -> impl Future<Output = Result<Self::Listener,   Self::Error>> + Send { /* ... */ }
    fn connect(addr: SocketAddr) -> impl Future<Output = Result<Self::Connection, Self::Error>> + Send { /* ... */ }
}
```

#### Backend tunables

`BindConfig` and `ConnectConfig` are how each backend exposes its own tunables
(frame size, NODELAY, socket buffers, queue depth, …) without leaking concrete
types into upstream generic code.  For the Tokio TCP backend they are
[`TcpServerConfig`] and [`TcpClientConfig`] respectively.

`ServerBuilder::bind_config(cfg)` and `ClientBuilder::connect_config(cfg)` let
callers supply a fully-constructed config.  When the `tcp-tokio` feature is
enabled, both builders also expose a `max_frame_size(usize)` shortcut so the
common case does not require importing the backend's config types.

```rust
let (server, _) = ServerBuilder::<MyHandler>::with_default_transport()
    .bind("0.0.0.0:9000".parse()?)
    .max_frame_size(256 * 1024) // raise above the 64 KiB default
    .handler(handler)
    .build();
```

#### Socket buffer sizes (`SO_RCVBUF` / `SO_SNDBUF`)

Both `TcpServerConfig` and `TcpClientConfig` expose
`recv_buffer_size: Option<usize>` / `send_buffer_size: Option<usize>` (default
`Some(256 KiB)`).  When set, the values are applied to every accepted /
connected socket via separate `setsockopt(SO_RCVBUF)` /
`setsockopt(SO_SNDBUF)` calls using the
[`socket2`](https://crates.io/crates/socket2) crate.

Caveats:

- The kernel may **clamp** the requested value to a system-wide ceiling
  (`/proc/sys/net/core/rmem_max` / `wmem_max` on Linux).
- Linux **doubles** the requested value internally and reports the doubled
  value via `getsockopt`.  macOS/BSD return the value as-set.
- Set both values **before** any heavy I/O begins; changing them on a live
  socket has no effect on already-queued data.

```rust
let server_cfg = TcpServerConfig::new(addr)
    .max_frame_size(256 * 1024)
    .recv_buffer_size(1024 * 1024)
    .send_buffer_size(1024 * 1024);
```

### `Listener`

Accepts incoming connections (server side).

```rust
pub trait Listener: Send + 'static {
    type Connection: Connection;
    type Error: std::error::Error + Send + Sync + 'static;

    fn accept(&mut self) -> impl Future<Output = Result<Self::Connection, Self::Error>> + Send + '_;
    fn local_addr(&self) -> std::io::Result<SocketAddr>;
}
```

### `Connection`

Framed send/recv over a single connection. The connection handles framing
internally — `recv()` returns complete SBE messages and `send()` accepts raw
message bytes and performs length-prefixing.

```rust
pub trait Connection: Send + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn recv(&mut self) -> impl Future<Output = Result<Option<BytesMut>, Self::Error>> + Send + '_;
    fn send<'a>(&'a mut self, msg: &'a [u8]) -> impl Future<Output = Result<(), Self::Error>> + Send + 'a;
    /// Owned-buffer send.  Default impl forwards to `send`; backends with
    /// zero-copy submission (io_uring, RDMA) override this to keep the
    /// `Bytes` alive across the operation.
    fn send_owned(&mut self, msg: Bytes) -> impl Future<Output = Result<(), Self::Error>> + Send + '_;
    fn peer_addr(&self) -> std::io::Result<SocketAddr>;
}
```

## Linux io_uring backend: `tcp-uring`

The `tcp-uring` feature provides an io_uring-based TCP backend built on
[`tokio-uring`](https://crates.io/crates/tokio-uring).  It is **Linux-only**:
on any other platform the feature flag compiles to a no-op so workspace
builds with `--all-features` continue to work.

| Type                  | Trait it implements |
|-----------------------|---------------------|
| `UringTcpTransport`   | `LocalTransport`    |
| `UringListener`       | `LocalListener`     |
| `UringConnection`     | `LocalConnection`   |

### Why a separate trait family

`tokio-uring`'s handle types (`TcpListener`, `TcpStream`) contain `Rc<...>`
internally because the runtime is single-threaded by design.  They are
therefore `!Send` and `!Sync`, and cannot satisfy the `Send + Sync` bounds
of the multi-threaded `Transport` family.  Rather than relax those bounds
(and break the `tcp-tokio` server), we expose a parallel
`LocalTransport` / `LocalListener` / `LocalConnection` family with the
`Send` bounds dropped.  Server integration code chooses which family to
drive based on the backend it was compiled with.

### Requirements

- Linux kernel **≥ 5.10**.  Older kernels lack the syscalls used by
  `tokio-uring 0.5`.
- All transport operations must run inside a [`tokio_uring::start`] block:

  ```rust
  fn main() -> std::io::Result<()> {
      tokio_uring::start(async {
          let addr = "0.0.0.0:9000".parse().expect("valid addr");
          let listener = UringTcpTransport::bind_with(
              UringServerConfig::new(addr)
          ).await?;
          // ...
          Ok::<(), std::io::Error>(())
      })?;
      Ok(())
  }
  ```

### Zero-copy `send_owned`

The `Connection` and `LocalConnection` traits both expose
`send_owned(Bytes)` with a default implementation that borrows the
provided `Bytes` and forwards to `send`.  The io_uring backend overrides
`send_owned` to keep the `Bytes` alive across the SQE submission, so
the kernel can continue referencing that buffer directly for the
operation.

### Wire format

The io_uring backend uses the same 4-byte little-endian length-prefix
framing as `tcp-tokio`, so the two backends are wire-compatible.

### Scope

This module ships the trait-level integration only.  Server integration
(`ironsbe-server` running on a `tokio-uring` runtime), examples, and
benchmark numbers are tracked in the follow-up issue.  Buffer pooling,
registered buffers, registered fds and `IORING_OP_SEND_ZC` are also
deferred to the same follow-up.

### Building

```sh
# On Linux:
cargo build -p ironsbe-transport --no-default-features --features tcp-uring

# On macOS / Windows:
# the feature flag is accepted but the module is gated out via cfg.
cargo build -p ironsbe-transport --features tcp-uring
```

## Adding a new backend

1. **Create a module** under `ironsbe-transport/src/` (e.g., `io_uring/`).
2. **Implement the three traits** for your types.
3. **Gate the module** behind a new Cargo feature in
   `ironsbe-transport/Cargo.toml`:
   ```toml
   [features]
   default = ["tcp-tokio"]
   tcp-tokio   = ["dep:tokio-util", "dep:futures"]
   io-uring    = ["dep:tokio-uring"]  # example
   ```
4. **Conditionally export** in `ironsbe-transport/src/lib.rs`:
   ```rust
   #[cfg(feature = "io-uring")]
   pub mod io_uring;
   ```
5. **Optionally update `DefaultTransport`** if the new backend should be the
   default when its feature is active (use `cfg` priority).
6. **Forward the feature** in `ironsbe-server/Cargo.toml` and
   `ironsbe-client/Cargo.toml`:
   ```toml
   [features]
   io-uring = ["ironsbe-transport/io-uring"]
   ```

No changes to `ironsbe-server` or `ironsbe-client` source code are required —
they are already generic over `T: Transport`.

## Building without a backend

```sh
cargo check -p ironsbe-transport --no-default-features
cargo check -p ironsbe-server    --no-default-features
cargo check -p ironsbe-client    --no-default-features
```

This compiles the trait definitions, error types, and non-TCP modules (UDP, IPC)
but excludes all TCP code. Useful for environments that only need the trait
interface (e.g., a test-double crate, or a downstream crate that plugs in its
own backend).

### Default type parameter gating

`ServerBuilder<H, T>` and `ClientBuilder<T>` provide `T = DefaultTransport` as a
default **only** when the `tcp-tokio` feature is enabled.  With the feature
disabled the default is removed and `T` must be specified explicitly:

```rust
// With tcp-tokio (default): T inferred as DefaultTransport.
let (server, _) = ServerBuilder::<MyHandler>::new().handler(h).build();

// Without tcp-tokio: T must be supplied.
let (server, _) = ServerBuilder::<MyHandler, MyCustomTransport>::new()
    .handler(h)
    .build();
```

This is intentional: if no backend feature is on, falling back to a stub
`DefaultTransport` would silently fail at runtime instead of at compile time.
We prefer the loud error.
