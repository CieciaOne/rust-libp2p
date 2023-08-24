// TODO: Should we have a timeout on transport?
// TODO: Be able to address `SwarmBuilder` configuration methods.
// TODO: Consider making with_other_transport fallible.

use libp2p_core::{muxing::StreamMuxerBox, Transport};
use libp2p_swarm::NetworkBehaviour;
use std::convert::Infallible;
use std::io;
use std::marker::PhantomData;

pub struct SwarmBuilder<Provider, Phase> {
    keypair: libp2p_identity::Keypair,
    phantom: PhantomData<Provider>,
    phase: Phase,
}

pub struct InitialPhase {}

impl SwarmBuilder<NoProviderSpecified, InitialPhase> {
    pub fn with_new_identity() -> SwarmBuilder<NoProviderSpecified, ProviderPhase> {
        SwarmBuilder::with_existing_identity(libp2p_identity::Keypair::generate_ed25519())
    }

    pub fn with_existing_identity(
        keypair: libp2p_identity::Keypair,
    ) -> SwarmBuilder<NoProviderSpecified, ProviderPhase> {
        SwarmBuilder {
            keypair,
            phantom: PhantomData,
            phase: ProviderPhase {},
        }
    }
}

pub struct ProviderPhase {}

impl SwarmBuilder<NoProviderSpecified, ProviderPhase> {
    #[cfg(feature = "async-std")]
    pub fn with_async_std(self) -> SwarmBuilder<AsyncStd, TcpPhase> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: TcpPhase {},
        }
    }

    #[cfg(feature = "tokio")]
    pub fn with_tokio(self) -> SwarmBuilder<AsyncStd, TcpPhase> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: TcpPhase {},
        }
    }
}

pub struct TcpPhase {}

#[cfg(feature = "tcp")]
impl<Provider> SwarmBuilder<Provider, TcpPhase> {
    pub fn with_tcp(self) -> SwarmBuilder<Provider, TcpTlsPhase> {
        self.with_tcp_config(Default::default())
    }

    pub fn with_tcp_config(
        self,
        config: libp2p_tcp::Config,
    ) -> SwarmBuilder<Provider, TcpTlsPhase> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: TcpTlsPhase { config },
        }
    }
}

impl<Provider> SwarmBuilder<Provider, TcpPhase> {
    // TODO: This would allow one to build a faulty transport.
    fn without_tcp(
        self,
    ) -> SwarmBuilder<Provider, QuicPhase<impl AuthenticatedMultiplexedTransport>> {
        SwarmBuilder {
            // TODO: Is this a good idea in a production environment? Unfortunately I don't know a
            // way around it. One can not define two `with_relay` methods, one with a real transport
            // using OrTransport, one with a fake transport discarding it right away.
            keypair: self.keypair,
            phantom: PhantomData,
            phase: QuicPhase {
                transport: libp2p_core::transport::dummy::DummyTransport::new(),
            },
        }
    }
}

// Shortcuts
#[cfg(all(feature = "quic", feature = "async-std"))]
impl SwarmBuilder<AsyncStd, TcpPhase> {
    pub fn with_quic(
        self,
    ) -> SwarmBuilder<AsyncStd, OtherTransportPhase<impl AuthenticatedMultiplexedTransport>> {
        self.without_tcp().with_quic()
    }
}
#[cfg(all(feature = "quic", feature = "tokio"))]
impl SwarmBuilder<Tokio, TcpPhase> {
    pub fn with_quic(
        self,
    ) -> SwarmBuilder<Tokio, OtherTransportPhase<impl AuthenticatedMultiplexedTransport>> {
        self.without_tcp().with_quic()
    }
}
impl<Provider> SwarmBuilder<Provider, TcpPhase> {
    pub fn with_other_transport<OtherTransport: AuthenticatedMultiplexedTransport>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair) -> OtherTransport,
    ) -> SwarmBuilder<Provider, OtherTransportPhase<impl AuthenticatedMultiplexedTransport>> {
        self.without_tcp()
            .without_quic()
            .with_other_transport(constructor)
    }
}

#[cfg(feature = "tcp")]
pub struct TcpTlsPhase {
    config: libp2p_tcp::Config,
}

#[cfg(feature = "tcp")]
impl<Provider> SwarmBuilder<Provider, TcpTlsPhase> {
    #[cfg(feature = "tls")]
    pub fn with_tls(self) -> SwarmBuilder<Provider, TcpNoisePhase<Tls>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: TcpNoisePhase {
                config: self.phase.config,
                phantom: PhantomData,
            },
        }
    }

    fn without_tls(self) -> SwarmBuilder<Provider, TcpNoisePhase<WithoutTls>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: TcpNoisePhase {
                config: self.phase.config,
                phantom: PhantomData,
            },
        }
    }
}

