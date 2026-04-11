//! Server builder and main server implementation.

use crate::error::ServerError;
use crate::handler::{MessageHandler, Responder, SendError};
use crate::session::SessionManager;
use ironsbe_channel::mpsc::{MpscChannel, MpscReceiver, MpscSender};
use ironsbe_core::header::MessageHeader;
use ironsbe_transport::traits::{Connection, Listener, Transport};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc as tokio_mpsc};
use tokio_util::sync::CancellationToken;

/// Shared per-session outbound-sender registry.  Populated in
/// [`Server::handle_connection`], drained in
/// [`Server::handle_command`] on `CloseSession` / `Shutdown`, and
/// cloned into every [`SessionResponder`] so `send_to` can resolve
/// the target against the live session table.  See #40, #41.
type SessionSenderMap = Arc<RwLock<HashMap<u64, tokio_mpsc::UnboundedSender<Vec<u8>>>>>;

/// Builder for configuring and creating a server.
///
/// The type parameter `T` selects the transport backend.  When the
/// `tcp-tokio` feature is enabled (the default), `T` defaults to
/// [`ironsbe_transport::DefaultTransport`] so existing call-sites compile
/// without changes.  With the feature disabled, `T` must be specified
/// explicitly so downstream crates can plug in a custom backend.
#[cfg(feature = "tcp-tokio")]
pub struct ServerBuilder<H, T: Transport = ironsbe_transport::DefaultTransport> {
    bind_addr: SocketAddr,
    bind_config: Option<T::BindConfig>,
    handler: Option<H>,
    max_connections: usize,
    channel_capacity: usize,
    _transport: PhantomData<T>,
}

/// Builder for configuring and creating a server.
///
/// With the `tcp-tokio` feature disabled, the transport backend must be
/// specified explicitly via the `T` type parameter.
#[cfg(not(feature = "tcp-tokio"))]
pub struct ServerBuilder<H, T: Transport> {
    bind_addr: SocketAddr,
    bind_config: Option<T::BindConfig>,
    handler: Option<H>,
    max_connections: usize,
    channel_capacity: usize,
    _transport: PhantomData<T>,
}

impl<H: MessageHandler, T: Transport> ServerBuilder<H, T> {
    /// Creates a new server builder with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bind_addr: "0.0.0.0:9000".parse().unwrap(),
            bind_config: None,
            handler: None,
            max_connections: 1000,
            channel_capacity: 4096,
            _transport: PhantomData,
        }
    }

    /// Sets the bind address.
    ///
    /// If a [`bind_config`](Self::bind_config) was previously supplied it
    /// will be cleared, since the address is now ambiguous.  Set the address
    /// first, then customize via `bind_config`.
    #[must_use]
    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self.bind_config = None;
        self
    }

    /// Supplies a backend-specific bind configuration.
    ///
    /// Use this to override transport tunables (frame size, NODELAY, socket
    /// buffer sizes, …).  When unset, the backend builds a default config
    /// from the bind address.
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

    /// Sets the maximum number of connections.
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Sets the channel capacity.
    #[must_use]
    pub fn channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }

    /// Builds the server and handle.
    ///
    /// # Panics
    /// Panics if no handler was set.
    #[must_use]
    pub fn build(self) -> (Server<H, T>, ServerHandle) {
        let handler = self.handler.expect("Handler required");
        let (cmd_tx, cmd_rx) = MpscChannel::bounded(self.channel_capacity);
        let (event_tx, event_rx) = MpscChannel::bounded(self.channel_capacity);

        let cmd_notify = Arc::new(Notify::new());

        let server = Server {
            bind_addr: self.bind_addr,
            bind_config: Some(
                self.bind_config
                    .unwrap_or_else(|| T::BindConfig::from(self.bind_addr)),
            ),
            handler: Arc::new(handler),
            max_connections: self.max_connections,
            cmd_tx: cmd_tx.clone(),
            cmd_rx,
            event_tx,
            sessions: SessionManager::new(),
            cmd_notify: Arc::clone(&cmd_notify),
            shutdown_token: CancellationToken::new(),
            session_tokens: HashMap::new(),
            session_senders: Arc::new(RwLock::new(HashMap::new())),
            _transport: PhantomData,
        };

        let handle = ServerHandle {
            cmd_tx,
            event_rx,
            cmd_notify,
        };

        (server, handle)
    }
}

