//! UDP multicast with A/B feed arbitration.

use bytes::Bytes;
use lru::LruCache;
use parking_lot::RwLock;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;

/// Configuration for multicast feed.
#[derive(Debug, Clone)]
pub struct MulticastConfig {
    /// Feed A multicast group address.
    pub feed_a_group: Ipv4Addr,
    /// Feed B multicast group address.
    pub feed_b_group: Ipv4Addr,
    /// Multicast port.
    pub port: u16,
    /// Network interface to use.
    pub interface: Ipv4Addr,
    /// Receive buffer size in bytes.
    pub recv_buffer_size: usize,
}

impl Default for MulticastConfig {
    fn default() -> Self {
        Self {
            feed_a_group: "239.1.1.1".parse().unwrap(),
            feed_b_group: "239.1.1.2".parse().unwrap(),
            port: 14310,
            interface: Ipv4Addr::UNSPECIFIED,
            recv_buffer_size: 8 * 1024 * 1024,
        }
    }
}

/// Packet with sequence number for arbitration.
#[derive(Debug, Clone)]
pub struct SequencedPacket {
    /// Sequence number.
    pub sequence: u64,
    /// Packet data (excluding sequence header).
    pub data: Bytes,
    /// Time when packet was received.
    pub recv_time: Instant,
}

/// A/B feed arbitrator for deduplication.
///
/// Tracks which sequence numbers have been processed to ensure
/// each message is only processed once across both feeds.
pub struct FeedArbitrator {
    /// Highest sequence seen.
    highest_seq: u64,
    /// Cache of processed sequences.
    processed: LruCache<u64, ()>,
    /// Expected next sequence.
    expected_seq: u64,
}

impl FeedArbitrator {
    /// Creates a new feed arbitrator.
    ///
    /// # Arguments
    /// * `cache_size` - Number of sequence numbers to track for deduplication
    #[must_use]
    pub fn new(cache_size: usize) -> Self {
        Self {
            highest_seq: 0,
            processed: LruCache::new(NonZeroUsize::new(cache_size).unwrap()),
            expected_seq: 1,
        }
    }

    /// Checks if a packet should be processed (first arrival wins).
    ///
    /// # Arguments
    /// * `seq` - Sequence number of the packet
    ///
    /// # Returns
    /// `true` if this is the first time seeing this sequence number.
    pub fn should_process(&mut self, seq: u64) -> bool {
        if self.processed.contains(&seq) {
            return false;
        }

        self.processed.put(seq, ());

        if seq > self.highest_seq {
            self.highest_seq = seq;
        }

        true
    }

    /// Checks for gaps in the sequence.
    ///
    /// # Arguments
    /// * `seq` - Current sequence number
    ///
    /// # Returns
    /// `Some((start, end))` if a gap was detected, `None` otherwise.
    pub fn check_gap(&mut self, seq: u64) -> Option<(u64, u64)> {
        if seq > self.expected_seq {
            let gap = (self.expected_seq, seq - 1);
            self.expected_seq = seq + 1;
            Some(gap)
        } else {
            if seq == self.expected_seq {
                self.expected_seq = seq + 1;
            }
            None
        }
    }

    /// Returns the highest sequence number seen.
    #[must_use]
    pub fn highest_sequence(&self) -> u64 {
        self.highest_seq
    }

    /// Returns the expected next sequence number.
    #[must_use]
    pub fn expected_sequence(&self) -> u64 {
        self.expected_seq
    }

    /// Resets the arbitrator state.
    pub fn reset(&mut self) {
        self.highest_seq = 0;
        self.expected_seq = 1;
        self.processed.clear();
    }
}

/// Multicast receiver with A/B feed arbitration.
pub struct MulticastReceiver {
    socket_a: Arc<UdpSocket>,
    socket_b: Arc<UdpSocket>,
    arbitrator: Arc<RwLock<FeedArbitrator>>,
}

