// SPDX-License-Identifier: GPL-3.0-or-later

//! Real-network [`Transport`] implementation backed by libp2p.
//!
//! See [ADR-0010](../../../../doc/adr/0010-p2p-transport-libp2p.md)
//! and [ADR-0013](../../../../doc/adr/0013-sync-transport-trait.md).
//!
//! Architecture: the public `Libp2pTransport` is a small handle that
//! owns two channels — outbound `Cmd`s to a background tokio task, and
//! inbound request notifications. The background task owns the
//! `Swarm` and drives its event loop. The split keeps the trait surface
//! `Send + Sync` and serialises Swarm access through a single owner,
//! which is how libp2p expects you to drive it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use libp2p::core::ConnectedPoint;
use libp2p::futures::StreamExt;
use libp2p::request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel, cbor};
use libp2p::swarm::SwarmEvent;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::{Multiaddr, PeerId, StreamProtocol, Swarm, identify, mdns, noise, tcp, yamux};
use libp2p_identity::Keypair;
use tokio::sync::{Mutex, mpsc, oneshot};

use super::transport::{Reply, Transport};

/// libp2p protocol identifier for the bypass request-response channel.
pub const PROTOCOL: &str = "/bypass/sync/1";

/// libp2p identifier used by the `identify` behaviour. Lets remote
/// peers see we're a `bypass` instance and which version we speak.
const IDENTIFY_PROTOCOL: &str = "/bypass/identify/1";

#[derive(Debug, thiserror::Error)]
pub enum Libp2pError {
    #[error("libp2p: {0}")]
    Libp2p(String),

    #[error("outbound request to {peer} failed: {reason}")]
    OutboundFailure { peer: PeerId, reason: String },

    #[error("inbound request failed: {0}")]
    InboundFailure(String),

    #[error("swarm task exited before completing the request")]
    SwarmGone,
}

impl<E: std::fmt::Display> From<libp2p::TransportError<E>> for Libp2pError {
    fn from(e: libp2p::TransportError<E>) -> Self {
        Self::Libp2p(e.to_string())
    }
}

#[derive(libp2p::swarm::NetworkBehaviour)]
struct Behaviour {
    rr: cbor::Behaviour<Vec<u8>, Vec<u8>>,
    mdns: Toggle<mdns::tokio::Behaviour>,
    identify: identify::Behaviour,
}

/// Command channel: requests bypass code makes of the Swarm task.
enum Cmd {
    /// "Send this request to that peer; deliver the response here."
    Request {
        peer: PeerId,
        bytes: Vec<u8>,
        reply: oneshot::Sender<Result<Vec<u8>, Libp2pError>>,
    },
    /// "Bypass code received an inbound request and prepared a
    /// response; here it is, hand it to the Swarm."
    SendResponse { inbound_id: u64, bytes: Vec<u8> },
    /// "Dial this multiaddr." Used during pairing where the enter-side
    /// has been given the show-side's peer-id + listen address. Also
    /// registers the address with the request-response behaviour so
    /// subsequent `send_request` calls know where to find this peer.
    Dial {
        peer_id: PeerId,
        addr: Multiaddr,
        reply: oneshot::Sender<Result<(), Libp2pError>>,
    },
    /// Best-effort shutdown trigger; the Swarm task drops when the
    /// channel closes anyway, but this lets `Drop::drop` push a
    /// poison-pill so we don't wait for a tick.
    Shutdown,
}

/// mDNS discovery event surfaced to the daemon. The Swarm task fans
/// these out on a separate mpsc so `Transport::next_request` (which
/// blocks on inbound RPCs) and "I saw a peer on the LAN" notifications
/// don't queue behind each other.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    Discovered { peer: PeerId, addr: Multiaddr },
    Expired { peer: PeerId, addr: Multiaddr },
}

/// Real-network `Transport` impl. Each instance owns one libp2p Swarm
/// via a background tokio task.
pub struct Libp2pTransport {
    local_peer_id: PeerId,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    inbound_rx: Mutex<mpsc::UnboundedReceiver<(PeerId, Vec<u8>, u64)>>,
    listen_addrs: Arc<StdMutex<Vec<Multiaddr>>>,
    discoveries_rx: Mutex<mpsc::UnboundedReceiver<DiscoveryEvent>>,
}

