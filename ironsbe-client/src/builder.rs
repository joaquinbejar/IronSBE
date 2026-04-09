//! Client builder and main client implementation.

use crate::error::ClientError;
use crate::reconnect::{ReconnectConfig, ReconnectState};
use crate::session::ClientSession;
use ironsbe_channel::spsc;
use ironsbe_transport::traits::Transport;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// Builder for configuring and creating a client.
///
/// The type parameter `T` selects the transport backend.  When the
/// `tcp-tokio` feature is enabled (the default), `T` defaults to
/// [`ironsbe_transport::DefaultTransport`] so existing call-sites compile
/// without changes.  With the feature disabled, `T` must be specified
/// explicitly so downstream crates can plug in a custom backend.
#[cfg(feature = "tcp-tokio")]
pub struct ClientBuilder<T: Transport = ironsbe_transport::DefaultTransport> {
    server_addr: SocketAddr,
    connect_config: Option<T::ConnectConfig>,
    connect_timeout: Duration,
    reconnect_config: ReconnectConfig,
    channel_capacity: usize,
    _transport: PhantomData<T>,
}

/// Builder for configuring and creating a client.
///
/// With the `tcp-tokio` feature disabled, the transport backend must be
/// specified explicitly via the `T` type parameter.
#[cfg(not(feature = "tcp-tokio"))]
pub struct ClientBuilder<T: Transport> {
    server_addr: SocketAddr,
    connect_config: Option<T::ConnectConfig>,
    connect_timeout: Duration,
    reconnect_config: ReconnectConfig,
    channel_capacity: usize,
    _transport: PhantomData<T>,
}

impl<T: Transport> ClientBuilder<T> {
    /// Creates a new client builder for the specified server address.
    #[must_use]
    pub fn new(server_addr: SocketAddr) -> Self {
        Self {
            server_addr,
            connect_config: None,
            connect_timeout: Duration::from_secs(5),
            reconnect_config: ReconnectConfig::default(),
            channel_capacity: 4096,
            _transport: PhantomData,
        }
    }

    /// Supplies a backend-specific connect configuration.
    ///
    /// Use this to override transport tunables (frame size, NODELAY, socket
    /// buffer sizes, …).  When unset, the backend builds a default config
    /// from the server address.
    #[must_use]
    pub fn connect_config(mut self, config: T::ConnectConfig) -> Self {
        self.connect_config = Some(config);
        self
    }

    /// Sets the connection timeout.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Enables or disables automatic reconnection.
    #[must_use]
    pub fn reconnect(mut self, enabled: bool) -> Self {
        self.reconnect_config.enabled = enabled;
        self
    }

    /// Sets the reconnection delay.
    #[must_use]
    pub fn reconnect_delay(mut self, delay: Duration) -> Self {
        self.reconnect_config.initial_delay = delay;
        self
    }

    /// Sets the maximum reconnection attempts.
    #[must_use]
    pub fn max_reconnect_attempts(mut self, max: usize) -> Self {
        self.reconnect_config.max_attempts = max;
        self
    }

    /// Sets the channel capacity.
    #[must_use]
    pub fn channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }

    /// Builds the client and handle.
    #[must_use]
    pub fn build(self) -> (Client<T>, ClientHandle) {
        let (cmd_tx, cmd_rx) = spsc::channel(self.channel_capacity);
        let (event_tx, event_rx) = spsc::channel(self.channel_capacity);

        let cmd_notify = Arc::new(Notify::new());
        let event_notify = Arc::new(Notify::new());

        let client = Client {
            server_addr: self.server_addr,
            connect_config: Some(
                self.connect_config
                    .unwrap_or_else(|| T::ConnectConfig::from(self.server_addr)),
            ),
            connect_timeout: self.connect_timeout,
            reconnect_state: ReconnectState::new(self.reconnect_config),
            cmd_rx,
            event_tx,
            cmd_notify: Arc::clone(&cmd_notify),
            event_notify: Arc::clone(&event_notify),
            _transport: PhantomData,
        };

        let handle = ClientHandle {
            cmd_tx,
            event_rx,
            cmd_notify,
            event_notify,
        };

        (client, handle)
    }
}

#[cfg(feature = "tcp-tokio")]
impl ClientBuilder {
    /// Creates a new client builder using the default transport backend.
    ///
    /// This is a convenience constructor that resolves the transport type
    /// parameter to [`ironsbe_transport::DefaultTransport`], keeping existing
    /// call-sites like `ClientBuilder::with_default_transport(addr).build()`
    /// working without turbofish syntax.
    #[must_use]
    pub fn with_default_transport(server_addr: SocketAddr) -> Self {
        Self::new(server_addr)
    }

