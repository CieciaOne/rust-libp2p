// Copyright 2019 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

mod either;
mod external_addresses;
mod listen_addresses;
pub mod toggle;

pub use external_addresses::ExternalAddresses;
pub use listen_addresses::ListenAddresses;

use crate::connection::ConnectionId;
use crate::dial_opts::DialOpts;
use crate::listen_opts::ListenOpts;
use crate::{
    ConnectionDenied, ConnectionHandler, DialError, ListenError, THandler, THandlerInEvent,
    THandlerOutEvent,
};
use libp2p_core::{
    transport::{ListenerId, PortUse},
    ConnectedPoint, Endpoint, Multiaddr,
};
use libp2p_identity::PeerId;
use std::{task::Context, task::Poll};

/// A [`NetworkBehaviour`] defines the behaviour of the local node on the network.
///
/// In contrast to [`Transport`](libp2p_core::Transport) which defines **how** to send bytes on the
/// network, [`NetworkBehaviour`] defines **what** bytes to send and **to whom**.
///
/// Each protocol (e.g. `libp2p-ping`, `libp2p-identify` or `libp2p-kad`) implements
/// [`NetworkBehaviour`]. Multiple implementations of [`NetworkBehaviour`] can be composed into a
/// hierarchy of [`NetworkBehaviour`]s where parent implementations delegate to child
/// implementations. Finally the root of the [`NetworkBehaviour`] hierarchy is passed to
/// [`Swarm`](crate::Swarm) where it can then control the behaviour of the local node on a libp2p
/// network.
///
/// # Hierarchy of [`NetworkBehaviour`]
///
/// To compose multiple [`NetworkBehaviour`] implementations into a single [`NetworkBehaviour`]
/// implementation, potentially building a multi-level hierarchy of [`NetworkBehaviour`]s, one can
/// use one of the [`NetworkBehaviour`] combinators, and/or use the [`NetworkBehaviour`] derive
/// macro.
///
/// ## Combinators
///
/// [`NetworkBehaviour`] combinators wrap one or more [`NetworkBehaviour`] implementations and
/// implement [`NetworkBehaviour`] themselves. Example is the
/// [`Toggle`](crate::behaviour::toggle::Toggle) [`NetworkBehaviour`].
///
/// ``` rust
/// # use libp2p_swarm::dummy;
/// # use libp2p_swarm::behaviour::toggle::Toggle;
/// let my_behaviour = dummy::Behaviour;
/// let my_toggled_behaviour = Toggle::from(Some(my_behaviour));
/// ```
///
/// ## Custom [`NetworkBehaviour`] with the Derive Macro
///
/// One can derive [`NetworkBehaviour`] for a custom `struct` via the `#[derive(NetworkBehaviour)]`
/// proc macro re-exported by the `libp2p` crate. The macro generates a delegating `trait`
/// implementation for the custom `struct`. Each [`NetworkBehaviour`] trait method is simply
/// delegated to each `struct` member in the order the `struct` is defined. For example for
/// [`NetworkBehaviour::poll`] it will first poll the first `struct` member until it returns
/// [`Poll::Pending`] before moving on to later members.
///
/// Events ([`NetworkBehaviour::ToSwarm`]) returned by each `struct` member are wrapped in a new
/// `enum` event, with an `enum` variant for each `struct` member. Users can define this event
/// `enum` themselves and provide the name to the derive macro via `#[behaviour(to_swarm =
/// "MyCustomOutEvent")]`. If the user does not specify an `to_swarm`, the derive macro generates
/// the event definition itself, naming it `<STRUCT_NAME>Event`.
///
/// The aforementioned conversion of each of the event types generated by the struct members to the
/// custom `to_swarm` is handled by [`From`] implementations which the user needs to define in
/// addition to the event `enum` itself.
///
/// ``` rust
/// # use libp2p_identify as identify;
/// # use libp2p_ping as ping;
/// # use libp2p_swarm_derive::NetworkBehaviour;
/// #[derive(NetworkBehaviour)]
/// #[behaviour(to_swarm = "Event")]
/// # #[behaviour(prelude = "libp2p_swarm::derive_prelude")]
/// struct MyBehaviour {
///   identify: identify::Behaviour,
///   ping: ping::Behaviour,
/// }
///
/// enum Event {
///   Identify(identify::Event),
///   Ping(ping::Event),
/// }
///
/// impl From<identify::Event> for Event {
///   fn from(event: identify::Event) -> Self {
///     Self::Identify(event)
///   }
/// }
///
/// impl From<ping::Event> for Event {
///   fn from(event: ping::Event) -> Self {
///     Self::Ping(event)
///   }
/// }
/// ```
pub trait NetworkBehaviour: 'static {
    /// Handler for all the protocols the network behaviour supports.
    type ConnectionHandler: ConnectionHandler;

    /// Event generated by the `NetworkBehaviour` and that the swarm will report back.
    type ToSwarm: Send + 'static;

    /// Callback that is invoked for every new inbound connection.
    ///
    /// At this point in the connection lifecycle, only the remote's and our local address are known.
    /// We have also already allocated a [`ConnectionId`].
    ///
    /// Any error returned from this function will immediately abort the dial attempt.
    fn handle_pending_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        Ok(())
    }

    /// Callback that is invoked for every established inbound connection.
    ///
    /// This is invoked once another peer has successfully dialed us.
    ///
    /// At this point, we have verified their [`PeerId`] and we know, which particular [`Multiaddr`] succeeded in the dial.
    /// In order to actually use this connection, this function must return a [`ConnectionHandler`].
    /// Returning an error will immediately close the connection.
    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied>;

    /// Callback that is invoked for every outbound connection attempt.
    ///
    /// We have access to:
    ///
    /// - The [`PeerId`], if known. Remember that we can dial without a [`PeerId`].
    /// - All addresses passed to [`DialOpts`] are passed in here too.
    /// - The effective [`Role`](Endpoint) of this peer in the dial attempt. Typically, this is set to [`Endpoint::Dialer`] except if we are attempting a hole-punch.
    /// - The [`ConnectionId`] identifying the future connection resulting from this dial, if successful.
    ///
    /// Note that the addresses returned from this function are only used for dialing if [`WithPeerIdWithAddresses::extend_addresses_through_behaviour`](crate::dial_opts::WithPeerIdWithAddresses::extend_addresses_through_behaviour) is set.
    ///
    /// Any error returned from this function will immediately abort the dial attempt.
    fn handle_pending_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _maybe_peer: Option<PeerId>,
        _addresses: &[Multiaddr],
        _effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        Ok(vec![])
    }

    /// Callback that is invoked for every established outbound connection.
    ///
    /// This is invoked once we have successfully dialed a peer.
    /// At this point, we have verified their [`PeerId`] and we know, which particular [`Multiaddr`] succeeded in the dial.
    /// In order to actually use this connection, this function must return a [`ConnectionHandler`].
    /// Returning an error will immediately close the connection.
    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied>;

    /// Informs the behaviour about an event from the [`Swarm`](crate::Swarm).
    fn on_swarm_event(&mut self, event: FromSwarm);

    /// Informs the behaviour about an event generated by the [`ConnectionHandler`]
    /// dedicated to the peer identified by `peer_id`. for the behaviour.
    ///
    /// The [`PeerId`] is guaranteed to be in a connected state. In other words,
    /// [`FromSwarm::ConnectionEstablished`] has previously been received with this [`PeerId`].
    fn on_connection_handler_event(
        &mut self,
        _peer_id: PeerId,
        _connection_id: ConnectionId,
        _event: THandlerOutEvent<Self>,
    );

    /// Polls for things that swarm should do.
    ///
    /// This API mimics the API of the `Stream` trait. The method may register the current task in
    /// order to wake it up at a later point in time.
    fn poll(&mut self, cx: &mut Context<'_>)
        -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>>;
}

