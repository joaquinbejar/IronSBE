//! Client builder and main client implementation.

use crate::error::ClientError;
use crate::reconnect::{ReconnectConfig, ReconnectState};
use crate::session::ClientSession;
use ironsbe_channel::spsc;
use std::net::SocketAddr;
use std::time::Duration;

/// Builder for configuring and creating a client.
pub struct ClientBuilder {
    server_addr: SocketAddr,
    connect_timeout: Duration,
    reconnect_config: ReconnectConfig,
    channel_capacity: usize,
}

impl ClientBuilder {
    /// Creates a new client builder for the specified server address.
    #[must_use]
    pub fn new(server_addr: SocketAddr) -> Self {
        Self {
            server_addr,
            connect_timeout: Duration::from_secs(5),
            reconnect_config: ReconnectConfig::default(),
            channel_capacity: 4096,
        }
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
    pub fn build(self) -> (Client, ClientHandle) {
        let (cmd_tx, cmd_rx) = spsc::channel(self.channel_capacity);
        let (event_tx, event_rx) = spsc::channel(self.channel_capacity);

        let client = Client {
            server_addr: self.server_addr,
            connect_timeout: self.connect_timeout,
            reconnect_state: ReconnectState::new(self.reconnect_config),
            cmd_rx,
            event_tx,
        };

        let handle = ClientHandle { cmd_tx, event_rx };

        (client, handle)
    }
}

/// The main client instance.
pub struct Client {
    server_addr: SocketAddr,
    connect_timeout: Duration,
    reconnect_state: ReconnectState,
    cmd_rx: spsc::SpscReceiver<ClientCommand>,
    event_tx: spsc::SpscSender<ClientEvent>,
}

impl Client {
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
        let stream = tokio::time::timeout(
            self.connect_timeout,
            tokio::net::TcpStream::connect(self.server_addr),
        )
        .await
        .map_err(|_| ClientError::ConnectTimeout)?
        .map_err(ClientError::Io)?;

        stream.set_nodelay(true)?;
        self.reconnect_state.on_success();

        let _ = self.event_tx.send(ClientEvent::Connected);
        tracing::info!("Connected to {}", self.server_addr);

        let mut session = ClientSession::new(stream);

        loop {
            tokio::select! {
                cmd = async { self.cmd_rx.recv() } => {
                    match cmd {
                        Some(ClientCommand::Send(msg)) => {
                            session.send(&msg).await?;
                        }
                        Some(ClientCommand::Disconnect) => {
                            return Ok(());
                        }
                        None => {
                            // Channel closed, check again
                            tokio::task::yield_now().await;
                        }
                    }
                }

                result = session.recv() => {
                    match result {
                        Ok(Some(msg)) => {
                            let _ = self.event_tx.send(ClientEvent::Message(msg.to_vec()));
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
            .map_err(|_| ClientError::Channel)
    }

    /// Disconnects from the server.
    pub fn disconnect(&mut self) {
        let _ = self.cmd_tx.send(ClientCommand::Disconnect);
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