impl<H: MessageHandler, T: Transport> Default for ServerBuilder<H, T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "tcp-tokio")]
impl<H: MessageHandler> ServerBuilder<H> {
    /// Creates a new server builder using the default transport backend.
    ///
    /// This is a convenience constructor that resolves the transport type
    /// parameter to [`ironsbe_transport::DefaultTransport`], keeping existing
    /// call-sites like `ServerBuilder::new().handler(h).build()` working
    /// without turbofish syntax.
    #[must_use]
    pub fn with_default_transport() -> Self {
        <Self as Default>::default()
    }

    /// Sets the maximum SBE frame size in bytes (Tokio TCP backend only).
    ///
    /// Convenience shortcut that mutates the underlying
    /// [`ironsbe_transport::tcp::TcpServerConfig`] without requiring callers
    /// to construct it manually.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        let cfg = self
            .bind_config
            .take()
            .unwrap_or_else(|| ironsbe_transport::tcp::TcpServerConfig::new(self.bind_addr));
        self.bind_config = Some(cfg.max_frame_size(size));
        self
    }
}

/// The main server instance.
///
/// Generic over handler `H` and transport backend `T`.
#[cfg(feature = "tcp-tokio")]
#[allow(dead_code)]
pub struct Server<H, T: Transport = ironsbe_transport::DefaultTransport> {
    bind_addr: SocketAddr,
    bind_config: Option<T::BindConfig>,
    handler: Arc<H>,
    max_connections: usize,
    /// Cloned and handed to per-session tasks so they can fire
    /// `ServerCommand::CloseSession` when the session ends, freeing the
    /// `SessionManager` slot back in the run loop.  Without this the
    /// slot leaks and `max_connections` eventually rejects every new
    /// connection.
    cmd_tx: MpscSender<ServerCommand>,
    cmd_rx: MpscReceiver<ServerCommand>,
    event_tx: MpscSender<ServerEvent>,
    sessions: SessionManager,
    cmd_notify: Arc<Notify>,
    /// Parent cancellation token. `cancel()` fans out to every live
    /// child token in `session_tokens`, so `ServerCommand::Shutdown`
    /// triggers cooperative tear-down on every spawned session task.
    shutdown_token: CancellationToken,
    /// Per-session child tokens. Inserted in `handle_connection`,
    /// removed (and cancelled) in `handle_command(CloseSession)` or
    /// cleared on `Shutdown`. No lock is needed: only the
    /// single-threaded run loop touches this map.
    session_tokens: HashMap<u64, CancellationToken>,
    /// Live per-session outbound channels, shared with every
    /// [`SessionResponder`] so `send_to(target, msg)` can resolve
    /// `target` against the live table and `ServerCommand::Broadcast`
    /// can iterate.  See #40, #41.
    session_senders: SessionSenderMap,
    _transport: PhantomData<T>,
}

/// The main server instance.
///
/// Generic over handler `H` and transport backend `T`.
#[cfg(not(feature = "tcp-tokio"))]
#[allow(dead_code)]
pub struct Server<H, T: Transport> {
    bind_addr: SocketAddr,
    bind_config: Option<T::BindConfig>,
    handler: Arc<H>,
    max_connections: usize,
    /// See the field with the same name on the `tcp-tokio` variant.
    cmd_tx: MpscSender<ServerCommand>,
    cmd_rx: MpscReceiver<ServerCommand>,
    event_tx: MpscSender<ServerEvent>,
    sessions: SessionManager,
    cmd_notify: Arc<Notify>,
    /// See the field with the same name on the `tcp-tokio` variant.
    shutdown_token: CancellationToken,
    /// See the field with the same name on the `tcp-tokio` variant.
    session_tokens: HashMap<u64, CancellationToken>,
    /// See the field with the same name on the `tcp-tokio` variant.
    session_senders: SessionSenderMap,
    _transport: PhantomData<T>,
}