/// A command issued from a [`NetworkBehaviour`] for the [`Swarm`].
///
/// [`Swarm`]: super::Swarm
#[derive(Debug)]
#[non_exhaustive]
pub enum ToSwarm<TOutEvent, TInEvent> {
    /// Instructs the `Swarm` to return an event when it is being polled.
    GenerateEvent(TOutEvent),

    /// Instructs the swarm to start a dial.
    ///
    /// On success, [`NetworkBehaviour::on_swarm_event`] with `ConnectionEstablished` is invoked.
    /// On failure, [`NetworkBehaviour::on_swarm_event`] with `DialFailure` is invoked.
    ///
    /// [`DialOpts`] provides access to the [`ConnectionId`] via [`DialOpts::connection_id`].
    /// This [`ConnectionId`] will be used throughout the connection's lifecycle to associate events with it.
    /// This allows a [`NetworkBehaviour`] to identify a connection that resulted out of its own dial request.
    Dial { opts: DialOpts },

    /// Instructs the [`Swarm`](crate::Swarm) to listen on the provided address.
    ListenOn { opts: ListenOpts },

    /// Instructs the [`Swarm`](crate::Swarm) to remove the listener.
    RemoveListener { id: ListenerId },

    /// Instructs the `Swarm` to send an event to the handler dedicated to a
    /// connection with a peer.
    ///
    /// If the `Swarm` is connected to the peer, the message is delivered to the [`ConnectionHandler`]
    /// instance identified by the peer ID and connection ID.
    ///
    /// If the specified connection no longer exists, the event is silently dropped.
    ///
    /// Typically the connection ID given is the same as the one passed to
    /// [`NetworkBehaviour::on_connection_handler_event`], i.e. whenever the behaviour wishes to
    /// respond to a request on the same connection (and possibly the same
    /// substream, as per the implementation of [`ConnectionHandler`]).
    ///
    /// Note that even if the peer is currently connected, connections can get closed
    /// at any time and thus the event may not reach a handler.
    NotifyHandler {
        /// The peer for whom a [`ConnectionHandler`] should be notified.
        peer_id: PeerId,
        /// The options w.r.t. which connection handler to notify of the event.
        handler: NotifyHandler,
        /// The event to send.
        event: TInEvent,
    },