impl MulticastReceiver {
    /// Creates a new multicast receiver.
    ///
    /// # Arguments
    /// * `config` - Multicast configuration
    ///
    /// # Errors
    /// Returns IO error if socket creation or multicast join fails.
    pub async fn new(config: MulticastConfig) -> std::io::Result<Self> {
        // Create and bind sockets
        let bind_addr: SocketAddr = (Ipv4Addr::UNSPECIFIED, config.port).into();

        let socket_a = UdpSocket::bind(bind_addr).await?;
        let socket_b = UdpSocket::bind(bind_addr).await?;

        // Join multicast groups
        socket_a.join_multicast_v4(config.feed_a_group, config.interface)?;
        socket_b.join_multicast_v4(config.feed_b_group, config.interface)?;

        Ok(Self {
            socket_a: Arc::new(socket_a),
            socket_b: Arc::new(socket_b),
            arbitrator: Arc::new(RwLock::new(FeedArbitrator::new(10000))),
        })
    }

    /// Receives the next unique packet (deduplicated across A/B feeds).
    ///
    /// # Errors
    /// Returns IO error if receive fails.
    pub async fn recv(&self) -> std::io::Result<SequencedPacket> {
        let mut buf_a = vec![0u8; 65536];
        let mut buf_b = vec![0u8; 65536];

        loop {
            tokio::select! {
                result = self.socket_a.recv(&mut buf_a) => {
                    let len = result?;
                    if let Some(packet) = self.process_packet(&buf_a[..len]) {
                        return Ok(packet);
                    }
                }
                result = self.socket_b.recv(&mut buf_b) => {
                    let len = result?;
                    if let Some(packet) = self.process_packet(&buf_b[..len]) {
                        return Ok(packet);
                    }
                }
            }
        }
    }

    /// Processes a received packet, returning it if it should be processed.
    fn process_packet(&self, data: &[u8]) -> Option<SequencedPacket> {
        // Extract sequence number from packet header (first 8 bytes)
        if data.len() < 8 {
            return None;
        }

        let seq = u64::from_le_bytes(data[0..8].try_into().unwrap());

        let mut arbitrator = self.arbitrator.write();
        if arbitrator.should_process(seq) {
            // Check for gaps
            if let Some((start, end)) = arbitrator.check_gap(seq) {
                tracing::warn!("Detected gap: {} - {}", start, end);
            }

            Some(SequencedPacket {
                sequence: seq,
                data: Bytes::copy_from_slice(&data[8..]),
                recv_time: Instant::now(),
            })
        } else {
            None // Duplicate, already processed from other feed
        }
    }

    /// Returns a reference to the arbitrator for statistics.
    #[must_use]
    pub fn arbitrator(&self) -> &Arc<RwLock<FeedArbitrator>> {
        &self.arbitrator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arbitrator_deduplication() {
        let mut arb = FeedArbitrator::new(100);

        // First time should process
        assert!(arb.should_process(1));
        // Second time should not
        assert!(!arb.should_process(1));

        // Different sequence should process
        assert!(arb.should_process(2));
    }

    #[test]
    fn test_arbitrator_gap_detection() {
        let mut arb = FeedArbitrator::new(100);

        // No gap for sequence 1
        assert!(arb.check_gap(1).is_none());

        // Gap detected for sequence 5 (missing 2, 3, 4)
        let gap = arb.check_gap(5);
        assert_eq!(gap, Some((2, 4)));

        // No gap for sequence 6
        assert!(arb.check_gap(6).is_none());
    }

    #[test]
    fn test_arbitrator_highest_sequence() {
        let mut arb = FeedArbitrator::new(100);

        arb.should_process(5);
        assert_eq!(arb.highest_sequence(), 5);

        arb.should_process(3);
        assert_eq!(arb.highest_sequence(), 5);

        arb.should_process(10);
        assert_eq!(arb.highest_sequence(), 10);
    }

    #[test]
    fn test_arbitrator_reset() {
        let mut arb = FeedArbitrator::new(100);

        arb.should_process(1);
        arb.should_process(2);

        arb.reset();

        assert_eq!(arb.highest_sequence(), 0);
        assert_eq!(arb.expected_sequence(), 1);
        // Should be able to process 1 again after reset
        assert!(arb.should_process(1));
    }
}