impl Libp2pTransport {
    /// Build a new transport, listen on the given addresses, and start
    /// the Swarm event loop on a background tokio task. Returns once
    /// the Swarm has confirmed at least one listen address (or has
    /// fielded an error trying).
    ///
    /// `with_mdns = true` advertises this peer on LAN multicast; tests
    /// that dial directly want `false`.
    pub async fn new(
        identity: Keypair,
        listen: Vec<Multiaddr>,
        with_mdns: bool,
    ) -> Result<Self, Libp2pError> {
        let local_peer_id = PeerId::from(identity.public());

        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(identity)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )
            .map_err(|e| Libp2pError::Libp2p(format!("tcp transport: {e}")))?
            .with_behaviour(
                |kp| -> Result<Behaviour, Box<dyn std::error::Error + Send + Sync>> {
                    let rr = cbor::Behaviour::<Vec<u8>, Vec<u8>>::new(
                        [(StreamProtocol::new(PROTOCOL), ProtocolSupport::Full)],
                        request_response::Config::default(),
                    );
                    let mdns_inner = if with_mdns {
                        Some(mdns::tokio::Behaviour::new(
                            mdns::Config::default(),
                            kp.public().to_peer_id(),
                        )?)
                    } else {
                        None
                    };
                    let identify = identify::Behaviour::new(identify::Config::new(
                        IDENTIFY_PROTOCOL.into(),
                        kp.public(),
                    ));
                    Ok(Behaviour {
                        rr,
                        mdns: Toggle::from(mdns_inner),
                        identify,
                    })
                },
            )
            .map_err(|e| Libp2pError::Libp2p(format!("build behaviour: {e}")))?
            // Keep connections alive for a generous window — libp2p's
            // default is to close idle connections immediately, which
            // would close our pairing channel between the dial and the
            // first request. 60 s covers a slow human typing in a PIN.
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        for addr in &listen {
            swarm
                .listen_on(addr.clone())
                .map_err(|e| Libp2pError::Libp2p(format!("listen on {addr}: {e}")))?;
        }

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (discoveries_tx, discoveries_rx) = mpsc::unbounded_channel();
        let listen_addrs = Arc::new(StdMutex::new(Vec::<Multiaddr>::new()));
        let listen_addrs_for_task = Arc::clone(&listen_addrs);

        tokio::spawn(run_swarm(
            swarm,
            cmd_rx,
            inbound_tx,
            discoveries_tx,
            listen_addrs_for_task,
        ));

        Ok(Self {
            local_peer_id,
            cmd_tx,
            inbound_rx: Mutex::new(inbound_rx),
            listen_addrs,
            discoveries_rx: Mutex::new(discoveries_rx),
        })
    }

    /// Pull the next mDNS discovery / expiry event. Returns `None`
    /// when the Swarm task has shut down.
    pub async fn next_discovery(&self) -> Option<DiscoveryEvent> {
        let mut rx = self.discoveries_rx.lock().await;
        rx.recv().await
    }

    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    /// Snapshot of the listen-addr list as observed so far. Callers
    /// typically wait a short moment after `new` for the Swarm task to
    /// register listen addrs before calling this.
    pub fn listen_addrs(&self) -> Vec<Multiaddr> {
        self.listen_addrs
            .lock()
            .expect("listen_addrs mutex")
            .clone()
    }

    /// Dial a remote peer's multiaddr and register the address with
    /// the request-response behaviour so subsequent
    /// [`Transport::request`] calls to this peer-id can find it.
    /// Returns once the dial has been enqueued; connection completion
    /// is observed asynchronously.
    pub async fn dial(&self, peer_id: PeerId, addr: Multiaddr) -> Result<(), Libp2pError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Dial {
                peer_id,
                addr,
                reply: tx,
            })
            .map_err(|_| Libp2pError::SwarmGone)?;
        rx.await.map_err(|_| Libp2pError::SwarmGone)?
    }
}

