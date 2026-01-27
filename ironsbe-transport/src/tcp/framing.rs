//! SBE message framing codec for TCP.
//!
//! Provides length-prefixed framing for SBE messages over TCP streams.

use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Simple length-prefixed framing codec for SBE messages.
///
/// Frame format: `[4-byte length (little-endian)][SBE message]`
pub struct SbeFrameCodec {
    max_frame_size: usize,
}

impl SbeFrameCodec {
    /// Creates a new frame codec with the specified maximum frame size.
    ///
    /// # Arguments
    /// * `max_frame_size` - Maximum allowed frame size in bytes
    #[must_use]
    pub fn new(max_frame_size: usize) -> Self {
        Self { max_frame_size }
    }

    /// Returns the maximum frame size.
    #[must_use]
    pub fn max_frame_size(&self) -> usize {
        self.max_frame_size
    }
}

impl Default for SbeFrameCodec {
    fn default() -> Self {
        Self::new(64 * 1024) // 64KB default
    }
}

impl Decoder for SbeFrameCodec {
    type Item = BytesMut;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Need at least 4 bytes for length prefix
        if src.len() < 4 {
            return Ok(None);
        }

        // Read length (little-endian)
        let length = u32::from_le_bytes([src[0], src[1], src[2], src[3]]) as usize;

        // Validate frame size
        if length > self.max_frame_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "frame too large: {} bytes exceeds maximum {} bytes",
                    length, self.max_frame_size
                ),
            ));
        }

        // Check if we have the complete frame
        if src.len() < 4 + length {
            // Reserve space for the rest of the frame
            src.reserve(4 + length - src.len());
            return Ok(None);
        }

        // Skip the length prefix
        src.advance(4);

        // Extract the frame
        Ok(Some(src.split_to(length)))
    }
}

impl Encoder<&[u8]> for SbeFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: &[u8], dst: &mut BytesMut) -> Result<(), Self::Error> {
        // Validate frame size
        if item.len() > self.max_frame_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "frame too large: {} bytes exceeds maximum {} bytes",
                    item.len(),
                    self.max_frame_size
                ),
            ));
        }

        // Reserve space
        dst.reserve(4 + item.len());

        // Write length prefix (little-endian)
        dst.put_u32_le(item.len() as u32);

        // Write frame data
        dst.put_slice(item);

        Ok(())
    }
}

impl Encoder<BytesMut> for SbeFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: BytesMut, dst: &mut BytesMut) -> Result<(), Self::Error> {
        <Self as Encoder<&[u8]>>::encode(self, &item, dst)
    }
}

impl Encoder<Vec<u8>> for SbeFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Vec<u8>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        <Self as Encoder<&[u8]>>::encode(self, &item, dst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode() {
        let mut codec = SbeFrameCodec::new(1024);
        let mut buf = BytesMut::new();

        // Encode a frame
        let data = b"Hello, SBE!";
        codec.encode(data.as_slice(), &mut buf).unwrap();

        // Should have length prefix + data
        assert_eq!(buf.len(), 4 + data.len());

        // Decode the frame
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(&decoded[..], data);
    }

    #[test]
    fn test_partial_frame() {
        let mut codec = SbeFrameCodec::new(1024);
        let mut buf = BytesMut::new();

        // Write partial length
        buf.put_u8(10);
        buf.put_u8(0);

        // Should return None (incomplete)
        assert!(codec.decode(&mut buf).unwrap().is_none());

        // Complete the length
        buf.put_u8(0);
        buf.put_u8(0);

        // Still incomplete (no data)
        assert!(codec.decode(&mut buf).unwrap().is_none());

        // Add the data
        buf.put_slice(&[0u8; 10]);

        // Now should decode
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.len(), 10);
    }

    #[test]
    fn test_frame_too_large() {
        let mut codec = SbeFrameCodec::new(100);
        let mut buf = BytesMut::new();

        // Write a length that exceeds max
        buf.put_u32_le(200);

        let result = codec.decode(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_encode_too_large() {
        let mut codec = SbeFrameCodec::new(10);
        let mut buf = BytesMut::new();

        let data = [0u8; 20];
        let result = codec.encode(data.as_slice(), &mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_frames() {
        let mut codec = SbeFrameCodec::new(1024);
        let mut buf = BytesMut::new();

        // Encode multiple frames
        codec.encode(b"frame1".as_slice(), &mut buf).unwrap();
        codec.encode(b"frame2".as_slice(), &mut buf).unwrap();
        codec.encode(b"frame3".as_slice(), &mut buf).unwrap();

        // Decode them
        assert_eq!(&codec.decode(&mut buf).unwrap().unwrap()[..], b"frame1");
        assert_eq!(&codec.decode(&mut buf).unwrap().unwrap()[..], b"frame2");
        assert_eq!(&codec.decode(&mut buf).unwrap().unwrap()[..], b"frame3");
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }
}