// Shortcuts
#[cfg(all(feature = "tcp", feature = "noise", feature = "async-std"))]
impl SwarmBuilder<AsyncStd, TcpTlsPhase> {
    #[cfg(feature = "noise")]
    pub fn with_noise(
        self,
    ) -> Result<
        SwarmBuilder<AsyncStd, QuicPhase<impl AuthenticatedMultiplexedTransport>>,
        AuthenticationError,
    > {
        self.without_tls().with_noise()
    }
}
#[cfg(all(feature = "tcp", feature = "noise", feature = "tokio"))]
impl SwarmBuilder<Tokio, TcpTlsPhase> {
    #[cfg(feature = "noise")]
    pub fn with_noise(
        self,
    ) -> Result<
        SwarmBuilder<Tokio, QuicPhase<impl AuthenticatedMultiplexedTransport>>,
        AuthenticationError,
    > {
        self.without_tls().with_noise()
    }
}

#[cfg(feature = "tcp")]
pub struct TcpNoisePhase<A> {
    config: libp2p_tcp::Config,
    phantom: PhantomData<A>,
}

#[cfg(feature = "tcp")]
macro_rules! construct_quic_builder {
    ($self:ident, $tcp:ident, $auth:expr) => {
        Ok(SwarmBuilder {
            phase: QuicPhase {
                transport: libp2p_tcp::$tcp::Transport::new($self.phase.config)
                    .upgrade(libp2p_core::upgrade::Version::V1Lazy)
                    .authenticate($auth)
                    .multiplex(libp2p_yamux::Config::default())
                    .map(|(p, c), _| (p, StreamMuxerBox::new(c))),
            },
            keypair: $self.keypair,
            phantom: PhantomData,
        })
    };
}

macro_rules! impl_tcp_noise_builder {
    ($providerKebabCase:literal, $providerCamelCase:ident, $tcp:ident) => {
        #[cfg(all(feature = $providerKebabCase, feature = "tcp", feature = "tls"))]
        impl SwarmBuilder<$providerCamelCase, TcpNoisePhase<Tls>> {
            #[cfg(feature = "noise")]
            pub fn with_noise(
                self,
            ) -> Result<
                SwarmBuilder<$providerCamelCase, QuicPhase<impl AuthenticatedMultiplexedTransport>>,
                AuthenticationError,
            > {
                construct_quic_builder!(
                    self,
                    $tcp,
                    libp2p_core::upgrade::Map::new(
                        libp2p_core::upgrade::SelectUpgrade::new(
                            libp2p_tls::Config::new(&self.keypair)?,
                            libp2p_noise::Config::new(&self.keypair)?,
                        ),
                        |upgrade| match upgrade {
                            futures::future::Either::Left((peer_id, upgrade)) => {
                                (peer_id, futures::future::Either::Left(upgrade))
                            }
                            futures::future::Either::Right((peer_id, upgrade)) => {
                                (peer_id, futures::future::Either::Right(upgrade))
                            }
                        },
                    )
                )
            }

            pub fn without_noise(
                self,
            ) -> Result<
                SwarmBuilder<$providerCamelCase, QuicPhase<impl AuthenticatedMultiplexedTransport>>,
                AuthenticationError,
            > {
                construct_quic_builder!(self, $tcp, libp2p_tls::Config::new(&self.keypair)?)
            }
        }

        #[cfg(feature = $providerKebabCase)]
        impl SwarmBuilder<$providerCamelCase, TcpNoisePhase<WithoutTls>> {
            #[cfg(feature = "noise")]
            pub fn with_noise(
                self,
            ) -> Result<
                SwarmBuilder<$providerCamelCase, QuicPhase<impl AuthenticatedMultiplexedTransport>>,
                AuthenticationError,
            > {
                construct_quic_builder!(self, $tcp, libp2p_noise::Config::new(&self.keypair)?)
            }
        }
    };
}

impl_tcp_noise_builder!("async-std", AsyncStd, async_io);
impl_tcp_noise_builder!("tokio", Tokio, tokio);

#[cfg(feature = "tls")]
pub enum Tls {}

pub enum WithoutTls {}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    #[error("Tls")]
    #[cfg(feature = "tls")]
    Tls(#[from] libp2p_tls::certificate::GenError),
    #[error("Noise")]
    #[cfg(feature = "noise")]
    Noise(#[from] libp2p_noise::Error),
}

pub struct QuicPhase<T> {
    transport: T,
}

#[cfg(all(feature = "quic", feature = "async-std"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<AsyncStd, QuicPhase<T>> {
    pub fn with_quic(
        self,
    ) -> SwarmBuilder<AsyncStd, OtherTransportPhase<impl AuthenticatedMultiplexedTransport>> {
        SwarmBuilder {
            phase: OtherTransportPhase {
                transport: self
                    .phase
                    .transport
                    .or_transport(
                        libp2p_quic::async_std::Transport::new(libp2p_quic::Config::new(
                            &self.keypair,
                        ))
                        .map(|(peer_id, muxer), _| (peer_id, StreamMuxerBox::new(muxer))),
                    )
                    .map(|either, _| either.into_inner()),
            },
            keypair: self.keypair,
            phantom: PhantomData,
        }
    }
}

