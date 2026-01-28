//! Server builder and main server implementation.

use crate::error::ServerError;
use crate::handler::{MessageHandler, Responder, SendError};
use crate::session::SessionManager;
use bytes::BytesMut;
use futures::SinkExt;
use ironsbe_channel::mpsc::{MpscChannel, MpscReceiver, MpscSender};
use ironsbe_core::header::MessageHeader;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, Encoder, Framed};

/// Builder for configuring and creating a server.
pub struct ServerBuilder<H> {
    bind_addr: SocketAddr,
    handler: Option<H>,
    max_connections: usize,
    max_frame_size: usize,
    channel_capacity: usize,
}

impl<H: MessageHandler> ServerBuilder<H> {
    /// Creates a new server builder with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bind_addr: "0.0.0.0:9000".parse().unwrap(),
            handler: None,
            max_connections: 1000,
            max_frame_size: 64 * 1024,
            channel_capacity: 4096,
        }
    }

    /// Sets the bind address.
    #[must_use]
    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
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

    /// Sets the maximum frame size.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.max_frame_size = size;
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
    pub fn build(self) -> (Server<H>, ServerHandle) {
        let handler = self.handler.expect("Handler required");
        let (cmd_tx, cmd_rx) = MpscChannel::bounded(self.channel_capacity);
        let (event_tx, event_rx) = MpscChannel::bounded(self.channel_capacity);

        let server = Server {
            bind_addr: self.bind_addr,
            handler: Arc::new(handler),
            max_connections: self.max_connections,
            max_frame_size: self.max_frame_size,
            cmd_rx,
            event_tx,
            sessions: SessionManager::new(),
        };

        let handle = ServerHandle { cmd_tx, event_rx };

        (server, handle)
    }
}

impl<H: MessageHandler> Default for ServerBuilder<H> {
    fn default() -> Self {
        Self::new()
    }
}

/// The main server instance.
#[allow(dead_code)]
pub struct Server<H> {
    bind_addr: SocketAddr,
    handler: Arc<H>,
    max_connections: usize,
    max_frame_size: usize,
    cmd_rx: MpscReceiver<ServerCommand>,
    event_tx: MpscSender<ServerEvent>,
    sessions: SessionManager,
}

impl<H: MessageHandler + Send + Sync + 'static> Server<H> {
    /// Runs the server, accepting connections and processing messages.
    ///
    /// # Errors
    /// Returns `ServerError` if the server fails to start or encounters an error.
    pub async fn run(&mut self) -> Result<(), ServerError> {
        let listener = tokio::net::TcpListener::bind(self.bind_addr).await?;
        tracing::info!("Server listening on {}", self.bind_addr);

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            self.handle_connection(stream, addr).await;
                        }
                        Err(e) => {
                            tracing::error!("Accept error: {}", e);
                        }
                    }
                }

                cmd = async { self.cmd_rx.try_recv() } => {
                    if let Some(cmd) = cmd && self.handle_command(cmd).await {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn handle_connection(&mut self, stream: TcpStream, addr: SocketAddr) {
        if self.sessions.count() >= self.max_connections {
            tracing::warn!("Max connections reached, rejecting {}", addr);
            return;
        }

        let session_id = self.sessions.create_session(addr);
        let handler = Arc::clone(&self.handler);
        let event_tx = self.event_tx.clone();
        let max_frame_size = self.max_frame_size;

        handler.on_session_start(session_id);
        let _ = event_tx.try_send(ServerEvent::SessionCreated(session_id, addr));

        // Spawn connection handler task
        tokio::spawn(async move {
            tracing::info!("Session {} connected from {}", session_id, addr);

            if let Err(e) =
                handle_session(session_id, stream, handler.as_ref(), max_frame_size).await
            {
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
}

impl ServerHandle {
    /// Requests server shutdown.
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.try_send(ServerCommand::Shutdown);
    }

    /// Closes a specific session.
    pub fn close_session(&self, session_id: u64) {
        let _ = self
            .cmd_tx
            .try_send(ServerCommand::CloseSession(session_id));
    }

    /// Broadcasts a message to all sessions.
    pub fn broadcast(&self, message: Vec<u8>) {
        let _ = self.cmd_tx.try_send(ServerCommand::Broadcast(message));
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

/// Length-prefixed frame codec for SBE messages.
struct SbeFrameCodec {
    max_frame_size: usize,
}

impl SbeFrameCodec {
    fn new(max_frame_size: usize) -> Self {
        Self { max_frame_size }
    }
}

impl Decoder for SbeFrameCodec {
    type Item = BytesMut;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }

        let length = u32::from_le_bytes([src[0], src[1], src[2], src[3]]) as usize;

        if length > self.max_frame_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Frame too large: {} > {}", length, self.max_frame_size),
            ));
        }

        if src.len() < 4 + length {
            src.reserve(4 + length - src.len());
            return Ok(None);
        }

        let _ = src.split_to(4);
        Ok(Some(src.split_to(length)))
    }
}

impl<T: AsRef<[u8]>> Encoder<T> for SbeFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let data = item.as_ref();
        let length = data.len() as u32;
        dst.reserve(4 + data.len());
        dst.extend_from_slice(&length.to_le_bytes());
        dst.extend_from_slice(data);
        Ok(())
    }
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