impl<H, T> Server<H, T>
where
    H: MessageHandler + Send + Sync + 'static,
    T: Transport,
    T::Connection: Send + 'static,
{
    /// Runs the server, accepting connections and processing messages.
    ///
    /// Uses the selected [`Transport`] backend to bind and accept connections.
    ///
    /// # Errors
    /// Returns `ServerError` if the server fails to start or encounters an error.
    pub async fn run(&mut self) -> Result<(), ServerError> {
        let bind_config = self
            .bind_config
            .take()
            .unwrap_or_else(|| T::BindConfig::from(self.bind_addr));
        let mut listener = T::bind_with(bind_config)
            .await
            .map_err(|e| ServerError::Io(std::io::Error::other(e)))?;
        let effective_addr = listener.local_addr().unwrap_or(self.bind_addr);
        tracing::info!("Server listening on {}", effective_addr);
        // Notify any external observer (test harness, supervisor) of
        // the effective bound address.  Mirrors the LocalServer path.
        let _ = self
            .event_tx
            .try_send(ServerEvent::Listening(effective_addr));

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok(conn) => {
                            let addr = conn.peer_addr().unwrap_or_else(
                                |_| "0.0.0.0:0".parse().unwrap()
                            );
                            self.handle_connection(conn, addr).await;
                        }
                        Err(e) => {
                            tracing::error!("Accept error: {}", e);
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

    async fn handle_connection(&mut self, conn: T::Connection, addr: SocketAddr) {
        if self.sessions.count() >= self.max_connections {
            tracing::warn!("Max connections reached, rejecting {}", addr);
            return;
        }

        let session_id = self.sessions.create_session(addr);
        let handler = Arc::clone(&self.handler);
        let event_tx = self.event_tx.clone();
        // Cloned cmd_tx so the spawned task can fire CloseSession back
        // to the run loop on disconnect, releasing the SessionManager
        // slot.  Without this the slot leaks and `max_connections`
        // eventually rejects every new connection.
        let cmd_tx = self.cmd_tx.clone();
        let cmd_notify = Arc::clone(&self.cmd_notify);

        // Per-session cancellation token, derived from the parent
        // shutdown token so `Shutdown` cancels every active session at
        // once and `CloseSession(id)` cancels exactly one.  See #42.
        let session_token = self.shutdown_token.child_token();
        self.session_tokens
            .insert(session_id, session_token.clone());

        // Per-session outbound channel.  The sender is registered in
        // `session_senders` (so `Broadcast` and cross-session
        // `send_to` can find it) and also moved into the spawned
        // task's `SessionResponder`, which uses it as its fast-path
        // `send()` local sender.  See #40, #41.
        let (out_tx, out_rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        self.session_senders
            .write()
            .insert(session_id, out_tx.clone());
        let senders = Arc::clone(&self.session_senders);

        handler.on_session_start(session_id);
        let _ = event_tx.try_send(ServerEvent::SessionCreated(session_id, addr));

        // Spawn connection handler task.  The span gives every log
        // line inside the session the `sbe_session{session_id=N}:`
        // prefix so operators can correlate messages per peer.
        let span = tracing::info_span!("sbe_session", session_id, %addr);
        tokio::spawn(async move {
            let _guard = span.enter();
            tracing::info!("connected");

            if let Err(e) = handle_session(
                session_id,
                conn,
                handler.as_ref(),
                session_token,
                out_tx,
                out_rx,
                senders,
            )
            .await
            {
                tracing::error!(error = %e, "session error");
            }

            tracing::info!("disconnected");
            handler.on_session_end(session_id);
            let _ = event_tx.try_send(ServerEvent::SessionClosed(session_id));
            let _ = cmd_tx.try_send(ServerCommand::CloseSession(session_id));
            cmd_notify.notify_one();
        });
    }

    async fn handle_command(&mut self, cmd: ServerCommand) -> bool {
        match cmd {
            ServerCommand::Shutdown => {
                tracing::info!("Server shutdown requested");
                // Cancel the parent token, which fans out to every
                // live child token in `session_tokens`. Each spawned
                // session task will wake from its `select!`, drop the
                // connection, and run its `on_session_end` cleanup.
                self.shutdown_token.cancel();
                self.session_tokens.clear();
                self.session_senders.write().clear();
                true
            }
            ServerCommand::CloseSession(session_id) => {
                // External `close_session` cancels the matching child
                // token so the spawned task tears the connection down.
                // Idempotent: a second CloseSession (e.g. the spawned
                // task's own cleanup signal) finds the entry already
                // gone and is a no-op.
                if let Some(token) = self.session_tokens.remove(&session_id) {
                    token.cancel();
                }
                self.session_senders.write().remove(&session_id);
                self.sessions.close_session(session_id);
                false
            }
            ServerCommand::Broadcast(message) => {
                // Push the bytes to every live session.  Any entry
                // whose channel is already closed (a session that has
                // exited but hasn't yet fired its own CloseSession
                // cleanup back to the run loop) is opportunistically
                // dropped from the registry via `retain`.  See #40.
                self.session_senders
                    .write()
                    .retain(|_, sender| sender.send(message.clone()).is_ok());
                false
            }
        }
    }
}

/// Handle for controlling the server from outside.
pub struct ServerHandle {
    cmd_tx: MpscSender<ServerCommand>,
    event_rx: MpscReceiver<ServerEvent>,
    cmd_notify: Arc<Notify>,
}

impl ServerHandle {
    /// Constructs a [`ServerHandle`] from its raw plumbing.
    ///
    /// Used internally by the multi-threaded [`Server`] builder and by
    /// the single-threaded `LocalServer` builder so both server flavours
    /// can hand back the same handle type.
    pub(crate) fn new(
        cmd_tx: MpscSender<ServerCommand>,
        event_rx: MpscReceiver<ServerEvent>,
        cmd_notify: Arc<Notify>,
    ) -> Self {
        Self {
            cmd_tx,
            event_rx,
            cmd_notify,
        }
    }

    /// Requests server shutdown.
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.try_send(ServerCommand::Shutdown);
        self.cmd_notify.notify_one();
    }

    /// Closes a specific session.
    pub fn close_session(&self, session_id: u64) {
        let _ = self
            .cmd_tx
            .try_send(ServerCommand::CloseSession(session_id));
        self.cmd_notify.notify_one();
    }

    /// Broadcasts a message to all sessions.
    pub fn broadcast(&self, message: Vec<u8>) {
        let _ = self.cmd_tx.try_send(ServerCommand::Broadcast(message));
        self.cmd_notify.notify_one();
    }

    /// Polls for server events.
    pub fn poll_events(&self) -> impl Iterator<Item = ServerEvent> + '_ {
        std::iter::from_fn(|| self.event_rx.try_recv())
    }
}

/// Commands that can be sent to the server.
#[derive(Debug, Clone)]
pub enum ServerCommand {
    /// Shutdown the server.
    Shutdown,
    /// Close a specific session.
    CloseSession(u64),
    /// Broadcast a message to all sessions.
    Broadcast(Vec<u8>),
}

/// Events emitted by the server.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// The server has bound its listener and is ready to accept
    /// connections.  Carries the *effective* local address (useful when
    /// the caller bound to port 0).
    Listening(SocketAddr),
    /// A new session was created.
    SessionCreated(u64, SocketAddr),
    /// A session was closed.
    SessionClosed(u64),
    /// An error occurred.
    Error(String),
}