#[cfg(all(feature = "quic", feature = "tokio"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<Tokio, QuicPhase<T>> {
    pub fn with_quic(
        self,
    ) -> SwarmBuilder<Tokio, OtherTransportPhase<impl AuthenticatedMultiplexedTransport>> {
        SwarmBuilder {
            phase: OtherTransportPhase {
                transport: self
                    .phase
                    .transport
                    .or_transport(
                        libp2p_quic::tokio::Transport::new(libp2p_quic::Config::new(&self.keypair))
                            .map(|(peer_id, muxer), _| (peer_id, StreamMuxerBox::new(muxer))),
                    )
                    .map(|either, _| either.into_inner()),
            },
            keypair: self.keypair,
            phantom: PhantomData,
        }
    }
}

impl<Provider, T> SwarmBuilder<Provider, QuicPhase<T>> {
    fn without_quic(self) -> SwarmBuilder<Provider, OtherTransportPhase<T>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: OtherTransportPhase {
                transport: self.phase.transport,
            },
        }
    }
}

// Shortcuts
impl<Provider, T: AuthenticatedMultiplexedTransport> SwarmBuilder<Provider, QuicPhase<T>> {
    #[cfg(feature = "relay")]
    pub fn with_relay(self) -> SwarmBuilder<Provider, RelayTlsPhase<T>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: RelayTlsPhase {
                transport: self.phase.transport,
            },
        }
    }

    pub fn with_other_transport<OtherTransport: AuthenticatedMultiplexedTransport>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair) -> OtherTransport,
    ) -> SwarmBuilder<Provider, OtherTransportPhase<impl AuthenticatedMultiplexedTransport>> {
        self.without_quic().with_other_transport(constructor)
    }

    #[cfg(feature = "websocket")]
    pub fn with_websocket(
        self,
    ) -> SwarmBuilder<
        Provider,
        WebsocketTlsPhase<impl AuthenticatedMultiplexedTransport, NoRelayBehaviour>,
    > {
        self.without_quic()
            .without_any_other_transports()
            .without_dns()
            .without_relay()
            .with_websocket()
    }

    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        self.without_quic()
            .without_any_other_transports()
            .without_dns()
            .without_relay()
            .without_websocket()
            .with_behaviour(constructor)
    }
}
#[cfg(all(feature = "async-std", feature = "dns"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<AsyncStd, QuicPhase<T>> {
    pub async fn with_dns(
        self,
    ) -> Result<SwarmBuilder<AsyncStd, RelayPhase<impl AuthenticatedMultiplexedTransport>>, io::Error>
    {
        self.without_quic()
            .without_any_other_transports()
            .with_dns()
            .await
    }
}
#[cfg(all(feature = "tokio", feature = "dns"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<Tokio, QuicPhase<T>> {
    pub fn with_dns(
        self,
    ) -> Result<SwarmBuilder<Tokio, RelayPhase<impl AuthenticatedMultiplexedTransport>>, io::Error>
    {
        self.without_quic()
            .without_any_other_transports()
            .with_dns()
    }
}

pub struct OtherTransportPhase<T> {
    transport: T,
}

impl<Provider, T: AuthenticatedMultiplexedTransport>
    SwarmBuilder<Provider, OtherTransportPhase<T>>
{
    pub fn with_other_transport<OtherTransport: AuthenticatedMultiplexedTransport>(
        self,
        mut constructor: impl FnMut(&libp2p_identity::Keypair) -> OtherTransport,
    ) -> SwarmBuilder<Provider, OtherTransportPhase<impl AuthenticatedMultiplexedTransport>> {
        SwarmBuilder {
            phase: OtherTransportPhase {
                transport: self
                    .phase
                    .transport
                    .or_transport(constructor(&self.keypair))
                    .map(|either, _| either.into_inner()),
            },
            keypair: self.keypair,
            phantom: PhantomData,
        }
    }

    // TODO: Not the ideal name.
    fn without_any_other_transports(
        self,
    ) -> SwarmBuilder<Provider, DnsPhase<impl AuthenticatedMultiplexedTransport>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: DnsPhase {
                transport: self.phase.transport,
            },
        }
    }
}