impl Drop for Libp2pTransport {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Shutdown);
    }
}

impl Transport for Libp2pTransport {
    type PeerId = PeerId;
    type Error = Libp2pError;

    async fn request(&self, peer: &PeerId, req: Vec<u8>) -> Result<Vec<u8>, Libp2pError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Request {
                peer: *peer,
                bytes: req,
                reply: tx,
            })
            .map_err(|_| Libp2pError::SwarmGone)?;
        rx.await.map_err(|_| Libp2pError::SwarmGone)?
    }

    async fn next_request(&self) -> Result<(PeerId, Vec<u8>, Reply), Libp2pError> {
        let (peer, bytes, inbound_id) = {
            let mut rx = self.inbound_rx.lock().await;
            rx.recv().await.ok_or(Libp2pError::SwarmGone)?
        };
        let cmd_tx = self.cmd_tx.clone();
        let reply = Reply::from_fn(move |bytes| {
            let _ = cmd_tx.send(Cmd::SendResponse { inbound_id, bytes });
        });
        Ok((peer, bytes, reply))
    }
}

async fn run_swarm(
    mut swarm: Swarm<Behaviour>,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    inbound_tx: mpsc::UnboundedSender<(PeerId, Vec<u8>, u64)>,
    discoveries_tx: mpsc::UnboundedSender<DiscoveryEvent>,
    listen_addrs: Arc<StdMutex<Vec<Multiaddr>>>,
) {
    let mut pending_outbound: HashMap<
        OutboundRequestId,
        oneshot::Sender<Result<Vec<u8>, Libp2pError>>,
    > = HashMap::new();
    let mut pending_inbound: HashMap<u64, ResponseChannel<Vec<u8>>> = HashMap::new();
    let mut next_inbound_id: u64 = 0;

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_swarm_event(
                    event,
                    &mut pending_outbound,
                    &mut pending_inbound,
                    &mut next_inbound_id,
                    &inbound_tx,
                    &discoveries_tx,
                    &listen_addrs,
                );
            }
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Request { peer, bytes, reply }) => {
                    let req_id = swarm.behaviour_mut().rr.send_request(&peer, bytes);
                    pending_outbound.insert(req_id, reply);
                }
                Some(Cmd::SendResponse { inbound_id, bytes }) => {
                    if let Some(channel) = pending_inbound.remove(&inbound_id) {
                        // send_response returns the bytes back as `Err`
                        // if the inbound stream was closed in the
                        // meantime; nothing we can do, the peer has
                        // already given up.
                        let _ = swarm.behaviour_mut().rr.send_response(channel, bytes);
                    }
                }
                Some(Cmd::Dial { peer_id, addr, reply }) => {
                    swarm.add_peer_address(peer_id, addr.clone());
                    let _ = reply.send(
                        swarm
                            .dial(addr.clone())
                            .map_err(|e| Libp2pError::Libp2p(format!("dial {addr}: {e}"))),
                    );
                }
                Some(Cmd::Shutdown) | None => break,
            }
        }
    }

    // Fail any pending outbound requests so callers don't hang on Drop.
    for (_, tx) in pending_outbound.drain() {
        let _ = tx.send(Err(Libp2pError::SwarmGone));
    }
}