    /// Sets the maximum SBE frame size in bytes (Tokio TCP backend only).
    ///
    /// Convenience shortcut that mutates the underlying
    /// [`ironsbe_transport::tcp::TcpClientConfig`] without requiring callers
    /// to construct it manually.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        let cfg = self
            .connect_config
            .take()
            .unwrap_or_else(|| ironsbe_transport::tcp::TcpClientConfig::new(self.server_addr));
        self.connect_config = Some(cfg.max_frame_size(size));
        self
    }
}

/// The main client instance.
///
/// Generic over transport backend `T`.
#[cfg(feature = "tcp-tokio")]
pub struct Client<T: Transport = ironsbe_transport::DefaultTransport> {
    server_addr: SocketAddr,
    connect_config: Option<T::ConnectConfig>,
    connect_timeout: Duration,
    reconnect_state: ReconnectState,
    cmd_rx: spsc::SpscReceiver<ClientCommand>,
    event_tx: spsc::SpscSender<ClientEvent>,
    cmd_notify: Arc<Notify>,
    event_notify: Arc<Notify>,
    _transport: PhantomData<T>,
}

/// The main client instance.
///
/// Generic over transport backend `T`.
#[cfg(not(feature = "tcp-tokio"))]
pub struct Client<T: Transport> {
    server_addr: SocketAddr,
    connect_config: Option<T::ConnectConfig>,
    connect_timeout: Duration,
    reconnect_state: ReconnectState,
    cmd_rx: spsc::SpscReceiver<ClientCommand>,
    event_tx: spsc::SpscSender<ClientEvent>,
    cmd_notify: Arc<Notify>,
    event_notify: Arc<Notify>,
    _transport: PhantomData<T>,
}

impl<T: Transport> Client<T> {
    /// Runs the client, connecting to the server and processing messages.
    ///
    /// # Errors
    /// Returns `ClientError` if the client fails to connect or encounters an error.
    pub async fn run(&mut self) -> Result<(), ClientError> {
        loop {
            match self.connect_and_run().await {
                Ok(()) => {
                    // Normal shutdown
                    return Ok(());
                }
                Err(e) => {
                    tracing::error!("Connection error: {:?}", e);

                    if let Some(delay) = self.reconnect_state.on_failure() {
                        let _ = self.event_tx.send(ClientEvent::Disconnected);
                        self.event_notify.notify_one();
                        tracing::info!("Reconnecting in {:?}...", delay);
                        tokio::time::sleep(delay).await;
                    } else {
                        tracing::error!("Max reconnect attempts reached");
                        return Err(ClientError::MaxReconnectAttempts);
                    }
                }
            }
        }
    }

    async fn connect_and_run(&mut self) -> Result<(), ClientError> {
        // Reconnect attempts share the same connect_config; clone on each attempt.
        let connect_config = self
            .connect_config
            .clone()
            .unwrap_or_else(|| T::ConnectConfig::from(self.server_addr));
        let conn = tokio::time::timeout(self.connect_timeout, T::connect_with(connect_config))
            .await
            .map_err(|_| ClientError::ConnectTimeout)?
            .map_err(|e| ClientError::Io(std::io::Error::other(e)))?;

        self.reconnect_state.on_success();

        let _ = self.event_tx.send(ClientEvent::Connected);
        self.event_notify.notify_one();
        tracing::info!("Connected to {}", self.server_addr);

        let mut session = ClientSession::new(conn);

        loop {
            tokio::select! {
                _ = self.cmd_notify.notified() => {
                    // Drain all available commands after notification.
                    while let Some(cmd) = self.cmd_rx.recv() {
                        match cmd {
                            ClientCommand::Send(msg) => {
                                session.send(&msg).await?;
                            }
                            ClientCommand::Disconnect => {
                                return Ok(());
                            }
                        }
                    }
                }

                result = session.recv() => {
                    match result {
                        Ok(Some(msg)) => {
                            let _ = self.event_tx.send(ClientEvent::Message(msg.to_vec()));
                            self.event_notify.notify_one();
                        }
                        Ok(None) => {
                            return Err(ClientError::ConnectionClosed);
                        }
                        Err(e) => {
                            return Err(ClientError::Io(e));
                        }
                    }
                }
            }
        }
    }
}

/// Handle for sending messages and receiving events.
pub struct ClientHandle {
    cmd_tx: spsc::SpscSender<ClientCommand>,
    event_rx: spsc::SpscReceiver<ClientEvent>,
    cmd_notify: Arc<Notify>,
    event_notify: Arc<Notify>,
}