// Shortcuts
#[cfg(all(feature = "async-std", feature = "dns"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<AsyncStd, OtherTransportPhase<T>> {
    pub async fn with_dns(
        self,
    ) -> Result<SwarmBuilder<AsyncStd, RelayPhase<impl AuthenticatedMultiplexedTransport>>, io::Error>
    {
        self.without_any_other_transports().with_dns().await
    }
}
#[cfg(all(feature = "tokio", feature = "dns"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<Tokio, OtherTransportPhase<T>> {
    pub fn with_dns(
        self,
    ) -> Result<SwarmBuilder<Tokio, RelayPhase<impl AuthenticatedMultiplexedTransport>>, io::Error>
    {
        self.without_any_other_transports().with_dns()
    }
}
#[cfg(feature = "relay")]
impl<T: AuthenticatedMultiplexedTransport, Provider>
    SwarmBuilder<Provider, OtherTransportPhase<T>>
{
    pub fn with_relay(
        self,
    ) -> SwarmBuilder<Provider, RelayTlsPhase<impl AuthenticatedMultiplexedTransport>> {
        self.without_any_other_transports()
            .without_dns()
            .with_relay()
    }
}
impl<Provider, T: AuthenticatedMultiplexedTransport>
    SwarmBuilder<Provider, OtherTransportPhase<T>>
{
    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        self.without_any_other_transports()
            .without_dns()
            .without_relay()
            .without_websocket()
            .with_behaviour(constructor)
    }
}

pub struct DnsPhase<T> {
    transport: T,
}

#[cfg(all(feature = "async-std", feature = "dns"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<AsyncStd, DnsPhase<T>> {
    pub async fn with_dns(
        self,
    ) -> Result<SwarmBuilder<AsyncStd, RelayPhase<impl AuthenticatedMultiplexedTransport>>, io::Error>
    {
        Ok(SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: RelayPhase {
                transport: libp2p_dns::DnsConfig::system(self.phase.transport).await?,
            },
        })
    }
}

#[cfg(all(feature = "tokio", feature = "dns"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<Tokio, DnsPhase<T>> {
    pub fn with_dns(
        self,
    ) -> Result<SwarmBuilder<Tokio, RelayPhase<impl AuthenticatedMultiplexedTransport>>, io::Error>
    {
        Ok(SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: RelayPhase {
                transport: libp2p_dns::TokioDnsConfig::system(self.phase.transport)?,
            },
        })
    }
}

impl<Provider, T> SwarmBuilder<Provider, DnsPhase<T>> {
    fn without_dns(self) -> SwarmBuilder<Provider, RelayPhase<T>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: RelayPhase {
                // TODO: Timeout needed?
                transport: self.phase.transport,
            },
        }
    }
}

// Shortcuts
impl<Provider, T: AuthenticatedMultiplexedTransport> SwarmBuilder<Provider, DnsPhase<T>> {
    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        self.without_dns()
            .without_relay()
            .without_websocket()
            .with_behaviour(constructor)
    }
}

pub struct RelayPhase<T> {
    transport: T,
}

// TODO: Noise feature or tls feature
#[cfg(feature = "relay")]
impl<Provider, T> SwarmBuilder<Provider, RelayPhase<T>> {
    // TODO: This should be with_relay_client.
    pub fn with_relay(self) -> SwarmBuilder<Provider, RelayTlsPhase<T>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: RelayTlsPhase {
                transport: self.phase.transport,
            },
        }
    }
}

pub struct NoRelayBehaviour;

impl<Provider, T> SwarmBuilder<Provider, RelayPhase<T>> {
    fn without_relay(self) -> SwarmBuilder<Provider, WebsocketPhase<T, NoRelayBehaviour>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: WebsocketPhase {
                transport: self.phase.transport,
                relay_behaviour: NoRelayBehaviour,
            },
        }
    }
}

// Shortcuts
impl<Provider, T: AuthenticatedMultiplexedTransport> SwarmBuilder<Provider, RelayPhase<T>> {
    #[cfg(feature = "websocket")]
    pub fn with_websocket(
        self,
    ) -> SwarmBuilder<
        Provider,
        WebsocketTlsPhase<impl AuthenticatedMultiplexedTransport, NoRelayBehaviour>,
    > {
        self.without_relay().with_websocket()
    }

    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        self.without_relay()
            .without_websocket()
            .with_behaviour(constructor)
    }
}

#[cfg(feature = "relay")]
pub struct RelayTlsPhase<T> {
    transport: T,
}

#[cfg(feature = "relay")]
impl<Provider, T> SwarmBuilder<Provider, RelayTlsPhase<T>> {
    #[cfg(feature = "tls")]
    pub fn with_tls(self) -> SwarmBuilder<Provider, RelayNoisePhase<T, Tls>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: RelayNoisePhase {
                transport: self.phase.transport,
                phantom: PhantomData,
            },
        }
    }

    fn without_tls(self) -> SwarmBuilder<Provider, RelayNoisePhase<T, WithoutTls>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: RelayNoisePhase {
                transport: self.phase.transport,

                phantom: PhantomData,
            },
        }
    }
}

