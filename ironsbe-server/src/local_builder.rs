//! Single-threaded server builder for thread-per-core / `!Send` backends.
//!
//! Mirrors [`crate::ServerBuilder`] but is generic over [`LocalTransport`]
//! instead of [`Transport`](ironsbe_transport::Transport).  Use this when
//! the chosen backend (e.g. `tokio-uring` via the `tcp-uring` feature) is
//! single-threaded by construction and its handle types are `!Send`.
//!
//! # Runtime requirements
//!
//! [`LocalServer::run`] must be polled inside a single-threaded reactor
//! that owns a Tokio `LocalSet` (or anything with the same semantics).
//! `tokio_uring::start` provides one for free; for plain `tokio` you can
//! use `tokio::task::LocalSet::run_until`.

use crate::error::ServerError;
use crate::handler::{MessageHandler, Responder, SendError};
use crate::session::SessionManager;
use ironsbe_channel::mpsc::{MpscChannel, MpscReceiver, MpscSender};
use ironsbe_core::header::MessageHeader;
use ironsbe_transport::traits::{LocalConnection, LocalListener, LocalTransport};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc as tokio_mpsc};

use crate::builder::{ServerCommand, ServerEvent, ServerHandle};

/// Builder for [`LocalServer`].
///
/// Single-threaded counterpart of [`crate::ServerBuilder`]; the type
/// parameter `T` selects a [`LocalTransport`] backend rather than the
/// multi-threaded [`Transport`](ironsbe_transport::Transport) family.
pub struct LocalServerBuilder<H, T: LocalTransport> {
    bind_addr: SocketAddr,
    bind_config: Option<T::BindConfig>,
    handler: Option<H>,
    max_connections: usize,
    channel_capacity: usize,
    _transport: PhantomData<T>,
}

impl<H: MessageHandler, T: LocalTransport> LocalServerBuilder<H, T> {
    /// Creates a new local server builder with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bind_addr: "0.0.0.0:9000"
                .parse()
                .expect("hardcoded default bind addr is valid"),
            bind_config: None,
            handler: None,
            max_connections: 1000,
            channel_capacity: 4096,
            _transport: PhantomData,
        }
    }

    /// Sets the bind address.  Clears any previously-supplied
    /// [`bind_config`](Self::bind_config) since the address is now
    /// ambiguous.
    #[must_use]
    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self.bind_config = None;
        self
    }

    /// Supplies a backend-specific bind configuration.
    #[must_use]
    pub fn bind_config(mut self, config: T::BindConfig) -> Self {
        self.bind_config = Some(config);
        self
    }

    /// Sets the message handler.
    #[must_use]
    pub fn handler(mut self, handler: H) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Sets the maximum number of concurrent sessions.
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Sets the cmd/event channel capacity.
    #[must_use]
    pub fn channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }

    /// Builds the server and its external handle.
    ///
    /// # Panics
    /// Panics if no [`handler`](Self::handler) was set.
    #[must_use]
    pub fn build(self) -> (LocalServer<H, T>, ServerHandle) {
        let handler = self.handler.expect("Handler required");
        let (cmd_tx, cmd_rx) = MpscChannel::bounded(self.channel_capacity);
        let (event_tx, event_rx) = MpscChannel::bounded(self.channel_capacity);
        let cmd_notify = Arc::new(Notify::new());

        let server = LocalServer {
            bind_addr: self.bind_addr,
            bind_config: Some(
                self.bind_config
                    .unwrap_or_else(|| T::BindConfig::from(self.bind_addr)),
            ),
            handler: Rc::new(handler),
            max_connections: self.max_connections,
            cmd_rx,
            event_tx,
            sessions: SessionManager::new(),
            cmd_notify: Arc::clone(&cmd_notify),
            _transport: PhantomData,
        };

        let handle = ServerHandle::new(cmd_tx, event_rx, cmd_notify);
        (server, handle)
    }
}

impl<H: MessageHandler, T: LocalTransport> Default for LocalServerBuilder<H, T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Single-threaded server instance for [`LocalTransport`] backends.
///
/// `LocalServer::run` **must** be polled inside a Tokio `LocalSet` (e.g.
/// from inside `tokio_uring::start(async { server.run().await })`).
/// Polling it from a context without a `LocalSet` will fail at the first
/// `spawn_local` call.
#[allow(dead_code)]
pub struct LocalServer<H, T: LocalTransport> {
    bind_addr: SocketAddr,
    bind_config: Option<T::BindConfig>,
    handler: Rc<H>,
    max_connections: usize,
    cmd_rx: MpscReceiver<ServerCommand>,
    event_tx: MpscSender<ServerEvent>,
    sessions: SessionManager,
    cmd_notify: Arc<Notify>,
    _transport: PhantomData<T>,
}

