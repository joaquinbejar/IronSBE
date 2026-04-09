//! Server builder and main server implementation.

use crate::error::ServerError;
use crate::handler::{MessageHandler, Responder, SendError};
use crate::session::SessionManager;
use ironsbe_channel::mpsc::{MpscChannel, MpscReceiver, MpscSender};
use ironsbe_core::header::MessageHeader;
use ironsbe_transport::traits::{Connection, Listener, Transport};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc as tokio_mpsc};

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
            cmd_rx,
            event_tx,
            sessions: SessionManager::new(),
            cmd_notify: Arc::clone(&cmd_notify),
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
    cmd_rx: MpscReceiver<ServerCommand>,
    event_tx: MpscSender<ServerEvent>,
    sessions: SessionManager,
    cmd_notify: Arc<Notify>,
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
    cmd_rx: MpscReceiver<ServerCommand>,
    event_tx: MpscSender<ServerEvent>,
    sessions: SessionManager,
    cmd_notify: Arc<Notify>,
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
        tracing::info!("Server listening on {}", self.bind_addr);

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

        handler.on_session_start(session_id);
        let _ = event_tx.try_send(ServerEvent::SessionCreated(session_id, addr));

        // Spawn connection handler task
        tokio::spawn(async move {
            tracing::info!("Session {} connected from {}", session_id, addr);

            if let Err(e) = handle_session(session_id, conn, handler.as_ref()).await {
                tracing::error!("Session {} error: {:?}", session_id, e);
            }

            // When done, notify
            handler.on_session_end(session_id);
            let _ = event_tx.try_send(ServerEvent::SessionClosed(session_id));
        });
    }

    async fn handle_command(&mut self, cmd: ServerCommand) -> bool {
        match cmd {
            ServerCommand::Shutdown => {
                tracing::info!("Server shutdown requested");
                true
            }
            ServerCommand::CloseSession(session_id) => {
                self.sessions.close_session(session_id);
                false
            }
            ServerCommand::Broadcast(_message) => {
                // Broadcast to all sessions
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
#[derive(Debug)]
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
    /// A new session was created.
    SessionCreated(u64, SocketAddr),
    /// A session was closed.
    SessionClosed(u64),
    /// An error occurred.
    Error(String),
}

/// Session responder that sends messages back to the client.
struct SessionResponder {
    tx: tokio_mpsc::UnboundedSender<Vec<u8>>,
}

impl Responder for SessionResponder {
    fn send(&self, message: &[u8]) -> Result<(), SendError> {
        self.tx.send(message.to_vec()).map_err(|_| SendError {
            message: "channel closed".to_string(),
        })
    }

    fn send_to(&self, _session_id: u64, message: &[u8]) -> Result<(), SendError> {
        // For now, just send to current session
        self.send(message)
    }
}

/// Handles a single client session over a transport [`Connection`].
async fn handle_session<H, C>(
    session_id: u64,
    mut conn: C,
    handler: &H,
) -> Result<(), std::io::Error>
where
    H: MessageHandler,
    C: Connection,
{
    // Channel for sending responses
    let (tx, mut rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
    let responder = SessionResponder { tx };

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
                        tracing::info!("Session {} disconnected", session_id);
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::error!("Session {} read error: {}", session_id, e);
                        return Err(std::io::Error::other(e));
                    }
                }
            }

            // Send outgoing messages
            Some(msg) = rx.recv() => {
                if let Err(e) = conn.send(&msg).await {
                    tracing::error!("Session {} write error: {}", session_id, e);
                    return Err(std::io::Error::other(e));
                }
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
}