    /// Reports a **new** candidate for an external address to the [`Swarm`](crate::Swarm).
    ///
    /// The emphasis on a **new** candidate is important.
    /// Protocols MUST take care to only emit a candidate once per "source".
    /// For example, the observed address of a TCP connection does not change throughout its lifetime.
    /// Thus, only one candidate should be emitted per connection.    
    ///
    /// This makes the report frequency of an address a meaningful data-point for consumers of this event.
    /// This address will be shared with all [`NetworkBehaviour`]s via [`FromSwarm::NewExternalAddrCandidate`].
    ///
    /// This address could come from a variety of sources:
    /// - A protocol such as identify obtained it from a remote.
    /// - The user provided it based on configuration.
    /// - We made an educated guess based on one of our listen addresses.
    NewExternalAddrCandidate(Multiaddr),

    /// Indicates to the [`Swarm`](crate::Swarm) that the provided address is confirmed to be externally reachable.
    ///
    /// This is intended to be issued in response to a [`FromSwarm::NewExternalAddrCandidate`] if we are indeed externally reachable on this address.
    /// This address will be shared with all [`NetworkBehaviour`]s via [`FromSwarm::ExternalAddrConfirmed`].
    ExternalAddrConfirmed(Multiaddr),

    /// Indicates to the [`Swarm`](crate::Swarm) that we are no longer externally reachable under the provided address.
    ///
    /// This expires an address that was earlier confirmed via [`ToSwarm::ExternalAddrConfirmed`].
    /// This address will be shared with all [`NetworkBehaviour`]s via [`FromSwarm::ExternalAddrExpired`].
    ExternalAddrExpired(Multiaddr),

    /// Instructs the `Swarm` to initiate a graceful close of one or all connections with the given peer.
    ///
    /// Closing a connection via [`ToSwarm::CloseConnection`] will poll [`ConnectionHandler::poll_close`] to completion.
    /// In most cases, stopping to "use" a connection is enough to have it closed.
    /// The keep-alive algorithm will close a connection automatically once all [`ConnectionHandler`]s are idle.
    ///
    /// Use this command if you want to close a connection _despite_ it still being in use by one or more handlers.
    CloseConnection {
        /// The peer to disconnect.
        peer_id: PeerId,
        /// Whether to close a specific or all connections to the given peer.
        connection: CloseConnection,
    },
}