impl<H, T> LocalServer<H, T>
where
    H: MessageHandler + 'static,
    T: LocalTransport,
    T::Connection: 'static,
{
    /// Runs the server, accepting connections and processing messages.
    ///
    /// # Errors
    /// Returns [`ServerError`] if the listener fails to bind or the
    /// accept loop encounters an unrecoverable error.
    ///
    /// # Panics
    /// Panics indirectly via `tokio::task::spawn_local` if called outside
    /// a `LocalSet` context.  See the type-level docs.
    pub async fn run(&mut self) -> Result<(), ServerError> {
        let bind_config = self
            .bind_config
            .take()
            .unwrap_or_else(|| T::BindConfig::from(self.bind_addr));
        let mut listener = T::bind_with(bind_config)
            .await
            .map_err(|e| ServerError::Io(std::io::Error::other(e.to_string())))?;
        let effective_addr = listener.local_addr().unwrap_or(self.bind_addr);
        tracing::info!("Local server listening on {}", effective_addr);

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok(conn) => {
                            let addr = conn
                                .peer_addr()
                                .unwrap_or_else(|_| "0.0.0.0:0".parse().expect("placeholder"));
                            self.handle_connection(conn, addr);
                        }
                        Err(e) => {
                            tracing::error!("Local accept error: {}", e);
                        }
                    }
                }

                _ = self.cmd_notify.notified() => {
                    while let Some(cmd) = self.cmd_rx.try_recv() {
                        if self.handle_command(cmd).await {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    fn handle_connection(&mut self, conn: T::Connection, addr: SocketAddr) {
        if self.sessions.count() >= self.max_connections {
            tracing::warn!("Max connections reached, rejecting {}", addr);
            return;
        }

        let session_id = self.sessions.create_session(addr);
        let handler = Rc::clone(&self.handler);
        let event_tx = self.event_tx.clone();

        handler.on_session_start(session_id);
        let _ = event_tx.try_send(ServerEvent::SessionCreated(session_id, addr));

        // `spawn_local` keeps the future on the current single-threaded
        // runtime, satisfying the `!Send` connection bound.
        tokio::task::spawn_local(async move {
            tracing::info!("Local session {} connected from {}", session_id, addr);
            if let Err(e) = handle_local_session(session_id, conn, handler.as_ref()).await {
                tracing::error!("Local session {} error: {:?}", session_id, e);
            }
            handler.on_session_end(session_id);
            let _ = event_tx.try_send(ServerEvent::SessionClosed(session_id));
        });
    }

    async fn handle_command(&mut self, cmd: ServerCommand) -> bool {
        match cmd {
            ServerCommand::Shutdown => {
                tracing::info!("Local server shutdown requested");
                true
            }
            ServerCommand::CloseSession(session_id) => {
                self.sessions.close_session(session_id);
                false
            }
            ServerCommand::Broadcast(_message) => false,
        }
    }
}

/// Per-session responder that ferries handler outputs back to the
/// connection writer over an unbounded local channel.  Mirrors the
/// equivalent type in [`crate::builder`].
struct LocalSessionResponder {
    tx: tokio_mpsc::UnboundedSender<Vec<u8>>,
}

impl Responder for LocalSessionResponder {
    fn send(&self, message: &[u8]) -> Result<(), SendError> {
        self.tx.send(message.to_vec()).map_err(|_| SendError {
            message: "channel closed".to_string(),
        })
    }

    fn send_to(&self, _session_id: u64, message: &[u8]) -> Result<(), SendError> {
        self.send(message)
    }
}

/// Drives one [`LocalConnection`] end-to-end: read framed SBE messages,
/// dispatch to the handler, and write any responses produced by the
/// handler back over the same connection.
///
/// Mirrors the [`Connection`](ironsbe_transport::traits::Connection)
/// version in [`crate::builder`].
async fn handle_local_session<H, C>(
    session_id: u64,
    mut conn: C,
    handler: &H,
) -> Result<(), std::io::Error>
where
    H: MessageHandler,
    C: LocalConnection,
{
    let (tx, mut rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
    let responder = LocalSessionResponder { tx };

    loop {
        tokio::select! {
            result = conn.recv() => {
                match result {
                    Ok(Some(data)) => {
                        if data.len() >= MessageHeader::ENCODED_LENGTH {
                            let header = MessageHeader::wrap(data.as_ref(), 0);
                            handler.on_message(session_id, &header, data.as_ref(), &responder);
                        } else {
                            handler.on_error(session_id, "Message too short for header");
                        }
                    }
                    Ok(None) => {
                        tracing::info!("Local session {} disconnected", session_id);
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::error!("Local session {} read error: {}", session_id, e);
                        return Err(std::io::Error::other(e.to_string()));
                    }
                }
            }

            Some(msg) = rx.recv() => {
                if let Err(e) = conn.send(&msg).await {
                    tracing::error!("Local session {} write error: {}", session_id, e);
                    return Err(std::io::Error::other(e.to_string()));
                }
            }
        }
    }
}

#[cfg(all(test, feature = "tcp-uring", target_os = "linux"))]
mod tests {
    use super::*;
    use crate::handler::Responder;
    use ironsbe_transport::tcp_uring::UringTcpTransport;

    struct TestHandler;
    impl MessageHandler for TestHandler {
        fn on_message(
            &self,
            _session_id: u64,
            _header: &MessageHeader,
            _data: &[u8],
            _responder: &dyn Responder,
        ) {
        }
    }

    #[test]
    fn test_local_server_builder_new() {
        let builder = LocalServerBuilder::<TestHandler, UringTcpTransport>::new();
        let _ = builder;
    }

    #[test]
    fn test_local_server_builder_default() {
        let builder = LocalServerBuilder::<TestHandler, UringTcpTransport>::default();
        let _ = builder;
    }

    #[test]
    fn test_local_server_builder_bind() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().expect("test addr");
        let builder = LocalServerBuilder::<TestHandler, UringTcpTransport>::new().bind(addr);
        let _ = builder;
    }

    #[test]
    fn test_local_server_builder_max_connections() {
        let builder =
            LocalServerBuilder::<TestHandler, UringTcpTransport>::new().max_connections(500);
        let _ = builder;
    }

    #[test]
    fn test_local_server_builder_channel_capacity() {
        let builder =
            LocalServerBuilder::<TestHandler, UringTcpTransport>::new().channel_capacity(8192);
        let _ = builder;
    }

    #[test]
    fn test_local_server_builder_build() {
        let (_server, _handle) = LocalServerBuilder::<TestHandler, UringTcpTransport>::new()
            .handler(TestHandler)
            .build();
    }
}
