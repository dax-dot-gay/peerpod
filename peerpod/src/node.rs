use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_channel::{unbounded, Receiver, Sender};
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use derive_builder::Builder;
use libp2p::StreamProtocol;
use libp2p::{
    autonat, dcutr, identify,
    identity::{Keypair, PublicKey},
    noise, relay, rendezvous,
    request_response::{self, ProtocolSupport},
    swarm::NetworkBehaviour,
    tcp, upnp, yamux, Multiaddr, PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use tokio::task::spawn;

use crate::event_loop::EventLoop;
use crate::types::{Command, Error, Event, NodeRequest, NodeResponse, PodResult};

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    relay_client: relay::client::Behaviour,
    request_response: request_response::json::Behaviour<NodeRequest, NodeResponse>,
    dcutr: dcutr::Behaviour,
    rendezvous: rendezvous::client::Behaviour,
    identify: identify::Behaviour,
    upnp: upnp::tokio::Behaviour,
    autonat: autonat::Behaviour,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PeerNode {
    Relay(Multiaddr),
    Rendezvous(Multiaddr)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnownNode {
    pub id: PeerId,
    pub address: PeerNode,
    pub friendly_name: Option<String>,
}

#[derive(Clone, Debug, Builder)]
pub struct Node {
    #[builder(default = "Keypair::generate_ed25519()")]
    pub key: Keypair,

    #[builder(default = "\"peerpod:default\".to_string()")]
    pub class: String,

    #[builder(default = "Vec::new()")]
    pub bootstrap: Vec<PeerNode>,

    pub friendly_name: Option<String>,

    #[builder(setter(skip))]
    pub command_sender: Option<Sender<Command>>,

    #[builder(setter(skip))]
    pub event_receiver: Option<Receiver<Event>>,

    #[builder(setter(skip))]
    pub event_loop: Option<EventLoop>
}

impl Node {
    pub fn from_info(info: NodeInfo) -> PodResult<Node> {
        let keypair = Keypair::from_protobuf_encoding(
            URL_SAFE
                .decode(info.key.clone())
                .or_else(|e| {
                    Err(Error::Base64DecodingError {
                        error: e.to_string(),
                        contents: info.key.clone(),
                    })
                })?
                .as_slice(),
        )
        .or(Err(Error::KeypairError(
            "Failed to decode keypair from encoded bytes".to_string(),
        )))?;
        Ok(Node {
            key: keypair,
            class: info.class,
            bootstrap: info.bootstrap,
            friendly_name: info.friendly_name,
            command_sender: None,
            event_receiver: None,
            event_loop: None
        })
    }

    pub fn into_info(&self) -> PodResult<NodeInfo> {
        let encoded_key = URL_SAFE.encode(self.key.to_protobuf_encoding().or(Err(
            Error::KeypairError("Failed to encode keypair as bytes".to_string()),
        ))?);
        Ok(NodeInfo {
            key: encoded_key,
            class: self.class.clone(),
            bootstrap: self.bootstrap.clone(),
            friendly_name: self.friendly_name.clone()
        })
    }

    pub fn public_key(&self) -> PublicKey {
        self.key.public()
    }

    pub fn peer_id(&self) -> PeerId {
        self.key.public().to_peer_id()
    }

    pub fn initialize(&mut self) -> PodResult<()> {
        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(self.key.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )
            .or_else(|e| {
                Err(Error::SwarmError {
                    kind: crate::types::SwarmErrorType::Initialization,
                    reason: e.to_string(),
                })
            })?
            .with_relay_client(noise::Config::new, yamux::Config::default)
            .or_else(|e| {
                Err(Error::SwarmError {
                    kind: crate::types::SwarmErrorType::RelaySetup,
                    reason: e.to_string(),
                })
            })?
            .with_behaviour(|key, relay_behaviour| Behaviour {
                relay_client: relay_behaviour,
                identify: identify::Behaviour::new(identify::Config::new(
                    format!("peerpod/{}", self.class),
                    key.public(),
                )),
                request_response: request_response::json::Behaviour::new(
                    [(StreamProtocol::new("/peerpod-json"), ProtocolSupport::Full)],
                    request_response::Config::default(),
                ),
                dcutr: dcutr::Behaviour::new(key.public().to_peer_id()),
                rendezvous: rendezvous::client::Behaviour::new(key.clone()),
                upnp: upnp::tokio::Behaviour::default(),
                autonat: autonat::Behaviour::new(
                    key.public().to_peer_id(),
                    autonat::Config::default(),
                ),
            })
            .or_else(|e| {
                Err(Error::SwarmError {
                    kind: crate::types::SwarmErrorType::BehaviourSetup,
                    reason: e.to_string(),
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        for node in self.bootstrap.clone() {
            let _ = match node {
                PeerNode::Relay(addr) => swarm.dial(addr.clone()).or_else(|e| Err(Error::DialError { error: e.to_string(), address: addr })),
                PeerNode::Rendezvous(addr) => swarm.dial(addr.clone()).or_else(|e| Err(Error::DialError { error: e.to_string(), address: addr })),
            };
        }

        let (command_send, command_recv) = unbounded::<Command>();
        let (event_send, event_recv) = unbounded::<Event>();
        self.event_loop.insert(EventLoop::new(swarm, command_recv, event_send));
        spawn(self.event_loop.unwrap().run());
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub key: String,
    pub class: String,
    pub bootstrap: Vec<PeerNode>,
    pub friendly_name: Option<String>,
}