impl<TOutEvent, TInEventOld> ToSwarm<TOutEvent, TInEventOld> {
    /// Map the handler event.
    pub fn map_in<TInEventNew>(
        self,
        f: impl FnOnce(TInEventOld) -> TInEventNew,
    ) -> ToSwarm<TOutEvent, TInEventNew> {
        match self {
            ToSwarm::GenerateEvent(e) => ToSwarm::GenerateEvent(e),
            ToSwarm::Dial { opts } => ToSwarm::Dial { opts },
            ToSwarm::ListenOn { opts } => ToSwarm::ListenOn { opts },
            ToSwarm::RemoveListener { id } => ToSwarm::RemoveListener { id },
            ToSwarm::NotifyHandler {
                peer_id,
                handler,
                event,
            } => ToSwarm::NotifyHandler {
                peer_id,
                handler,
                event: f(event),
            },
            ToSwarm::CloseConnection {
                peer_id,
                connection,
            } => ToSwarm::CloseConnection {
                peer_id,
                connection,
            },
            ToSwarm::NewExternalAddrCandidate(addr) => ToSwarm::NewExternalAddrCandidate(addr),
            ToSwarm::ExternalAddrConfirmed(addr) => ToSwarm::ExternalAddrConfirmed(addr),
            ToSwarm::ExternalAddrExpired(addr) => ToSwarm::ExternalAddrExpired(addr),
        }
    }
}

impl<TOutEvent, THandlerIn> ToSwarm<TOutEvent, THandlerIn> {
    /// Map the event the swarm will return.
    pub fn map_out<E>(self, f: impl FnOnce(TOutEvent) -> E) -> ToSwarm<E, THandlerIn> {
        match self {
            ToSwarm::GenerateEvent(e) => ToSwarm::GenerateEvent(f(e)),
            ToSwarm::Dial { opts } => ToSwarm::Dial { opts },
            ToSwarm::ListenOn { opts } => ToSwarm::ListenOn { opts },
            ToSwarm::RemoveListener { id } => ToSwarm::RemoveListener { id },
            ToSwarm::NotifyHandler {
                peer_id,
                handler,
                event,
            } => ToSwarm::NotifyHandler {
                peer_id,
                handler,
                event,
            },
            ToSwarm::NewExternalAddrCandidate(addr) => ToSwarm::NewExternalAddrCandidate(addr),
            ToSwarm::ExternalAddrConfirmed(addr) => ToSwarm::ExternalAddrConfirmed(addr),
            ToSwarm::ExternalAddrExpired(addr) => ToSwarm::ExternalAddrExpired(addr),
            ToSwarm::CloseConnection {
                peer_id,
                connection,
            } => ToSwarm::CloseConnection {
                peer_id,
                connection,
            },
        }
    }
}

/// The options w.r.t. which connection handler to notify of an event.
#[derive(Debug, Clone)]
pub enum NotifyHandler {
    /// Notify a particular connection handler.
    One(ConnectionId),
    /// Notify an arbitrary connection handler.
    Any,
}

/// The options which connections to close.
#[derive(Debug, Clone, Default)]
pub enum CloseConnection {
    /// Disconnect a particular connection.
    One(ConnectionId),
    /// Disconnect all connections.
    #[default]
    All,
}