// Shortcuts
#[cfg(all(feature = "relay", feature = "noise", feature = "async-std"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<AsyncStd, RelayTlsPhase<T>> {
    #[cfg(feature = "noise")]
    pub fn with_noise(
        self,
    ) -> Result<
        SwarmBuilder<
            AsyncStd,
            WebsocketPhase<impl AuthenticatedMultiplexedTransport, libp2p_relay::client::Behaviour>,
        >,
        AuthenticationError,
    > {
        self.without_tls().with_noise()
    }
}
#[cfg(all(feature = "relay", feature = "noise", feature = "tokio"))]
impl<T: AuthenticatedMultiplexedTransport> SwarmBuilder<Tokio, RelayTlsPhase<T>> {
    #[cfg(feature = "noise")]
    pub fn with_noise(
        self,
    ) -> Result<
        SwarmBuilder<
            Tokio,
            WebsocketPhase<impl AuthenticatedMultiplexedTransport, libp2p_relay::client::Behaviour>,
        >,
        AuthenticationError,
    > {
        self.without_tls().with_noise()
    }
}

#[cfg(feature = "relay")]
pub struct RelayNoisePhase<T, A> {
    transport: T,
    phantom: PhantomData<A>,
}

// TODO: Rename these macros to phase not builder. All.
#[cfg(feature = "relay")]
macro_rules! construct_websocket_builder {
    ($self:ident, $auth:expr) => {{
        let (relay_transport, relay_behaviour) =
            libp2p_relay::client::new($self.keypair.public().to_peer_id());

        Ok(SwarmBuilder {
            phase: WebsocketPhase {
                relay_behaviour,
                transport: $self
                    .phase
                    .transport
                    .or_transport(
                        relay_transport
                            .upgrade(libp2p_core::upgrade::Version::V1Lazy)
                            .authenticate($auth)
                            .multiplex(libp2p_yamux::Config::default())
                            .map(|(p, c), _| (p, StreamMuxerBox::new(c))),
                    )
                    .map(|either, _| either.into_inner()),
            },
            keypair: $self.keypair,
            phantom: PhantomData,
        })
    }};
}

#[cfg(all(feature = "relay", feature = "tls"))]
impl<Provider, T: AuthenticatedMultiplexedTransport>
    SwarmBuilder<Provider, RelayNoisePhase<T, Tls>>
{
    #[cfg(feature = "noise")]
    pub fn with_noise(
        self,
    ) -> Result<
        SwarmBuilder<
            Provider,
            WebsocketPhase<impl AuthenticatedMultiplexedTransport, libp2p_relay::client::Behaviour>,
        >,
        AuthenticationError,
    > {
        construct_websocket_builder!(
            self,
            libp2p_core::upgrade::Map::new(
                libp2p_core::upgrade::SelectUpgrade::new(
                    libp2p_tls::Config::new(&self.keypair)?,
                    libp2p_noise::Config::new(&self.keypair)?,
                ),
                |upgrade| match upgrade {
                    futures::future::Either::Left((peer_id, upgrade)) => {
                        (peer_id, futures::future::Either::Left(upgrade))
                    }
                    futures::future::Either::Right((peer_id, upgrade)) => {
                        (peer_id, futures::future::Either::Right(upgrade))
                    }
                },
            )
        )
    }

    pub fn without_noise(
        self,
    ) -> Result<
        SwarmBuilder<
            Provider,
            WebsocketPhase<impl AuthenticatedMultiplexedTransport, libp2p_relay::client::Behaviour>,
        >,
        AuthenticationError,
    > {
        construct_websocket_builder!(self, libp2p_tls::Config::new(&self.keypair)?)
    }
}

#[cfg(feature = "relay")]
impl<Provider, T: AuthenticatedMultiplexedTransport>
    SwarmBuilder<Provider, RelayNoisePhase<T, WithoutTls>>
{
    #[cfg(feature = "noise")]
    pub fn with_noise(
        self,
    ) -> Result<
        SwarmBuilder<
            Provider,
            WebsocketPhase<impl AuthenticatedMultiplexedTransport, libp2p_relay::client::Behaviour>,
        >,
        AuthenticationError,
    > {
        construct_websocket_builder!(self, libp2p_noise::Config::new(&self.keypair)?)
    }
}

pub struct WebsocketPhase<T, R> {
    transport: T,
    relay_behaviour: R,
}

#[cfg(feature = "websocket")]
impl<Provider, T, R> SwarmBuilder<Provider, WebsocketPhase<T, R>> {
    pub fn with_websocket(self) -> SwarmBuilder<Provider, WebsocketTlsPhase<T, R>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: WebsocketTlsPhase {
                transport: self.phase.transport,
                relay_behaviour: self.phase.relay_behaviour,
            },
        }
    }
}

impl<Provider, T: AuthenticatedMultiplexedTransport, R>
    SwarmBuilder<Provider, WebsocketPhase<T, R>>
{
    fn without_websocket(self) -> SwarmBuilder<Provider, BehaviourPhase<R>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: BehaviourPhase {
                relay_behaviour: self.phase.relay_behaviour,
                // TODO: Timeout needed?
                transport: self.phase.transport.boxed(),
            },
        }
    }
}

