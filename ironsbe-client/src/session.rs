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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sbe_frame_codec_new() {
        let codec = SbeFrameCodec::new();
        assert_eq!(codec.max_frame_size, 64 * 1024);
    }

    #[test]
    fn test_sbe_frame_codec_with_max_frame_size() {
        let codec = SbeFrameCodec::with_max_frame_size(128 * 1024);
        assert_eq!(codec.max_frame_size, 128 * 1024);
    }

    #[test]
    fn test_sbe_frame_codec_default() {
        let codec = SbeFrameCodec::default();
        assert_eq!(codec.max_frame_size, 64 * 1024);
    }

    #[test]
    fn test_decode_incomplete_header() {
        let mut codec = SbeFrameCodec::new();
        let mut buf = BytesMut::from(&[0u8, 1, 2][..]);

        let result = codec.decode(&mut buf);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_decode_incomplete_frame() {
        let mut codec = SbeFrameCodec::new();
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&10u32.to_le_bytes()); // length = 10
        buf.extend_from_slice(&[1, 2, 3, 4, 5]); // only 5 bytes, need 10

        let result = codec.decode(&mut buf);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_decode_complete_frame() {
        let mut codec = SbeFrameCodec::new();
        let mut buf = BytesMut::new();
        let data = b"Hello";
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(data);

        let result = codec.decode(&mut buf);
        assert!(result.is_ok());
        let frame = result.unwrap();
        assert!(frame.is_some());
        assert_eq!(frame.unwrap().as_ref(), data);
    }

    #[test]
    fn test_decode_frame_too_large() {
        let mut codec = SbeFrameCodec::with_max_frame_size(10);
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&100u32.to_le_bytes()); // length = 100, exceeds max

        let result = codec.decode(&mut buf);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_encode_frame() {
        use tokio_util::codec::Encoder;

        let mut codec = SbeFrameCodec::new();
        let mut buf = BytesMut::new();
        let data = b"Hello";

        let result = codec.encode(data.as_slice(), &mut buf);
        assert!(result.is_ok());

        // Check length prefix
        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        assert_eq!(len, data.len());

        // Check data
        assert_eq!(&buf[4..], data);
    }

    #[test]
    fn test_encode_frame_too_large() {
        use tokio_util::codec::Encoder;

        let mut codec = SbeFrameCodec::with_max_frame_size(5);
        let mut buf = BytesMut::new();
        let data = b"Hello World"; // 11 bytes, exceeds max of 5

        let result = codec.encode(data.as_slice(), &mut buf);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
