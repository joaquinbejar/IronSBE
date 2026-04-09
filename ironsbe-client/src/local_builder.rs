//! Single-threaded client builder for thread-per-core / `!Send` backends.
//!
//! Mirrors [`crate::ClientBuilder`] but is generic over [`LocalTransport`]
//! instead of [`Transport`](ironsbe_transport::Transport).  Use this when
//! the chosen backend (e.g. `tokio-uring` via the `tcp-uring` feature) is
//! single-threaded by construction.
//!
//! [`LocalClient::run`] must be polled inside a single-threaded reactor
//! that owns a Tokio `LocalSet` (typically `tokio_uring::start`).

use crate::builder::{ClientCommand, ClientEvent, ClientHandle};
use crate::error::ClientError;
use crate::reconnect::{ReconnectConfig, ReconnectState};
use ironsbe_channel::spsc;
use ironsbe_transport::traits::{LocalConnection, LocalTransport};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// Builder for [`LocalClient`].
///
/// Single-threaded counterpart of [`crate::ClientBuilder`]; the type
/// parameter `T` selects a [`LocalTransport`] backend rather than the
/// multi-threaded [`Transport`](ironsbe_transport::Transport) family.
pub struct LocalClientBuilder<T: LocalTransport> {
    server_addr: SocketAddr,
    connect_config: Option<T::ConnectConfig>,
    connect_timeout: Duration,
    reconnect_config: ReconnectConfig,
    channel_capacity: usize,
    _transport: PhantomData<T>,
}

impl<T: LocalTransport> LocalClientBuilder<T> {
    /// Creates a new local client builder targeting `server_addr`.
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
    #[must_use]
    pub fn connect_config(mut self, config: T::ConnectConfig) -> Self {
        self.connect_config = Some(config);
        self
    }

    /// Sets the outer connect timeout used by the reconnect loop.
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

    /// Sets the cmd/event channel capacity.
    #[must_use]
    pub fn channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }

    /// Builds the client and its external handle.
    #[must_use]
    pub fn build(self) -> (LocalClient<T>, ClientHandle) {
        let (cmd_tx, cmd_rx) = spsc::channel(self.channel_capacity);
        let (event_tx, event_rx) = spsc::channel(self.channel_capacity);
        let cmd_notify = Arc::new(Notify::new());
        let event_notify = Arc::new(Notify::new());

        let client = LocalClient {
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

        let handle = ClientHandle::new(cmd_tx, event_rx, cmd_notify, event_notify);
        (client, handle)
    }
}

/// Single-threaded client instance for [`LocalTransport`] backends.
///
/// `LocalClient::run` **must** be polled inside a Tokio `LocalSet`
/// (typically `tokio_uring::start(async move { client.run().await })`).
pub struct LocalClient<T: LocalTransport> {
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

impl<T: LocalTransport> LocalClient<T> {
    /// Runs the client, connecting and processing messages until shutdown.
    ///
    /// # Errors
    /// Returns [`ClientError`] if the connection fails repeatedly or the
    /// session encounters an unrecoverable error.
    pub async fn run(&mut self) -> Result<(), ClientError> {
        loop {
            match self.connect_and_run().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::error!("Local client connection error: {:?}", e);
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
        // Reconnect attempts share the same connect_config; clone on
        // each attempt so a custom config survives across reconnects.
        let connect_config = self
            .connect_config
            .clone()
            .unwrap_or_else(|| T::ConnectConfig::from(self.server_addr));
        let mut conn = tokio::time::timeout(self.connect_timeout, T::connect_with(connect_config))
            .await
            .map_err(|_| ClientError::ConnectTimeout)?
            .map_err(|e| ClientError::Io(std::io::Error::other(e.to_string())))?;

        self.reconnect_state.on_success();

        let _ = self.event_tx.send(ClientEvent::Connected);
        self.event_notify.notify_one();
        tracing::info!("Local client connected to {}", self.server_addr);

        loop {
            tokio::select! {
                _ = self.cmd_notify.notified() => {
                    while let Some(cmd) = self.cmd_rx.recv() {
                        match cmd {
                            ClientCommand::Send(msg) => {
                                conn.send(&msg)
                                    .await
                                    .map_err(|e| ClientError::Io(std::io::Error::other(e.to_string())))?;
                            }
                            ClientCommand::Disconnect => return Ok(()),
                        }
                    }
                }

                result = conn.recv() => {
                    match result {
                        Ok(Some(msg)) => {
                            let _ = self.event_tx.send(ClientEvent::Message(msg.to_vec()));
                            self.event_notify.notify_one();
                        }
                        Ok(None) => return Err(ClientError::ConnectionClosed),
                        Err(e) => {
                            return Err(ClientError::Io(std::io::Error::other(e.to_string())));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "tcp-uring", target_os = "linux"))]
mod tests {
    use super::*;
    use ironsbe_transport::tcp_uring::UringTcpTransport;

    #[test]
    fn test_local_client_builder_new() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().expect("test addr");
        let builder = LocalClientBuilder::<UringTcpTransport>::new(addr);
        let _ = builder;
    }

    #[test]
    fn test_local_client_builder_connect_timeout() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().expect("test addr");
        let builder = LocalClientBuilder::<UringTcpTransport>::new(addr)
            .connect_timeout(Duration::from_secs(2));
        let _ = builder;
    }

    #[test]
    fn test_local_client_builder_build() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().expect("test addr");
        let (_client, _handle) = LocalClientBuilder::<UringTcpTransport>::new(addr).build();
    }
}