// Shortcuts
#[cfg(feature = "relay")]
impl<Provider, T: AuthenticatedMultiplexedTransport>
    SwarmBuilder<Provider, WebsocketPhase<T, libp2p_relay::client::Behaviour>>
{
    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair, libp2p_relay::client::Behaviour) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        self.without_websocket().with_behaviour(constructor)
    }
}

impl<Provider, T: AuthenticatedMultiplexedTransport>
    SwarmBuilder<Provider, WebsocketPhase<T, NoRelayBehaviour>>
{
    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        constructor: impl FnMut(&libp2p_identity::Keypair) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        self.without_websocket().with_behaviour(constructor)
    }
}

#[cfg(feature = "websocket")]
pub struct WebsocketTlsPhase<T, R> {
    transport: T,
    relay_behaviour: R,
}

#[cfg(feature = "websocket")]
impl<Provider, T, R> SwarmBuilder<Provider, WebsocketTlsPhase<T, R>> {
    #[cfg(feature = "tls")]
    pub fn with_tls(self) -> SwarmBuilder<Provider, WebsocketNoisePhase<T, R, Tls>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: WebsocketNoisePhase {
                relay_behaviour: self.phase.relay_behaviour,
                transport: self.phase.transport,
                phantom: PhantomData,
            },
        }
    }

    fn without_tls(self) -> SwarmBuilder<Provider, WebsocketNoisePhase<T, R, WithoutTls>> {
        SwarmBuilder {
            keypair: self.keypair,
            phantom: PhantomData,
            phase: WebsocketNoisePhase {
                relay_behaviour: self.phase.relay_behaviour,
                transport: self.phase.transport,
                phantom: PhantomData,
            },
        }
    }
}

// Shortcuts
#[cfg(all(feature = "websocket", feature = "noise", feature = "async-std"))]
impl<T: AuthenticatedMultiplexedTransport, R> SwarmBuilder<AsyncStd, WebsocketTlsPhase<T, R>> {
    #[cfg(feature = "noise")]
    pub async fn with_noise(
        self,
    ) -> Result<SwarmBuilder<AsyncStd, BehaviourPhase<R>>, WebsocketError> {
        self.without_tls().with_noise().await
    }
}
#[cfg(all(feature = "websocket", feature = "noise", feature = "tokio"))]
impl<T: AuthenticatedMultiplexedTransport, R> SwarmBuilder<Tokio, WebsocketTlsPhase<T, R>> {
    #[cfg(feature = "noise")]
    pub async fn with_noise(
        self,
    ) -> Result<SwarmBuilder<Tokio, BehaviourPhase<R>>, WebsocketError> {
        self.without_tls().with_noise().await
    }
}

#[cfg(feature = "websocket")]
pub struct WebsocketNoisePhase<T, R, A> {
    transport: T,
    relay_behaviour: R,
    phantom: PhantomData<A>,
}

#[cfg(feature = "websocket")]
macro_rules! construct_behaviour_builder {
    ($self:ident, $dnsTcp:expr, $auth:expr) => {{
        let websocket_transport = libp2p_websocket::WsConfig::new($dnsTcp.await?)
            .upgrade(libp2p_core::upgrade::Version::V1)
            .authenticate($auth)
            .multiplex(libp2p_yamux::Config::default())
            .map(|(p, c), _| (p, StreamMuxerBox::new(c)));

        Ok(SwarmBuilder {
            keypair: $self.keypair,
            phantom: PhantomData,
            phase: BehaviourPhase {
                transport: websocket_transport
                    .or_transport($self.phase.transport)
                    .map(|either, _| either.into_inner())
                    .boxed(),
                relay_behaviour: $self.phase.relay_behaviour,
            },
        })
    }};
}

macro_rules! impl_websocket_noise_builder {
    ($providerKebabCase:literal, $providerCamelCase:ident, $dnsTcp:expr) => {
        #[cfg(all(
                                                                    feature = $providerKebabCase,
                                                                    feature = "websocket",
                                                                    feature = "dns",
                                                                    feature = "tls"
                                                                ))]
        impl<T: AuthenticatedMultiplexedTransport, R>
            SwarmBuilder<$providerCamelCase, WebsocketNoisePhase< T, R, Tls>>
        {
            #[cfg(feature = "noise")]
            pub async fn with_noise(self) -> Result<SwarmBuilder<$providerCamelCase,BehaviourPhase<R>>, WebsocketError> {
                construct_behaviour_builder!(
                    self,
                    $dnsTcp,
                    libp2p_core::upgrade::Map::new(
                        libp2p_core::upgrade::SelectUpgrade::new(
                            libp2p_tls::Config::new(&self.keypair).map_err(Into::<AuthenticationError>::into)?,
                            libp2p_noise::Config::new(&self.keypair).map_err(Into::<AuthenticationError>::into)?,
                        ),
                        |upgrade| match upgrade {
                            futures::future::Either::Left((peer_id, upgrade)) => {
                                (peer_id, futures::future::Either::Left(upgrade))
                            }
                            futures::future::Either::Right((peer_id, upgrade)) => {
                                (peer_id, futures::future::Either::Right(upgrade))
                            }
                        },
                    )
                )
            }
            pub async fn without_noise(self) -> Result<SwarmBuilder<$providerCamelCase,BehaviourPhase<R>>, WebsocketError> {
                construct_behaviour_builder!(
                    self,
                    $dnsTcp,
                    libp2p_tls::Config::new(&self.keypair).map_err(Into::<AuthenticationError>::into)?
                )
            }
        }

        #[cfg(all(feature = $providerKebabCase, feature = "dns", feature = "websocket", feature = "noise"))]
        impl<T: AuthenticatedMultiplexedTransport, R>
            SwarmBuilder<$providerCamelCase, WebsocketNoisePhase< T, R, WithoutTls>>
        {
            pub async fn with_noise(self) -> Result<SwarmBuilder<$providerCamelCase, BehaviourPhase<R>>, WebsocketError> {
                construct_behaviour_builder!(
                    self,
                    $dnsTcp,
                    libp2p_noise::Config::new(&self.keypair).map_err(Into::<AuthenticationError>::into)?
                )
            }
        }
    };
}

