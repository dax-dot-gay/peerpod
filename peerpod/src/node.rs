use std::fmt::{Debug, Display};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_channel::{bounded, unbounded, Receiver, Sender};
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use derive_builder::Builder;
use libp2p::StreamProtocol;
use libp2p::{
    autonat, dcutr, identify,
    identity::{Keypair, PublicKey},
    noise, relay, rendezvous,
    request_response::{self, ProtocolSupport},
    swarm::NetworkBehaviour,
    tcp, upnp, yamux, Multiaddr, PeerId,
    ping
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::{spawn, JoinHandle};
use uuid::Uuid;

use crate::event_loop::{EventLoop, KnownNode};
use crate::types::{
    Command, CommandKind, Error, Event, EventKind, EventType, NodeRequest, NodeResponse, PodResult,
};

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub relay_client: relay::client::Behaviour,
    pub request_response: request_response::json::Behaviour<NodeRequest, NodeResponse>,
    pub dcutr: dcutr::Behaviour,
    pub rendezvous: rendezvous::client::Behaviour,
    pub identify: identify::Behaviour,
    pub upnp: upnp::tokio::Behaviour,
    pub autonat: autonat::Behaviour,
    pub ping: ping::Behaviour
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PeerNode {
    Relay(PeerId, Multiaddr),
    Rendezvous(PeerId, Multiaddr),
}

#[derive(Builder)]
pub struct Node {
    #[builder(default = "Keypair::generate_ed25519()")]
    pub key: Keypair,

    #[builder(default = "\"peerpod:default\".to_string()")]
    pub class: String,

    #[builder(default = "Vec::new()")]
    pub bootstrap: Vec<PeerNode>,

    #[builder(default = "\"/ip4/0.0.0.0/tcp/0\".to_string()")]
    pub listen_address: String,

    #[builder(setter(skip))]
    pub command_sender: Option<Sender<Command>>,

    #[builder(setter(skip))]
    pub event_receiver: Option<Receiver<Event>>,

    #[builder(setter(skip))]
    pub event_loop: Option<JoinHandle<PodResult<()>>>,
}

impl NodeBuilder {
    pub fn add_rendezvous(&mut self, id: String, address: String) -> PodResult<&mut NodeBuilder> {
        let peer_id =
            PeerId::from_str(id.clone().as_str()).or(Err(Error::InvalidPeerId(id.clone())))?;
        let multiaddr = Multiaddr::from_str(&address.clone().as_str())
            .or(Err(Error::InvalidMultiAddr(address.clone())))?;
        if self.bootstrap.is_none() {
            self.bootstrap = Some(Vec::new());
        }
        if let Some(bootstrap) = &mut self.bootstrap {
            bootstrap.push(PeerNode::Rendezvous(peer_id, multiaddr));
        }

        Ok(self)
    }

    pub fn add_relay(&mut self, id: String, address: String) -> PodResult<&mut NodeBuilder> {
        let peer_id =
            PeerId::from_str(id.clone().as_str()).or(Err(Error::InvalidPeerId(id.clone())))?;
        let multiaddr = Multiaddr::from_str(&address.clone().as_str())
            .or(Err(Error::InvalidMultiAddr(address.clone())))?;
        if self.bootstrap.is_none() {
            self.bootstrap = Some(Vec::new());
        }
        if let Some(bootstrap) = &mut self.bootstrap {
            bootstrap.push(PeerNode::Relay(peer_id, multiaddr));
        }

        Ok(self)
    }
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
            command_sender: None,
            event_receiver: None,
            event_loop: None,
            listen_address: info.listen_address,
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
            listen_address: self.listen_address.clone(),
        })
    }

    pub fn public_key(&self) -> PublicKey {
        self.key.public()
    }

    pub fn peer_id(&self) -> PeerId {
        self.key.public().to_peer_id()
    }

    pub fn initialize(&mut self) -> PodResult<()> {
        if self.event_loop.is_some() {
            return Err(Error::AlreadyInitialized);
        }
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
                ping: ping::Behaviour::default()
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
                PeerNode::Relay(_, addr) => swarm.dial(addr.clone()).or_else(|e| {
                    Err(Error::DialError {
                        error: e.to_string(),
                        address: addr,
                    })
                }),
                PeerNode::Rendezvous(_, addr) => swarm.dial(addr.clone()).or_else(|e| {
                    Err(Error::DialError {
                        error: e.to_string(),
                        address: addr,
                    })
                }),
            };
        }

        let (command_send, command_recv) = unbounded::<Command>();
        let (event_send, event_recv) = unbounded::<Event>();
        let mut ev_loop = EventLoop::new(
            swarm,
            command_recv,
            event_send,
            self.listen_address.clone(),
            self.into_info()?,
        );
        self.event_loop = Some(spawn(async move { ev_loop.run().await }));
        let _ = self.command_sender.insert(command_send);
        let _ = self.event_receiver.insert(event_recv);
        Ok(())
    }

    pub async fn execute_command<T: Serialize + DeserializeOwned>(
        &self,
        command: CommandKind,
    ) -> PodResult<T> {
        if let Some(sender) = self.command_sender.clone() {
            let (tx, rx) = bounded::<PodResult<Value>>(1);
            let cmd = Command {
                command,
                response_channel: tx.clone(),
            };
            sender
                .send(cmd)
                .await
                .or_else(|e| Err(Error::ChannelFailure(e.to_string())))?;
            let response = rx
                .recv()
                .await
                .or_else(|e| Err(Error::ChannelFailure(e.to_string())))?;
            match response {
                Ok(success) => Ok(serde_json::from_value::<T>(success.clone()).or_else(|e| {
                    Err(Error::JsonDecodingError {
                        error: e.to_string(),
                        contents: success.clone(),
                    })
                })?),
                Err(e) => Err(e),
            }
        } else {
            Err(Error::NotInitialized)
        }
    }

    pub async fn on_event(
        &self,
        event: EventType,
        listener: impl Fn(EventKind) -> () + Sync + Send + 'static,
    ) -> PodResult<Uuid> {
        self.execute_command::<Uuid>(CommandKind::RegisterListener(
            crate::event_loop::Listener::Event {
                kind: vec![event],
                listener: Arc::new(Mutex::new(listener)),
            },
        ))
        .await
    }

    pub async fn on_events(
        &self,
        events: Vec<EventType>,
        listener: impl Fn(EventKind) -> () + Sync + Send + 'static,
    ) -> PodResult<Uuid> {
        self.execute_command::<Uuid>(CommandKind::RegisterListener(
            crate::event_loop::Listener::Event {
                kind: events,
                listener: Arc::new(Mutex::new(listener)),
            },
        ))
        .await
    }

    pub async fn on_request<
        T: Serialize + DeserializeOwned + Clone + 'static,
        R: Serialize + DeserializeOwned + Clone + 'static,
        E: Serialize + DeserializeOwned + Clone + Debug + Display + 'static,
    >(
        &self,
        path: String,
        listener: impl Fn(T) -> Result<R, E> + Sync + Send + 'static,
    ) -> PodResult<Uuid> {
        let process_internal = move |request: NodeRequest| {
            {
                match request.unwrap::<T>() {
                    Ok(parsed) => match request.respond(listener(parsed)) {
                        Ok(r) => Ok::<NodeResponse, E>(r),
                        Err(e) => Ok(request
                            .respond::<T, Error>(Err(e))
                            .expect("Failed to wrap internal error.")),
                    },
                    Err(e) => Ok(request
                        .respond::<T, Error>(Err(e))
                        .expect("Failed to wrap internal error.")),
                }
                .unwrap()
            }
            .clone()
        };

        self.execute_command(CommandKind::RegisterListener(
            crate::event_loop::Listener::Request {
                path,
                listener: Arc::new(Mutex::new(process_internal)),
            },
        ))
        .await
    }

    pub async fn request<T: Serialize + DeserializeOwned + Clone, R: Serialize + DeserializeOwned + Clone, E: Serialize + DeserializeOwned + Clone>(
        &self,
        peer: String,
        path: String,
        data: T
    ) -> PodResult<Result<R, E>> {
        let peer_id = PeerId::from_str(peer.clone().as_str()).or(Err(Error::InvalidPeerId(peer.clone())))?;
        let request = NodeRequest::new(self.peer_id(), path, data.clone())?;
        let rq_id = request.id.clone();
        let (tx, rx) = bounded::<Result<Value, Value>>(1);
        let listener = self.on_events(vec![EventType::Response, EventType::RequestFailed], move |evt| {
            if let EventKind::ReceivedResponse(resp) = evt {
                if resp.request_id == rq_id {
                    let _ = tx.send_blocking(resp.content);
                }
            } else if let EventKind::RequestFailed { error, request_id } = evt {
                if request_id == rq_id {
                    let _ = tx.send_blocking(Err::<Value, Value>(serde_json::to_value(Error::RequestFailed(error.to_string())).expect("Failed to encode internal error.")));
                }
            }
        }).await?;
        self.execute_command::<()>(CommandKind::SendRequest { target: peer_id, request }).await?;
        let result = rx.recv().await.or_else(|e| Err(Error::ChannelFailure(e.to_string())))?;
        self.execute_command::<()>(CommandKind::DeregisterListener(listener)).await?;
        Ok(match result {
            Ok(val) => Ok(serde_json::from_value::<R>(val.clone()).or_else(|e| Err(Error::JsonDecodingError { error: e.to_string(), contents: val.clone() }))?),
            Err(val) => Err(serde_json::from_value::<E>(val.clone()).or_else(|e| Err(Error::JsonDecodingError { error: e.to_string(), contents: val.clone() }))?)
        })
    }

    pub async fn get_peers(&self) -> PodResult<Vec<KnownNode>> {
        self.execute_command::<Vec<PeerId>>(CommandKind::DiscoverPeers).await?;
        self.execute_command::<Vec<KnownNode>>(CommandKind::GetKnownNodes).await
    }

    pub async fn get_connected_peers(&self) -> PodResult<Vec<KnownNode>> {
        self.execute_command::<Vec<KnownNode>>(CommandKind::ActiveConnections).await
    }

    pub async fn is_connected(&self, peer: PeerId) -> bool {
        if let Ok(connected) = self.get_connected_peers().await {
            for node in connected {
                if node.id == peer {
                    return true;
                }
            }
        }
        false
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub key: String,
    pub class: String,
    pub bootstrap: Vec<PeerNode>,
    pub listen_address: String,
}