/// Session responder that sends messages back to the client.
///
/// Holds two channel references:
///
/// - `tx` is the fast path used by [`Responder::send`] — the
///   responder's own session's sender, so the common case is a
///   single channel push with no map lookup and no locking.
/// - `senders` is a clone of the shared per-session sender table on
///   [`Server`], used by [`Responder::send_to`] to resolve the
///   target session against the live registry.  See #40, #41.
struct SessionResponder {
    tx: tokio_mpsc::UnboundedSender<Vec<u8>>,
    senders: SessionSenderMap,
    session_id: u64,
}

impl Responder for SessionResponder {
    fn send(&self, message: &[u8]) -> Result<(), SendError> {
        self.tx.send(message.to_vec()).map_err(|_| SendError {
            message: format!("session {} channel closed", self.session_id),
        })
    }

    fn send_to(&self, session_id: u64, message: &[u8]) -> Result<(), SendError> {
        let senders = self.senders.read();
        match senders.get(&session_id) {
            Some(sender) => sender.send(message.to_vec()).map_err(|_| SendError {
                message: format!("session {session_id} channel closed"),
            }),
            None => Err(SendError {
                message: format!("unknown session {session_id}"),
            }),
        }
    }
}

/// Handles a single client session over a transport [`Connection`].
///
/// `session_token` is the per-session [`CancellationToken`] cloned out
/// of `Server::session_tokens`. When the run loop fires
/// `ServerCommand::Shutdown` (cancels the parent) or
/// `ServerCommand::CloseSession(id)` (cancels just this child), this
/// function returns `Ok(())` and the spawned task drops `conn`,
/// closing the underlying socket so the peer observes EOF.
///
/// `out_tx` / `out_rx` are the two halves of the per-session
/// outbound channel, created in [`Server::handle_connection`] so the
/// sender can be registered in [`Server::session_senders`] before the
/// spawn.  `senders` is a clone of that shared map, handed into the
/// [`SessionResponder`] so cross-session `send_to` and
/// `ServerCommand::Broadcast` can find live sessions.  See #40, #41.
async fn handle_session<H, C>(
    session_id: u64,
    mut conn: C,
    handler: &H,
    session_token: CancellationToken,
    out_tx: tokio_mpsc::UnboundedSender<Vec<u8>>,
    mut out_rx: tokio_mpsc::UnboundedReceiver<Vec<u8>>,
    senders: SessionSenderMap,
) -> Result<(), std::io::Error>
where
    H: MessageHandler,
    C: Connection,
{
    let responder = SessionResponder {
        tx: out_tx,
        senders,
        session_id,
    };

    loop {
        tokio::select! {
            // Read incoming messages
            result = conn.recv() => {
                match result {
                    Ok(Some(data)) => {
                        // Decode header and dispatch to handler
                        if data.len() >= MessageHeader::ENCODED_LENGTH {
                            let header = MessageHeader::wrap(data.as_ref(), 0);
                            handler.on_message(session_id, &header, data.as_ref(), &responder);
                        } else {
                            handler.on_error(session_id, "Message too short for header");
                        }
                    }
                    Ok(None) => {
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "read error");
                        return Err(std::io::Error::other(e));
                    }
                }
            }

            // Send outgoing messages.  The send is itself raced
            // against `session_token.cancelled()` so that an in-flight
            // write to a stalled peer (TCP backpressure) cannot pin
            // the session task open after Shutdown / CloseSession —
            // the outer `select!` only races at the future level, so
            // once we enter this arm we are committed until the inner
            // `await` resolves.
            Some(msg) = out_rx.recv() => {
                tokio::select! {
                    send_result = conn.send(&msg) => {
                        if let Err(e) = send_result {
                            tracing::error!(error = %e, "write error");
                            return Err(std::io::Error::other(e));
                        }
                    }
                    _ = session_token.cancelled() => {
                        tracing::debug!("session cancelled mid-send");
                        return Ok(());
                    }
                }
            }

            // Cooperative cancellation from the run loop. Cleanup
            // (on_session_end + ServerEvent::SessionClosed) runs in
            // the spawned task closure once we return.
            _ = session_token.cancelled() => {
                tracing::debug!("session cancelled");
                return Ok(());
            }
        }
    }
}

