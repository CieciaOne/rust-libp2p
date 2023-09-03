// Native re-exports
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{build_swarm, init_logger, sleep, Instant, RedisClient};

// Wasm re-exports
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::{build_swarm, init_logger, sleep, Instant, RedisClient};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod native {
    use std::time::Duration;

    use anyhow::{bail, Context, Result};
    use env_logger::{Env, Target};
    use futures::future::BoxFuture;
    use futures::FutureExt;
    use libp2p::core::muxing::StreamMuxerBox;
    use libp2p::identity::Keypair;
    use libp2p::swarm::{NetworkBehaviour, Swarm};
    use libp2p::Transport as _;
    use libp2p_webrtc as webrtc;
    use redis::AsyncCommands;

    use crate::{from_env, Muxer, SecProtocol, Transport};

    pub(crate) type Instant = std::time::Instant;

    pub(crate) fn init_logger() {
        env_logger::Builder::from_env(Env::default().default_filter_or("info"))
            .target(Target::Stdout)
            .init();
    }

    pub(crate) fn sleep(duration: Duration) -> BoxFuture<'static, ()> {
        tokio::time::sleep(duration).boxed()
    }

    fn expect_muxer_yamux() -> Result<()> {
        Ok(match from_env("muxer")? {
            Muxer::Yamux => (),
            Muxer::Mplex => {
                bail!("Only Yamux is supported, not Mplex")
            }
        })
    }

    pub(crate) async fn build_swarm<B: NetworkBehaviour>(
        ip: &str,
        transport: Transport,
        behaviour_constructor: impl FnOnce(&Keypair) -> B,
    ) -> Result<(Swarm<B>, String)> {
        let (swarm, addr) = match (transport, from_env::<SecProtocol>("security")) {
            (Transport::QuicV1, _) => {
                let swarm = libp2p::SwarmBuilder::with_new_identity()
                    .with_tokio()
                    .with_quic()
                    .with_behaviour(behaviour_constructor)?
                    .build();
                (swarm, format!("/ip4/{ip}/udp/0/quic-v1"))
            }
            (Transport::Tcp, Ok(SecProtocol::Tls)) => {
                expect_muxer_yamux()?;

                let swarm = libp2p::SwarmBuilder::with_new_identity()
                    .with_tokio()
                    .with_tcp()
                    .with_tls()?
                    .with_behaviour(behaviour_constructor)?
                    .build();
                (swarm, format!("/ip4/{ip}/tcp/0"))
            }
            (Transport::Tcp, Ok(SecProtocol::Noise)) => {
                expect_muxer_yamux()?;

                let swarm = libp2p::SwarmBuilder::with_new_identity()
                    .with_tokio()
                    .with_tcp()
                    .with_noise()?
                    .with_behaviour(behaviour_constructor)?
                    .build();
                (swarm, format!("/ip4/{ip}/tcp/0"))
            }
            (Transport::Ws, Ok(SecProtocol::Tls)) => {
                expect_muxer_yamux()?;

                let swarm = libp2p::SwarmBuilder::with_new_identity()
                    .with_tokio()
                    .with_websocket()
                    .with_tls()?
                    .without_noise()
                    .await?
                    .with_behaviour(behaviour_constructor)?
                    .build();
                (swarm, format!("/ip4/{ip}/tcp/0/ws"))
            }
            (Transport::Ws, Ok(SecProtocol::Noise)) => {
                expect_muxer_yamux()?;

                let swarm = libp2p::SwarmBuilder::with_new_identity()
                    .with_tokio()
                    .with_websocket()
                    .with_noise()
                    .await?
                    .with_behaviour(behaviour_constructor)?
                    .build();
                (swarm, format!("/ip4/{ip}/tcp/0/ws"))
            }
            (Transport::WebRtcDirect, _) => {
                let swarm = libp2p::SwarmBuilder::with_new_identity()
                    .with_tokio()
                    .with_other_transport(|key| {
                        Ok(webrtc::tokio::Transport::new(
                            key.clone(),
                            webrtc::tokio::Certificate::generate(&mut rand::thread_rng())?,
                        )
                        .map(|(peer_id, conn), _| (peer_id, StreamMuxerBox::new(conn))))
                    })?
                    .with_behaviour(behaviour_constructor)?
                    .build();

                (swarm, format!("/ip4/{ip}/udp/0/webrtc-direct"))
            }
            (Transport::Tcp, Err(_)) => bail!("Missing security protocol for TCP transport"),
            (Transport::Ws, Err(_)) => bail!("Missing security protocol for Websocket transport"),
            (Transport::Webtransport, _) => bail!("Webtransport can only be used with wasm"),
        };
        Ok((swarm, addr))
    }

    pub(crate) struct RedisClient(redis::Client);

    impl RedisClient {
        pub(crate) fn new(redis_addr: &str) -> Result<Self> {
            Ok(Self(
                redis::Client::open(redis_addr).context("Could not connect to redis")?,
            ))
        }

        pub(crate) async fn blpop(&self, key: &str, timeout: u64) -> Result<Vec<String>> {
            let mut conn = self.0.get_async_connection().await?;
            Ok(conn.blpop(key, timeout as usize).await?)
        }

        pub(crate) async fn rpush(&self, key: &str, value: String) -> Result<()> {
            let mut conn = self.0.get_async_connection().await?;
            conn.rpush(key, value).await?;
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) mod wasm {
    use anyhow::{bail, Result};
    use futures::future::{BoxFuture, FutureExt};
    use libp2p::core::muxing::StreamMuxerBox;
    use libp2p::identity::Keypair;
    use libp2p::swarm::{NetworkBehaviour, Swarm};
    use libp2p::Transport as _;
    use std::time::Duration;

    use crate::{BlpopRequest, Transport};

    pub(crate) type Instant = instant::Instant;

    pub(crate) fn init_logger() {
        console_error_panic_hook::set_once();
        wasm_logger::init(wasm_logger::Config::default());
    }

    pub(crate) fn sleep(duration: Duration) -> BoxFuture<'static, ()> {
        futures_timer::Delay::new(duration).boxed()
    }

    pub(crate) async fn build_swarm<B: NetworkBehaviour>(
        ip: &str,
        transport: Transport,
        behaviour_constructor: impl FnOnce(&Keypair) -> B,
    ) -> Result<(Swarm<B>, String)> {
        if let Transport::Webtransport = transport {
            let swarm = libp2p::SwarmBuilder::with_new_identity()
                .with_wasm_bindgen()
                .with_other_transport(|key| {
                    libp2p::webtransport_websys::Transport::new(
                        libp2p::webtransport_websys::Config::new(key),
                    )
                    .map(|(peer_id, conn), _| (peer_id, StreamMuxerBox::new(conn)))
                })?
                .with_behaviour(behaviour_constructor)?
                .build();
            return Ok((swarm, format!("/ip4/{ip}/udp/0/quic/webtransport")));
        } else {
            bail!("Only webtransport supported with wasm")
        }
    }

    pub(crate) struct RedisClient(String);

    impl RedisClient {
        pub(crate) fn new(base_url: &str) -> Result<Self> {
            Ok(Self(base_url.to_owned()))
        }

        pub(crate) async fn blpop(&self, key: &str, timeout: u64) -> Result<Vec<String>> {
            let res = reqwest::Client::new()
                .post(&format!("http://{}/blpop", self.0))
                .json(&BlpopRequest {
                    key: key.to_owned(),
                    timeout,
                })
                .send()
                .await?
                .json()
                .await?;
            Ok(res)
        }

        pub(crate) async fn rpush(&self, _: &str, _: String) -> Result<()> {
            bail!("unimplemented")
        }
    }
}
