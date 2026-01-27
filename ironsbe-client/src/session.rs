//! Client session management.

use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Decoder, Framed};

/// Simple length-prefixed framing codec for SBE messages.
pub struct SbeFrameCodec {
    max_frame_size: usize,
}

impl SbeFrameCodec {
    /// Creates a new frame codec with default max frame size.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_frame_size: 64 * 1024,
        }
    }

    /// Creates a new frame codec with custom max frame size.
    #[must_use]
    pub fn with_max_frame_size(max_frame_size: usize) -> Self {
        Self { max_frame_size }
    }
}

impl Default for SbeFrameCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder for SbeFrameCodec {
    type Item = BytesMut;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        use bytes::Buf;

        if src.len() < 4 {
            return Ok(None);
        }

        let length = u32::from_le_bytes([src[0], src[1], src[2], src[3]]) as usize;

        if length > self.max_frame_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }

        if src.len() < 4 + length {
            src.reserve(4 + length - src.len());
            return Ok(None);
        }

        src.advance(4);
        Ok(Some(src.split_to(length)))
    }
}

impl<T: AsRef<[u8]>> tokio_util::codec::Encoder<T> for SbeFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        use bytes::BufMut;

        let data = item.as_ref();
        if data.len() > self.max_frame_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }

        dst.reserve(4 + data.len());
        dst.put_u32_le(data.len() as u32);
        dst.put_slice(data);
        Ok(())
    }
}

/// Client session wrapping a TCP connection.
pub struct ClientSession {
    framed: Framed<TcpStream, SbeFrameCodec>,
}

impl ClientSession {
    /// Creates a new client session from a TCP stream.
    #[must_use]
    pub fn new(stream: TcpStream) -> Self {
        Self {
            framed: Framed::new(stream, SbeFrameCodec::default()),
        }
    }

    /// Creates a new client session with custom frame size.
    #[must_use]
    pub fn with_max_frame_size(stream: TcpStream, max_frame_size: usize) -> Self {
        Self {
            framed: Framed::new(stream, SbeFrameCodec::with_max_frame_size(max_frame_size)),
        }
    }

    /// Sends a message to the server.
    ///
    /// # Errors
    /// Returns IO error if send fails.
    pub async fn send(&mut self, message: &[u8]) -> std::io::Result<()> {
        self.framed.send(message).await
    }

    /// Receives a message from the server.
    ///
    /// # Returns
    /// `Ok(Some(bytes))` if received, `Ok(None)` if connection closed.
    ///
    /// # Errors
    /// Returns IO error if receive fails.
    pub async fn recv(&mut self) -> std::io::Result<Option<BytesMut>> {
        match self.framed.next().await {
            Some(result) => result.map(Some),
            None => Ok(None),
        }
    }

    /// Closes the session.
    pub async fn close(mut self) -> std::io::Result<()> {
        SinkExt::<&[u8]>::close(&mut self.framed).await
    }
}
