use std::pin::Pin;

use fleek_crypto::NodePublicKey;
use futures::stream::FuturesUnordered;
use futures::Future;
use fxhash::{FxHashMap, FxHashSet};
use lightning_interfaces::types::NodeIndex;
use lightning_interfaces::{ReceiverInterface, SenderInterface, SyncQueryRunnerInterface};

use crate::ev::Topology;
use crate::frame::{Digest, Frame};
use crate::receivers::Receivers;
use crate::MessageInternedId;

/// This struct is responsible for holding the state of the current peers
/// that we are connected to.
pub struct Peers<S: SenderInterface<Frame>, R: ReceiverInterface<Frame>> {
    stats: FxHashMap<NodeIndex, ConnectionStats>,
    /// Map each public key to the info we have about that peer.
    peers: FxHashMap<NodePublicKey, Peer<S>>,
    /// The message queue from all the connections we have.
    incoming_messages: Receivers<R>,
}

/// An interned id. But not from our interned table.
type RemoteInternedId = MessageInternedId;

struct Peer<S> {
    /// The index of the node.
    index: NodeIndex,
    /// The sender that we can use to send messages to this peer.
    sender: S,
    /// The origin of this connection can tell us if we started this connection or if
    /// the remote has dialed us.
    origin: ConnectionOrigin,
    /// The status of our connection with this peer.
    status: ConnectionStatus,
    // We know this peer has these messages. They have advertised them to us before.
    //
    // Here we are storing the mapping from the interned id of a message in our table,
    // to the interned id of the message known to the client.
    has: FxHashMap<MessageInternedId, RemoteInternedId>,
}

enum ConnectionOrigin {
    // We have established the connection.
    Us,
    /// The remote has dialed us and we have this connection because we got
    /// a connection from the listener.
    Remote,
}

#[derive(Debug, Clone, Copy)]
pub enum ConnectionStatus {
    /// The connection with the other peer is open.
    ///
    /// We are sending advertisements to this node and actively listening for their
    /// advertisements.
    Open,
    /// The connection with this node is closing. We do not wish to interact with it
    /// anymore. But since we may have pending communication with them. We are still
    /// keeping the connection alive.
    ///
    /// At this point we do not care about their advertisements. We only care about
    /// the messages they owe us. Once the other peer does not owe us anything anymore
    /// we close the connection.
    Closing,
    /// The connection with the other peer is closed and we are not communicating with
    /// the node.
    Closed,
}

/// A bunch of statistics that we gather from a peer throughout the life of the gossip.
#[derive(Default)]
pub struct ConnectionStats {
    /// How many things have we advertised to this node.
    pub advertisements_received_from_us: usize,
    /// How many things has this peer advertised to us.
    pub advertisements_received_from_peer: usize,
    /// How many `WANT`s have we sent to this node.
    pub wants_received_from_us: usize,
    /// How many `WANT`s has this peer sent our way.
    pub wants_received_from_peer: usize,
    /// Valid messages sent by this node to us.
    pub messages_received_from_peer: usize,
    /// Number of messages we have received from this peer that
    /// we did not continue propagating.
    pub invalid_messages_received_from_peer: usize,
}

impl<S: SenderInterface<Frame>, R: ReceiverInterface<Frame>> Peers<S, R> {
    /// Handle a new incoming connection. Should be provided along the index of the node until
    /// we have refactored everything to use the node index.
    pub async fn handle_new_incoming_connection(
        &mut self,
        index: NodeIndex,
        (sender, receiver): (S, R),
    ) {
        let pk = *sender.pk();

        if let Some(info) = self.peers.get(&pk) {
            // The connection already exists.
            // TODO(qti3e): Handle this.
        }

        let info = Peer {
            index,
            sender,
            origin: ConnectionOrigin::Remote,
            status: ConnectionStatus::Open,
            has: FxHashMap::default(),
        };

        self.stats.entry(index).or_default();
        self.peers.insert(pk, info);
        self.incoming_messages.push(receiver);
    }

    // Right now this looks awfully similar to the previous function. But not for long.
    pub async fn handle_new_outgoing_connection(
        &mut self,
        index: NodeIndex,
        (sender, receiver): (S, R),
    ) {
        let pk = *sender.pk();

        if let Some(info) = self.peers.get(&pk) {
            // The connection already exists.
            // TODO(qti3e): Handle this.
        }

        let info = Peer {
            index,
            sender,
            origin: ConnectionOrigin::Us,
            status: ConnectionStatus::Open,
            has: FxHashMap::default(),
        };

        self.stats.entry(index).or_default();
        self.peers.insert(pk, info);
        self.incoming_messages.push(receiver);
    }

    pub async fn recv(&mut self) -> Option<(NodePublicKey, Frame)> {
        match self.incoming_messages.recv().await {
            Some((r, None)) => {
                self.disconnected(r.pk());
                None
            },
            Some((receiver, Some(frame))) => self.handle_frame(receiver, frame),
            None => None,
        }
    }

    #[inline(always)]
    fn disconnected(&mut self, pk: &NodePublicKey) {
        if let Some(mut info) = self.peers.remove(pk) {
            info.status = ConnectionStatus::Closed;
        }
    }

    #[inline(always)]
    fn handle_frame(&mut self, receiver: R, frame: Frame) -> Option<(NodePublicKey, Frame)> {
        let pk = *receiver.pk();
        let info = self.peers.get(&pk)?;

        // Update the stats on what we got.
        let mut stats = self.stats.get_mut(&info.index).unwrap();
        match frame {
            Frame::Advr(_) => stats.advertisements_received_from_peer += 1,
            Frame::Want(_) => stats.wants_received_from_peer += 1,
            Frame::Message(_) => stats.messages_received_from_peer += 1,
        }

        match (info.status, frame) {
            (ConnectionStatus::Open, frame) => {
                self.incoming_messages.push(receiver);
                Some((pk, frame))
            },
            (ConnectionStatus::Closed, _) => None,
            (ConnectionStatus::Closing, Frame::Message(msg)) => {
                if self.keep_alive(&pk) {
                    self.incoming_messages.push(receiver);
                }
                Some((pk, Frame::Message(msg)))
            },
            // If we're closing the connection with this peer, we don't care about
            // anything other than the actual payloads that they are still sending
            // our way.
            (ConnectionStatus::Closing, _) => {
                if self.keep_alive(&pk) {
                    self.incoming_messages.push(receiver);
                }
                None
            },
        }
    }

    /// Based on the number of messages the other node owes us. And the time elapsed since
    /// we decided to close the connection, returns a boolean indicating whether or not we
    /// are interested in keeping this connection going and if we should still keep listening
    /// for a message to come.
    #[inline(always)]
    fn keep_alive(&self, pk: &NodePublicKey) -> bool {
        // TODO(qti3e): Implement this function.
        true
    }
}

impl<S: SenderInterface<Frame>, R: ReceiverInterface<Frame>> Default for Peers<S, R> {
    fn default() -> Self {
        Self {
            stats: Default::default(),
            peers: Default::default(),
            incoming_messages: Default::default(),
        }
    }
}