impl_websocket_noise_builder!(
    "async-std",
    AsyncStd,
    libp2p_dns::DnsConfig::system(libp2p_tcp::async_io::Transport::new(
        libp2p_tcp::Config::default(),
    ))
);
// TODO: Unnecessary await for Tokio Websocket (i.e. tokio dns). Not ideal but don't know a better way.
impl_websocket_noise_builder!(
    "tokio",
    Tokio,
    futures::future::ready(libp2p_dns::TokioDnsConfig::system(
        libp2p_tcp::tokio::Transport::new(libp2p_tcp::Config::default())
    ))
);

#[derive(Debug, thiserror::Error)]
pub enum WebsocketError {
    #[error("Dns")]
    #[cfg(any(feature = "tls", feature = "noise"))]
    Authentication(#[from] AuthenticationError),
    #[cfg(feature = "dns")]
    #[error("Dns")]
    Dns(#[from] io::Error),
}

pub struct BehaviourPhase<R> {
    relay_behaviour: R,
    transport: libp2p_core::transport::Boxed<(libp2p_identity::PeerId, StreamMuxerBox)>,
}

#[cfg(feature = "relay")]
impl<Provider> SwarmBuilder<Provider, BehaviourPhase<libp2p_relay::client::Behaviour>> {
    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        mut constructor: impl FnMut(&libp2p_identity::Keypair, libp2p_relay::client::Behaviour) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        Ok(SwarmBuilder {
            phase: BuildPhase {
                behaviour: constructor(&self.keypair, self.phase.relay_behaviour)
                    .try_into_behaviour()?,
                transport: self.phase.transport,
            },
            keypair: self.keypair,
            phantom: PhantomData,
        })
    }
}

impl<Provider> SwarmBuilder<Provider, BehaviourPhase<NoRelayBehaviour>> {
    pub fn with_behaviour<B, R: TryIntoBehaviour<B>>(
        self,
        mut constructor: impl FnMut(&libp2p_identity::Keypair) -> R,
    ) -> Result<SwarmBuilder<Provider, BuildPhase<B>>, R::Error> {
        // Discard `NoRelayBehaviour`.
        let _ = self.phase.relay_behaviour;

        Ok(SwarmBuilder {
            phase: BuildPhase {
                behaviour: constructor(&self.keypair).try_into_behaviour()?,
                transport: self.phase.transport,
            },
            keypair: self.keypair,
            phantom: PhantomData,
        })
    }
}

pub struct BuildPhase<B> {
    behaviour: B,
    transport: libp2p_core::transport::Boxed<(libp2p_identity::PeerId, StreamMuxerBox)>,
}

#[cfg(feature = "async-std")]
impl<B: libp2p_swarm::NetworkBehaviour> SwarmBuilder<AsyncStd, BuildPhase<B>> {
    pub fn build(self) -> libp2p_swarm::Swarm<B> {
        libp2p_swarm::SwarmBuilder::with_async_std_executor(
            self.phase.transport,
            self.phase.behaviour,
            self.keypair.public().to_peer_id(),
        )
        .build()
    }
}

#[cfg(feature = "tokio")]
impl<B: libp2p_swarm::NetworkBehaviour> SwarmBuilder<Tokio, BuildPhase<B>> {
    pub fn build(self) -> libp2p_swarm::Swarm<B> {
        libp2p_swarm::SwarmBuilder::with_tokio_executor(
            self.phase.transport,
            self.phase.behaviour,
            self.keypair.public().to_peer_id(),
        )
        .build()
    }
}

pub enum NoProviderSpecified {}

#[cfg(feature = "async-std")]
pub enum AsyncStd {}

#[cfg(feature = "tokio")]
pub enum Tokio {}

pub trait AuthenticatedMultiplexedTransport:
    Transport<
        Error = Self::E,
        Dial = Self::D,
        ListenerUpgrade = Self::U,
        Output = (libp2p_identity::PeerId, StreamMuxerBox),
    > + Send
    + Unpin
    + 'static
{
    type E: Send + Sync + 'static;
    type D: Send;
    type U: Send;
}

impl<T> AuthenticatedMultiplexedTransport for T
where
    T: Transport<Output = (libp2p_identity::PeerId, StreamMuxerBox)> + Send + Unpin + 'static,
    <T as Transport>::Error: Send + Sync + 'static,
    <T as Transport>::Dial: Send,
    <T as Transport>::ListenerUpgrade: Send,
{
    type E = T::Error;
    type D = T::Dial;
    type U = T::ListenerUpgrade;
}

// TODO: Seal this.
pub trait TryIntoBehaviour<B> {
    type Error;