#[cfg(all(test, feature = "tcp-tokio"))]
mod tests {
    use super::*;

    type DefaultBuilder<H> = ServerBuilder<H, ironsbe_transport::DefaultTransport>;

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
    fn test_server_builder_new() {
        let builder = DefaultBuilder::<TestHandler>::new();
        let _ = builder;
    }

    #[test]
    fn test_server_builder_default() {
        let builder = DefaultBuilder::<TestHandler>::default();
        let _ = builder;
    }

    #[test]
    fn test_server_builder_bind() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let builder = DefaultBuilder::<TestHandler>::new().bind(addr);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_handler() {
        let builder = DefaultBuilder::<TestHandler>::new().handler(TestHandler);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_max_connections() {
        let builder = DefaultBuilder::<TestHandler>::new().max_connections(500);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_channel_capacity() {
        let builder = DefaultBuilder::<TestHandler>::new().channel_capacity(8192);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_build() {
        let (_server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();
    }

    #[test]
    fn test_server_command_debug() {
        let cmd = ServerCommand::Shutdown;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Shutdown"));

        let cmd2 = ServerCommand::CloseSession(42);
        let debug_str2 = format!("{:?}", cmd2);
        assert!(debug_str2.contains("CloseSession"));

        let cmd3 = ServerCommand::Broadcast(vec![1, 2, 3]);
        let debug_str3 = format!("{:?}", cmd3);
        assert!(debug_str3.contains("Broadcast"));
    }

    #[test]
    fn test_server_event_clone_debug() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let event = ServerEvent::SessionCreated(1, addr);
        let cloned = event.clone();
        let _ = cloned;

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("SessionCreated"));

        let event2 = ServerEvent::SessionClosed(1);
        let debug_str2 = format!("{:?}", event2);
        assert!(debug_str2.contains("SessionClosed"));

        let event3 = ServerEvent::Error("test error".to_string());
        let debug_str3 = format!("{:?}", event3);
        assert!(debug_str3.contains("Error"));
    }

    #[test]
    fn test_server_handle_shutdown() {
        let (_server, handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();
        handle.shutdown();
    }

    #[test]
    fn test_server_handle_close_session() {
        let (_server, handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();
        handle.close_session(1);
    }

    #[test]
    fn test_server_handle_broadcast() {
        let (_server, handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();
        handle.broadcast(vec![1, 2, 3]);
    }

    /// `Server` is built with a fresh, uncancelled parent token and an
    /// empty session-token registry.  See #42.
    #[test]
    fn test_server_starts_with_uncancelled_shutdown_token() {
        let (server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        assert!(
            !server.shutdown_token.is_cancelled(),
            "fresh server should have an uncancelled shutdown_token"
        );
        assert!(
            server.session_tokens.is_empty(),
            "fresh server should have an empty session_tokens registry"
        );
    }

    /// Cancelling the parent shutdown token must propagate to every
    /// child token derived from it — this is the mechanism that
    /// `ServerCommand::Shutdown` relies on to terminate every spawned
    /// session task at once.  See #42.
    #[tokio::test]
    async fn test_shutdown_handler_cancels_every_child_token() {
        let (mut server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        // Pre-seed two child tokens as if two sessions had been
        // accepted, then drive the Shutdown command directly.
        let child_a = server.shutdown_token.child_token();
        let child_b = server.shutdown_token.child_token();
        server.session_tokens.insert(1, child_a.clone());
        server.session_tokens.insert(2, child_b.clone());

        let exited = server.handle_command(ServerCommand::Shutdown).await;

        assert!(exited, "Shutdown must signal the run loop to exit");
        assert!(
            server.shutdown_token.is_cancelled(),
            "parent token must be cancelled after Shutdown"
        );
        assert!(
            child_a.is_cancelled() && child_b.is_cancelled(),
            "every child token must be cancelled by the parent"
        );
        assert!(
            server.session_tokens.is_empty(),
            "session_tokens registry must be drained on Shutdown"
        );
    }

    /// `CloseSession(id)` must cancel exactly one child token and
    /// leave its siblings live.  This is the contract that
    /// `ServerHandle::close_session` exposes — without it the targeted
    /// session keeps running.  See #42.
    #[tokio::test]
    async fn test_close_session_handler_cancels_only_that_token() {
        let (mut server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        let child_a = server.shutdown_token.child_token();
        let child_b = server.shutdown_token.child_token();
        server.session_tokens.insert(1, child_a.clone());
        server.session_tokens.insert(2, child_b.clone());

        let exited = server.handle_command(ServerCommand::CloseSession(1)).await;

        assert!(!exited, "CloseSession must not stop the run loop");
        assert!(
            child_a.is_cancelled(),
            "the targeted child token must be cancelled"
        );
        assert!(
            !child_b.is_cancelled(),
            "untargeted siblings must remain live"
        );
        assert!(
            !server.session_tokens.contains_key(&1),
            "the closed session entry must be removed from the registry"
        );
        assert!(
            server.session_tokens.contains_key(&2),
            "untargeted entries must stay in the registry"
        );
    }

    /// `CloseSession` for an unknown id is a no-op (idempotent
    /// cleanup) — the spawned task fires its own `CloseSession` after
    /// it exits, and that second message must not panic or affect
    /// other state.  See #42.
    #[tokio::test]
    async fn test_close_session_handler_unknown_id_is_noop() {
        let (mut server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        let exited = server
            .handle_command(ServerCommand::CloseSession(999))
            .await;

        assert!(!exited);
        assert!(server.session_tokens.is_empty());
    }

    /// `Broadcast` with an empty session table must be a no-op: no
    /// panic, no error, and the registry stays empty.  See #40.
    #[tokio::test]
    async fn test_broadcast_handler_with_no_sessions_is_noop() {
        let (mut server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        let exited = server
            .handle_command(ServerCommand::Broadcast(b"anything".to_vec()))
            .await;

        assert!(!exited);
        assert!(server.session_senders.read().is_empty());
    }

    /// `Broadcast` must push the exact payload bytes to every live
    /// session's outbound channel.  See #40.
    #[tokio::test]
    async fn test_broadcast_handler_pushes_to_every_session() {
        let (mut server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        let (tx1, mut rx1) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let (tx2, mut rx2) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        {
            let mut senders = server.session_senders.write();
            senders.insert(1, tx1);
            senders.insert(2, tx2);
        }

        let payload = b"hello-broadcast".to_vec();
        let exited = server
            .handle_command(ServerCommand::Broadcast(payload.clone()))
            .await;

        assert!(!exited);
        match rx1.try_recv() {
            Ok(bytes) => assert_eq!(bytes, payload),
            other => panic!("session 1 did not receive broadcast: {other:?}"),
        }
        match rx2.try_recv() {
            Ok(bytes) => assert_eq!(bytes, payload),
            other => panic!("session 2 did not receive broadcast: {other:?}"),
        }
        // Both entries must still be live — their channels are
        // healthy.
        assert_eq!(server.session_senders.read().len(), 2);
    }

    /// `Broadcast` must drop entries whose receiver has already been
    /// closed (a session that exited but whose `CloseSession`
    /// cleanup has not yet reached the run loop).  See #40.
    #[tokio::test]
    async fn test_broadcast_handler_drops_closed_senders() {
        let (mut server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        let (tx_live, mut rx_live) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_dead, rx_dead) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        drop(rx_dead); // simulate a gone-away session
        {
            let mut senders = server.session_senders.write();
            senders.insert(1, tx_live);
            senders.insert(2, tx_dead);
        }

        let _ = server
            .handle_command(ServerCommand::Broadcast(b"ping".to_vec()))
            .await;

        // The live entry must have received the message and must
        // still be in the registry; the dead entry must be gone.
        match rx_live.try_recv() {
            Ok(bytes) => assert_eq!(bytes, b"ping"),
            other => panic!("live session did not receive broadcast: {other:?}"),
        }
        let senders = server.session_senders.read();
        assert_eq!(senders.len(), 1);
        assert!(senders.contains_key(&1));
        assert!(!senders.contains_key(&2));
    }

    /// `CloseSession` must remove the matching entry from
    /// `session_senders` alongside the cancellation bookkeeping.
    /// See #40, #41, #42.
    #[tokio::test]
    async fn test_close_session_handler_removes_session_sender() {
        let (mut server, _handle) = DefaultBuilder::<TestHandler>::new()
            .handler(TestHandler)
            .build();

        let (tx1, _rx1) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let (tx2, _rx2) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        {
            let mut senders = server.session_senders.write();
            senders.insert(1, tx1);
            senders.insert(2, tx2);
        }

        let _ = server.handle_command(ServerCommand::CloseSession(1)).await;

        let senders = server.session_senders.read();
        assert!(!senders.contains_key(&1));
        assert!(senders.contains_key(&2));
    }

    /// `SessionResponder::send_to` with a session id that is not in
    /// the registry must return `SendError`, not silently succeed.
    /// See #41.
    #[test]
    fn test_session_responder_send_to_unknown_session_returns_err() {
        let senders: SessionSenderMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let responder = SessionResponder {
            tx,
            senders,
            session_id: 1,
        };

        let result = responder.send_to(99, b"payload");
        match result {
            Err(err) => assert!(
                err.message.contains("unknown session 99"),
                "unexpected error: {err}"
            ),
            Ok(()) => panic!("send_to on unknown session must fail"),
        }
    }

    /// `SessionResponder::send_to` must route the payload to the
    /// target's channel and only the target's channel.  See #41.
    #[test]
    fn test_session_responder_send_to_routes_to_target() {
        let senders: SessionSenderMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx_self, mut rx_self) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_other, mut rx_other) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        senders.write().insert(1, tx_self.clone());
        senders.write().insert(2, tx_other);

        let responder = SessionResponder {
            tx: tx_self,
            senders,
            session_id: 1,
        };

        let result = responder.send_to(2, b"cross-routed");
        assert!(result.is_ok(), "send_to should succeed for a live target");

        match rx_other.try_recv() {
            Ok(bytes) => assert_eq!(bytes, b"cross-routed"),
            other => panic!("target session did not receive payload: {other:?}"),
        }
        // The responder's own channel must NOT have received the
        // message — this is the bug #41 was filed for.
        assert!(
            rx_self.try_recv().is_err(),
            "send_to must not fall through to the sender's own session"
        );
    }

    /// `SessionResponder::send_to` must return `SendError` when the
    /// target exists in the registry but its receiver has been
    /// dropped (channel closed).  See #41.
    #[test]
    fn test_session_responder_send_to_closed_channel_returns_err() {
        let senders: SessionSenderMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx_self, _rx_self) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_dead, rx_dead) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        drop(rx_dead);
        senders.write().insert(1, tx_self.clone());
        senders.write().insert(2, tx_dead);

        let responder = SessionResponder {
            tx: tx_self,
            senders,
            session_id: 1,
        };

        let result = responder.send_to(2, b"lost");
        match result {
            Err(err) => assert!(
                err.message.contains("channel closed"),
                "unexpected error: {err}"
            ),
            Ok(()) => panic!("send_to on closed channel must fail"),
        }
    }
}
