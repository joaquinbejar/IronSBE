//! UDP transport module.
//!
//! Provides UDP unicast and multicast implementations with A/B feed arbitration.

pub mod multicast;
pub mod unicast;

pub use multicast::{FeedArbitrator, MulticastConfig, MulticastReceiver, SequencedPacket};
pub use unicast::{UdpReceiver, UdpSender};