fn handle_swarm_event(
    event: SwarmEvent<BehaviourEvent>,
    pending_outbound: &mut HashMap<
        OutboundRequestId,
        oneshot::Sender<Result<Vec<u8>, Libp2pError>>,
    >,
    pending_inbound: &mut HashMap<u64, ResponseChannel<Vec<u8>>>,
    next_inbound_id: &mut u64,
    inbound_tx: &mpsc::UnboundedSender<(PeerId, Vec<u8>, u64)>,
    discoveries_tx: &mpsc::UnboundedSender<DiscoveryEvent>,
    listen_addrs: &Arc<StdMutex<Vec<Multiaddr>>>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            listen_addrs
                .lock()
                .expect("listen_addrs mutex")
                .push(address);
        }
        SwarmEvent::ConnectionEstablished { endpoint, .. } => {
            // Useful for debugging; quiet by default.
            let _ = match endpoint {
                ConnectedPoint::Dialer { .. } => "dialer",
                ConnectedPoint::Listener { .. } => "listener",
            };
        }
        SwarmEvent::Behaviour(BehaviourEvent::Rr(rr_event)) => match rr_event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    let id = *next_inbound_id;
                    *next_inbound_id += 1;
                    pending_inbound.insert(id, channel);
                    if inbound_tx.send((peer, request, id)).is_err() {
                        // No one listening — drop the inbound channel so the
                        // peer's request doesn't sit forever.
                        pending_inbound.remove(&id);
                    }
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    if let Some(reply) = pending_outbound.remove(&request_id) {
                        let _ = reply.send(Ok(response));
                    }
                }
            },
            request_response::Event::OutboundFailure {
                request_id,
                peer,
                error,
                ..
            } => {
                if let Some(reply) = pending_outbound.remove(&request_id) {
                    let _ = reply.send(Err(Libp2pError::OutboundFailure {
                        peer,
                        reason: error.to_string(),
                    }));
                }
            }
            request_response::Event::InboundFailure { .. } => {
                // Peer gave up on the request before we could reply;
                // nothing for us to do beyond the implicit cleanup
                // when send_response eventually drops the channel.
            }
            request_response::Event::ResponseSent { .. } => {}
        },
        SwarmEvent::Behaviour(BehaviourEvent::Mdns(ev)) => {
            // 5.2.c daemon uses these to auto-connect to paired peers
            // discovered on the LAN. We fan out every discovered /
            // expired tuple; the daemon filters by `peers.toml`.
            match ev {
                mdns::Event::Discovered(list) => {
                    for (peer, addr) in list {
                        let _ = discoveries_tx.send(DiscoveryEvent::Discovered { peer, addr });
                    }
                }
                mdns::Event::Expired(list) => {
                    for (peer, addr) in list {
                        let _ = discoveries_tx.send(DiscoveryEvent::Expired { peer, addr });
                    }
                }
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::Identify(_)) => {}
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn build_transport(kp: Keypair) -> (Libp2pTransport, Multiaddr) {
        let listen: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
        let t = Libp2pTransport::new(kp, vec![listen], /* with_mdns = */ false)
            .await
            .expect("build transport");
        // Wait for the listen-addr registration to land.
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let addrs = t.listen_addrs();
            if !addrs.is_empty() {
                return (t, addrs.into_iter().next().unwrap());
            }
        }
        panic!("timed out waiting for listen addr");
    }

    #[tokio::test]
    async fn two_libp2p_transports_exchange_request_and_reply() {
        let kp_a = Keypair::generate_ed25519();
        let kp_b = Keypair::generate_ed25519();
        let (a, _a_addr) = build_transport(kp_a).await;
        let (b, b_addr) = build_transport(kp_b).await;
        let b_peer_id = b.local_peer_id();

        // Compose the dial-target as `<addr>/p2p/<peer-id>` so libp2p
        // associates the dial with B's identity from the outset.
        use libp2p::multiaddr::Protocol;
        let mut full_addr = b_addr.clone();
        full_addr.push(Protocol::P2p(b_peer_id));

        let a_fut = async {
            a.dial(b_peer_id, full_addr).await.expect("dial");
            // Drive the dial→Noise→substream handshake by yielding.
            tokio::time::sleep(Duration::from_millis(500)).await;
            a.request(&b_peer_id, b"ping".to_vec())
                .await
                .expect("request")
        };
        let b_fut = async {
            let (from, msg, reply) = b.next_request().await.expect("inbound");
            assert_eq!(msg, b"ping");
            reply.send(format!("hello from b to {from}").into_bytes());
            // Hold `b` alive until A has received the response.
            tokio::time::sleep(Duration::from_millis(500)).await;
        };

        let (resp, ()) = tokio::join!(a_fut, b_fut);
        assert!(
            resp.starts_with(b"hello from b"),
            "got {:?}",
            String::from_utf8_lossy(&resp)
        );
    }
}