    fn try_into_behaviour(self) -> Result<B, Self::Error>;
}

impl<B> TryIntoBehaviour<B> for B
where
    B: NetworkBehaviour,
{
    type Error = Infallible;

    fn try_into_behaviour(self) -> Result<B, Self::Error> {
        Ok(self)
    }
}

impl<B> TryIntoBehaviour<B> for Result<B, Box<dyn std::error::Error + Send + Sync>>
where
    B: NetworkBehaviour,
{
    type Error = io::Error; // TODO: Consider a dedicated type here with a descriptive message like "failed to build behaviour"?

    fn try_into_behaviour(self) -> Result<B, Self::Error> {
        self.map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(all(feature = "tokio", feature = "tcp", feature = "tls", feature = "noise"))]
    fn tcp() {
        let _ = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp()
            .with_tls()
            .with_noise()
            .unwrap()
            .with_behaviour(|_| libp2p_swarm::dummy::Behaviour)
            .unwrap()
            .build();
    }

    #[test]
    #[cfg(all(
        feature = "tokio",
        feature = "tcp",
        feature = "tls",
        feature = "noise",
        feature = "quic"
    ))]
    fn tcp_quic() {
        let _ = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp()
            .with_tls()
            .with_noise()
            .unwrap()
            .with_quic()
            .with_behaviour(|_| libp2p_swarm::dummy::Behaviour)
            .unwrap()
            .build();
    }

    #[test]
    #[cfg(all(
        feature = "tokio",
        feature = "tcp",
        feature = "tls",
        feature = "noise",
        feature = "relay"
    ))]
    fn tcp_relay() {
        #[derive(libp2p_swarm::NetworkBehaviour)]
        #[behaviour(prelude = "libp2p_swarm::derive_prelude")]
        struct Behaviour {
            dummy: libp2p_swarm::dummy::Behaviour,
            relay: libp2p_relay::client::Behaviour,
        }

        let _ = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp()
            .with_tls()
            .with_noise()
            .unwrap()
            .with_relay()
            .with_tls()
            .with_noise()
            .unwrap()
            .with_behaviour(|_, relay| Behaviour {
                dummy: libp2p_swarm::dummy::Behaviour,
                relay,
            })
            .unwrap()
            .build();
    }

    #[test]
    #[cfg(all(
        feature = "tokio",
        feature = "tcp",
        feature = "tls",
        feature = "noise",
        feature = "dns"
    ))]
    fn tcp_dns() {
        let _ = futures::executor::block_on(
            SwarmBuilder::with_new_identity()
                .with_tokio()
                .with_tcp()
                .with_tls()
                .with_noise()
                .unwrap()
                .with_dns(),
        )
        .unwrap()
        .with_behaviour(|_| libp2p_swarm::dummy::Behaviour)
        .unwrap()
        .build();
    }

    /// Showcases how to provide custom transports unknown to the libp2p crate, e.g. QUIC or WebRTC.
    #[test]
    #[cfg(all(feature = "tokio", feature = "tcp", feature = "tls", feature = "noise"))]
    fn tcp_other_transport_other_transport() {
        let _ = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp()
            .with_tls()
            .with_noise()
            .unwrap()
            .with_other_transport(|_| libp2p_core::transport::dummy::DummyTransport::new())
            .with_other_transport(|_| libp2p_core::transport::dummy::DummyTransport::new())
            .with_other_transport(|_| libp2p_core::transport::dummy::DummyTransport::new())
            .with_behaviour(|_| libp2p_swarm::dummy::Behaviour)
            .unwrap()
            .build();
    }

    #[tokio::test]
    #[cfg(all(
        feature = "tokio",
        feature = "tcp",
        feature = "tls",
        feature = "noise",
        feature = "dns",
        feature = "websocket",
    ))]
    async fn tcp_websocket() {
        let _ = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp()
            .with_tls()
            .with_noise()
            .unwrap()
            .with_websocket()
            .with_tls()
            .with_noise()
            .await
            .unwrap()
            .with_behaviour(|_| libp2p_swarm::dummy::Behaviour)
            .unwrap()
            .build();
    }
}