/// Handles a single client session.
async fn handle_session<H: MessageHandler>(
    session_id: u64,
    stream: TcpStream,
    handler: &H,
    max_frame_size: usize,
) -> Result<(), std::io::Error> {
    let codec = SbeFrameCodec::new(max_frame_size);
    let mut framed = Framed::new(stream, codec);

    // Channel for sending responses
    let (tx, mut rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
    let responder = SessionResponder { tx };

    loop {
        tokio::select! {
            // Read incoming messages
            result = framed.next() => {
                match result {
                    Some(Ok(data)) => {
                        // Decode header and dispatch to handler
                        if data.len() >= MessageHeader::ENCODED_LENGTH {
                            let header = MessageHeader::wrap(data.as_ref(), 0);
                            handler.on_message(session_id, &header, data.as_ref(), &responder);
                        } else {
                            handler.on_error(session_id, "Message too short for header");
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("Session {} read error: {}", session_id, e);
                        return Err(e);
                    }
                    None => {
                        tracing::info!("Session {} disconnected", session_id);
                        return Ok(());
                    }
                }
            }

            // Send outgoing messages
            Some(msg) = rx.recv() => {
                if let Err(e) = framed.send(msg).await {
                    tracing::error!("Session {} write error: {}", session_id, e);
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let builder = ServerBuilder::<TestHandler>::new();
        let _ = builder;
    }

    #[test]
    fn test_server_builder_default() {
        let builder = ServerBuilder::<TestHandler>::default();
        let _ = builder;
    }

    #[test]
    fn test_server_builder_bind() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let builder = ServerBuilder::<TestHandler>::new().bind(addr);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_handler() {
        let builder = ServerBuilder::new().handler(TestHandler);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_max_connections() {
        let builder = ServerBuilder::<TestHandler>::new().max_connections(500);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_max_frame_size() {
        let builder = ServerBuilder::<TestHandler>::new().max_frame_size(128 * 1024);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_channel_capacity() {
        let builder = ServerBuilder::<TestHandler>::new().channel_capacity(8192);
        let _ = builder;
    }

    #[test]
    fn test_server_builder_build() {
        let (_server, _handle) = ServerBuilder::new().handler(TestHandler).build();
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
        let (_server, handle) = ServerBuilder::new().handler(TestHandler).build();
        handle.shutdown();
    }

    #[test]
    fn test_server_handle_close_session() {
        let (_server, handle) = ServerBuilder::new().handler(TestHandler).build();
        handle.close_session(1);
    }

    #[test]
    fn test_server_handle_broadcast() {
        let (_server, handle) = ServerBuilder::new().handler(TestHandler).build();
        handle.broadcast(vec![1, 2, 3]);
    }

    #[test]
    fn test_sbe_frame_codec_new() {
        let codec = SbeFrameCodec::new(64 * 1024);
        assert_eq!(codec.max_frame_size, 64 * 1024);
    }

    #[test]
    fn test_sbe_frame_codec_decode_incomplete() {
        let mut codec = SbeFrameCodec::new(1024);
        let mut buf = BytesMut::from(&[0u8, 0, 0][..]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn test_sbe_frame_codec_decode_complete() {
        let mut codec = SbeFrameCodec::new(1024);
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&5u32.to_le_bytes());
        buf.extend_from_slice(b"hello");

        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().as_ref(), b"hello");
    }

    #[test]
    fn test_sbe_frame_codec_decode_too_large() {
        let mut codec = SbeFrameCodec::new(10);
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&100u32.to_le_bytes());

        let result = codec.decode(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_sbe_frame_codec_encode() {
        let mut codec = SbeFrameCodec::new(1024);
        let mut buf = BytesMut::new();
        codec.encode(b"hello", &mut buf).unwrap();

        assert_eq!(&buf[0..4], &5u32.to_le_bytes());
        assert_eq!(&buf[4..9], b"hello");
    }
}