/// Enumeration with the list of the possible events
/// to pass to [`on_swarm_event`](NetworkBehaviour::on_swarm_event).
#[derive(Debug)]
#[non_exhaustive]
pub enum FromSwarm<'a> {
    /// Informs the behaviour about a newly established connection to a peer.
    ConnectionEstablished(ConnectionEstablished<'a>),
    /// Informs the behaviour about a closed connection to a peer.
    ///
    /// This event is always paired with an earlier
    /// [`FromSwarm::ConnectionEstablished`] with the same peer ID, connection ID
    /// and endpoint.
    ConnectionClosed(ConnectionClosed<'a>),
    /// Informs the behaviour that the [`ConnectedPoint`] of an existing
    /// connection has changed.
    AddressChange(AddressChange<'a>),
    /// Informs the behaviour that the dial to a known
    /// or unknown node failed.
    DialFailure(DialFailure<'a>),
    /// Informs the behaviour that an error
    /// happened on an incoming connection during its initial handshake.
    ///
    /// This can include, for example, an error during the handshake of the encryption layer, or the
    /// connection unexpectedly closed.
    ListenFailure(ListenFailure<'a>),
    /// Informs the behaviour that a new listener was created.
    NewListener(NewListener),
    /// Informs the behaviour that we have started listening on a new multiaddr.
    NewListenAddr(NewListenAddr<'a>),
    /// Informs the behaviour that a multiaddr
    /// we were listening on has expired,
    /// which means that we are no longer listening on it.
    ExpiredListenAddr(ExpiredListenAddr<'a>),
    /// Informs the behaviour that a listener experienced an error.
    ListenerError(ListenerError<'a>),
    /// Informs the behaviour that a listener closed.
    ListenerClosed(ListenerClosed<'a>),
    /// Informs the behaviour that we have discovered a new candidate for an external address for us.
    NewExternalAddrCandidate(NewExternalAddrCandidate<'a>),
    /// Informs the behaviour that an external address of the local node was confirmed.
    ExternalAddrConfirmed(ExternalAddrConfirmed<'a>),
    /// Informs the behaviour that an external address of the local node expired, i.e. is no-longer confirmed.
    ExternalAddrExpired(ExternalAddrExpired<'a>),
}

/// [`FromSwarm`] variant that informs the behaviour about a newly established connection to a peer.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionEstablished<'a> {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
    pub endpoint: &'a ConnectedPoint,
    pub failed_addresses: &'a [Multiaddr],
    pub other_established: usize,
}

/// [`FromSwarm`] variant that informs the behaviour about a closed connection to a peer.
///
/// This event is always paired with an earlier
/// [`FromSwarm::ConnectionEstablished`] with the same peer ID, connection ID
/// and endpoint.
#[derive(Debug)]
pub struct ConnectionClosed<'a> {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
    pub endpoint: &'a ConnectedPoint,
    pub remaining_established: usize,
}

/// [`FromSwarm`] variant that informs the behaviour that the [`ConnectedPoint`] of an existing
/// connection has changed.
#[derive(Debug, Clone, Copy)]
pub struct AddressChange<'a> {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
    pub old: &'a ConnectedPoint,
    pub new: &'a ConnectedPoint,
}

/// [`FromSwarm`] variant that informs the behaviour that the dial to a known
/// or unknown node failed.
#[derive(Debug, Clone, Copy)]
pub struct DialFailure<'a> {
    pub peer_id: Option<PeerId>,
    pub error: &'a DialError,
    pub connection_id: ConnectionId,
}

/// [`FromSwarm`] variant that informs the behaviour that an error
/// happened on an incoming connection during its initial handshake.
///
/// This can include, for example, an error during the handshake of the encryption layer, or the
/// connection unexpectedly closed.
#[derive(Debug, Clone, Copy)]
pub struct ListenFailure<'a> {
    pub local_addr: &'a Multiaddr,
    pub send_back_addr: &'a Multiaddr,
    pub error: &'a ListenError,
    pub connection_id: ConnectionId,
}

/// [`FromSwarm`] variant that informs the behaviour that a new listener was created.
#[derive(Debug, Clone, Copy)]
pub struct NewListener {
    pub listener_id: ListenerId,
}

/// [`FromSwarm`] variant that informs the behaviour
/// that we have started listening on a new multiaddr.
#[derive(Debug, Clone, Copy)]
pub struct NewListenAddr<'a> {
    pub listener_id: ListenerId,
    pub addr: &'a Multiaddr,
}

/// [`FromSwarm`] variant that informs the behaviour that a multiaddr
/// we were listening on has expired,
/// which means that we are no longer listening on it.
#[derive(Debug, Clone, Copy)]
pub struct ExpiredListenAddr<'a> {
    pub listener_id: ListenerId,
    pub addr: &'a Multiaddr,
}

/// [`FromSwarm`] variant that informs the behaviour that a listener experienced an error.
#[derive(Debug, Clone, Copy)]
pub struct ListenerError<'a> {
    pub listener_id: ListenerId,
    pub err: &'a (dyn std::error::Error + 'static),
}

/// [`FromSwarm`] variant that informs the behaviour that a listener closed.
#[derive(Debug, Clone, Copy)]
pub struct ListenerClosed<'a> {
    pub listener_id: ListenerId,
    pub reason: Result<(), &'a std::io::Error>,
}

/// [`FromSwarm`] variant that informs the behaviour about a new candidate for an external address for us.
#[derive(Debug, Clone, Copy)]
pub struct NewExternalAddrCandidate<'a> {
    pub addr: &'a Multiaddr,
}

/// [`FromSwarm`] variant that informs the behaviour that an external address was confirmed.
#[derive(Debug, Clone, Copy)]
pub struct ExternalAddrConfirmed<'a> {
    pub addr: &'a Multiaddr,
}

/// [`FromSwarm`] variant that informs the behaviour that an external address was removed.
#[derive(Debug, Clone, Copy)]
pub struct ExternalAddrExpired<'a> {
    pub addr: &'a Multiaddr,
}
