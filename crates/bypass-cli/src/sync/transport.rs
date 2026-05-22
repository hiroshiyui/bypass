// SPDX-License-Identifier: GPL-3.0-or-later

//! Request-response transport seam between sync code and the network.
//!
//! [ADR-0013](../../../../doc/adr/0013-sync-transport-trait.md):
//!
//! - One [`Transport`] trait, two implementations.
//! - [`InProcessTransport`] (this module): paired in-memory channels
//!   for unit tests. No network, no flake.
//! - `Libp2pTransport` (5.2.b): real libp2p over Noise.
//!
//! The trait shape matches libp2p's `request-response` protocol 1:1, so
//! the real adapter is thin and the orchestration code above the trait
//! is identical regardless of which side it's running on.

use std::fmt;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc, oneshot};

/// A peer identifier the transport knows about. Generic so
/// `InProcessTransport` can use a lightweight ID for tests and the
/// future `Libp2pTransport` can plug in `libp2p::PeerId`.
pub trait PeerIdLike:
    Clone + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static
{
}

impl<T> PeerIdLike for T where
    T: Clone + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static
{
}

/// A request-response transport. Symmetric: each peer can both
/// originate requests via [`request`](Transport::request) and accept
/// inbound requests via [`next_request`](Transport::next_request).
///
/// Errors are an associated type so implementations can carry rich
/// failure detail
/// ([ADR-0006](../../../../doc/adr/0006-trait-associated-error-types.md)).
pub trait Transport: Send + Sync + 'static {
    type PeerId: PeerIdLike;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Send `req` to `peer`, await their response.
    fn request(
        &self,
        peer: &Self::PeerId,
        req: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> + Send;

    /// Block until the next inbound request arrives. The returned
    /// [`Reply`] handle is fulfilled by the caller to send the
    /// response back over the same logical connection.
    fn next_request(
        &self,
    ) -> impl std::future::Future<Output = Result<(Self::PeerId, Vec<u8>, Reply), Self::Error>> + Send;
}

/// One-shot handle the inbound-handler uses to reply.
#[derive(Debug)]
pub struct Reply(oneshot::Sender<Vec<u8>>);

impl Reply {
    pub fn send(self, bytes: Vec<u8>) {
        // `send` returns Err only if the requester dropped its receiver
        // (i.e. timed out or cancelled). Nothing for us to do — the
        // peer will see a timeout on their side.
        let _ = self.0.send(bytes);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InProcessError {
    #[error("peer disconnected")]
    Disconnected,

    #[error("peer {0:?} not paired with this transport")]
    UnknownPeer(String),

    #[error("request was dropped before a response arrived")]
    NoReply,
}

/// In-flight message on the wire of an [`InProcessTransport`]: the
/// sender's id, the request bytes, and a oneshot for the response.
type Wire = (String, Vec<u8>, oneshot::Sender<Vec<u8>>);

/// Test fake: two `InProcessTransport`s connected via channels.
///
/// Construct a pair with [`InProcessTransport::pair`]. Each half knows
/// the other's `PeerId` and can talk to it via `request`. Inbound
/// requests are surfaced via `next_request`.
pub struct InProcessTransport {
    self_id: String,
    /// Channel for sending outbound requests to the peer. The other
    /// half's `next_request` reads from this channel via its
    /// `inbound`.
    outbound: mpsc::UnboundedSender<Wire>,
    /// Channel for receiving inbound requests destined for us.
    inbound: Arc<Mutex<mpsc::UnboundedReceiver<Wire>>>,
    /// Peer's id (the other half of the pair).
    peer_id: String,
}

impl fmt::Debug for InProcessTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InProcessTransport")
            .field("self_id", &self.self_id)
            .field("peer_id", &self.peer_id)
            .finish()
    }
}

impl InProcessTransport {
    /// Build a connected pair. Each half can `request` from the other,
    /// and each surfaces inbound requests via `next_request`.
    pub fn pair(a_id: impl Into<String>, b_id: impl Into<String>) -> (Self, Self) {
        let a_id = a_id.into();
        let b_id = b_id.into();
        // A → B
        let (a_to_b_tx, a_to_b_rx) = mpsc::unbounded_channel();
        // B → A
        let (b_to_a_tx, b_to_a_rx) = mpsc::unbounded_channel();
        let a = Self {
            self_id: a_id.clone(),
            outbound: a_to_b_tx,
            inbound: Arc::new(Mutex::new(b_to_a_rx)),
            peer_id: b_id.clone(),
        };
        let b = Self {
            self_id: b_id,
            outbound: b_to_a_tx,
            inbound: Arc::new(Mutex::new(a_to_b_rx)),
            peer_id: a_id,
        };
        (a, b)
    }

    pub fn self_id(&self) -> &str {
        &self.self_id
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }
}

impl Transport for InProcessTransport {
    type PeerId = String;
    type Error = InProcessError;

    async fn request(&self, peer: &Self::PeerId, req: Vec<u8>) -> Result<Vec<u8>, Self::Error> {
        if peer != &self.peer_id {
            return Err(InProcessError::UnknownPeer(peer.clone()));
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.outbound
            .send((self.self_id.clone(), req, reply_tx))
            .map_err(|_| InProcessError::Disconnected)?;
        reply_rx.await.map_err(|_| InProcessError::NoReply)
    }

    async fn next_request(&self) -> Result<(Self::PeerId, Vec<u8>, Reply), Self::Error> {
        let (from, bytes, reply_tx) = {
            let mut rx = self.inbound.lock().await;
            rx.recv().await.ok_or(InProcessError::Disconnected)?
        };
        Ok((from, bytes, Reply(reply_tx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn paired_transports_can_exchange_a_request_and_reply() {
        let (a, b) = InProcessTransport::pair("alice", "bob");

        // Bob handles whatever Alice sends.
        let handler = tokio::spawn(async move {
            let (from, msg, reply) = b.next_request().await.unwrap();
            assert_eq!(from, "alice");
            assert_eq!(msg, b"ping");
            reply.send(b"pong".to_vec());
        });

        let response = a
            .request(&"bob".to_string(), b"ping".to_vec())
            .await
            .unwrap();
        assert_eq!(response, b"pong");
        handler.await.unwrap();
    }

    #[tokio::test]
    async fn request_to_unknown_peer_errors() {
        let (a, _b) = InProcessTransport::pair("alice", "bob");
        let err = a
            .request(&"charlie".to_string(), b"x".to_vec())
            .await
            .unwrap_err();
        assert!(matches!(err, InProcessError::UnknownPeer(_)));
    }

    #[tokio::test]
    async fn dropping_the_peer_disconnects() {
        let (a, b) = InProcessTransport::pair("alice", "bob");
        drop(b);
        let err = a
            .request(&"bob".to_string(), b"x".to_vec())
            .await
            .unwrap_err();
        assert!(matches!(err, InProcessError::Disconnected));
    }

    #[tokio::test]
    async fn reply_drop_surfaces_as_no_reply() {
        let (a, b) = InProcessTransport::pair("alice", "bob");

        let handler = tokio::spawn(async move {
            let (_from, _msg, reply) = b.next_request().await.unwrap();
            drop(reply); // peer accepted the request but dropped without replying
        });

        let err = a
            .request(&"bob".to_string(), b"ping".to_vec())
            .await
            .unwrap_err();
        assert!(matches!(err, InProcessError::NoReply));
        handler.await.unwrap();
    }
}
