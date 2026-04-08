# Transport Backends

IronSBE uses a pluggable transport architecture. The `ironsbe-transport` crate
defines three traits — `Transport`, `Listener`, and `Connection` — that abstract
over the underlying network implementation. Concrete backends are selected at
compile time via Cargo feature flags.

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

    fn bind(addr: SocketAddr) -> impl Future<Output = Result<Self::Listener, Self::Error>> + Send;
    fn connect(addr: SocketAddr) -> impl Future<Output = Result<Self::Connection, Self::Error>> + Send;
}
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
    fn peer_addr(&self) -> std::io::Result<SocketAddr>;
}
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
```

This compiles the trait definitions, error types, and non-TCP modules (UDP, IPC)
but excludes all TCP code. Useful for environments that only need the trait
interface (e.g., a test-double crate).