impl ClientHandle {
    /// Sends an SBE message to the server (non-blocking).
    ///
    /// # Errors
    /// Returns error if the channel is disconnected.
    #[inline]
    pub fn send(&mut self, message: Vec<u8>) -> Result<(), ClientError> {
        self.cmd_tx
            .send(ClientCommand::Send(message))
            .map_err(|_| ClientError::Channel)?;
        self.cmd_notify.notify_one();
        Ok(())
    }

    /// Disconnects from the server.
    pub fn disconnect(&mut self) {
        let _ = self.cmd_tx.send(ClientCommand::Disconnect);
        self.cmd_notify.notify_one();
    }

    /// Polls for events (non-blocking).
    #[inline]
    pub fn poll(&mut self) -> Option<ClientEvent> {
        self.event_rx.recv()
    }

    /// Busy-poll for next event (for hot path).
    #[inline]
    pub fn poll_spin(&mut self) -> ClientEvent {
        self.event_rx.recv_spin()
    }

    /// Drains all available events.
    pub fn drain(&mut self) -> impl Iterator<Item = ClientEvent> + '_ {
        self.event_rx.drain()
    }

    /// Asynchronously waits for the next event.
    ///
    /// Returns `Some(event)` when an event is available, or keeps waiting.
    /// Returns `None` only if the sender (client) has been dropped.
    pub async fn wait_event(&mut self) -> Option<ClientEvent> {
        loop {
            if let Some(event) = self.event_rx.recv() {
                return Some(event);
            }
            if !self.event_rx.is_connected() {
                return None;
            }
            self.event_notify.notified().await;
        }
    }

    /// Returns a clone of the event notification handle.
    ///
    /// Use this to await event availability when holding the handle behind
    /// a `Mutex` — await the notifier *outside* the lock, then lock and
    /// drain with \[`poll`\].
    #[must_use]
    pub fn event_notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.event_notify)
    }
}

/// Commands that can be sent to the client.
#[derive(Debug)]
pub enum ClientCommand {
    /// Send a message to the server.
    Send(Vec<u8>),
    /// Disconnect from the server.
    Disconnect,
}

/// Events emitted by the client.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// Connected to the server.
    Connected,
    /// Disconnected from the server.
    Disconnected,
    /// Received a message from the server.
    Message(Vec<u8>),
    /// An error occurred.
    Error(String),
}

#[cfg(all(test, feature = "tcp-tokio"))]
mod tests {
    use super::*;
    use std::time::Duration;

    type DefaultClientBuilder = ClientBuilder<ironsbe_transport::DefaultTransport>;

    #[test]
    fn test_client_builder_new() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let builder = DefaultClientBuilder::new(addr);
        let _ = builder;
    }

    #[test]
    fn test_client_builder_connect_timeout() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let builder = DefaultClientBuilder::new(addr).connect_timeout(Duration::from_secs(10));
        let _ = builder;
    }

    #[test]
    fn test_client_builder_reconnect() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let builder = DefaultClientBuilder::new(addr).reconnect(true);
        let _ = builder;
    }

    #[test]
    fn test_client_builder_reconnect_delay() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let builder = DefaultClientBuilder::new(addr).reconnect_delay(Duration::from_millis(500));
        let _ = builder;
    }

    #[test]
    fn test_client_builder_max_reconnect_attempts() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let builder = DefaultClientBuilder::new(addr).max_reconnect_attempts(5);
        let _ = builder;
    }

    #[test]
    fn test_client_builder_channel_capacity() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let builder = DefaultClientBuilder::new(addr).channel_capacity(8192);
        let _ = builder;
    }

    #[test]
    fn test_client_builder_build() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let (_client, _handle) = DefaultClientBuilder::new(addr).build();
    }

    #[test]
    fn test_client_command_debug() {
        let cmd = ClientCommand::Send(vec![1, 2, 3]);
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Send"));

        let cmd2 = ClientCommand::Disconnect;
        let debug_str2 = format!("{:?}", cmd2);
        assert!(debug_str2.contains("Disconnect"));
    }

    #[test]
    fn test_client_event_clone_debug() {
        let event = ClientEvent::Connected;
        let cloned = event.clone();
        let _ = cloned;

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("Connected"));

        let event2 = ClientEvent::Message(vec![1, 2, 3]);
        let debug_str2 = format!("{:?}", event2);
        assert!(debug_str2.contains("Message"));

        let event3 = ClientEvent::Error("test error".to_string());
        let debug_str3 = format!("{:?}", event3);
        assert!(debug_str3.contains("Error"));
    }

    #[test]
    fn test_client_handle_disconnect() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let (_client, mut handle) = DefaultClientBuilder::new(addr).build();
        handle.disconnect();
    }

    #[test]
    fn test_client_handle_poll() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let (_client, mut handle) = DefaultClientBuilder::new(addr).build();
        assert!(handle.poll().is_none());
    }
}
