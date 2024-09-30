use std::{collections::HashMap, sync::{Arc, Mutex}};

use async_channel::{Receiver, Sender};
use libp2p::{
    futures::StreamExt, rendezvous::{client::Event as RsvEvent, Namespace}, request_response::{Event as ReqEvent, Message, ResponseChannel}, swarm::SwarmEvent, Multiaddr, PeerId, Swarm
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    node::{Behaviour, BehaviourEvent, NodeInfo, PeerNode},
    types::{Command, CommandKind, Error, Event, EventKind, EventType, NodeRequest, NodeResponse, PodResult},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnownNode {
    pub id: PeerId,
    pub addresses: Vec<Multiaddr>
}

pub struct EventLoop {
    pub swarm: Swarm<Behaviour>,
    pub commands: Receiver<Command>,
    pub events: Sender<Event>,
    pub known_nodes: Vec<KnownNode>,
    pub address: String,
    pub node_info: NodeInfo,
    pub channels: HashMap<Uuid, ResponseChannel<NodeResponse>>,
    pub listeners: HashMap<Uuid, Listener>
}

#[derive(Clone)]
pub enum Listener {
    Event {
        kind: EventType,
        listener: fn(EventKind) -> ()
    },
    Request {
        path: String,
        listener: Arc<Mutex<dyn Fn(NodeRequest) -> NodeResponse + Send + Sync>>
    }
}

impl EventLoop {
    pub fn add_known(&mut self, node: KnownNode) -> KnownNode {
        for known in &mut self.known_nodes {
            if known.id == node.id {
                known
                    .addresses
                    .extend_from_slice(node.addresses.as_slice());
                known.addresses.dedup();
                return known.clone();
            }
        }
        self.known_nodes.push(node.clone());
        node.clone()
    }

    pub fn is_rendezvous(&self, id: PeerId) -> bool {
        self.node_info.bootstrap.iter().any(|node| {
            if let PeerNode::Rendezvous(peer_id, _) = node {
                id == *peer_id
            } else {
                false
            }
        })
    }

    pub fn is_relay(&self, id: PeerId) -> bool {
        self.node_info.bootstrap.iter().any(|node| {
            if let PeerNode::Relay(peer_id, _) = node {
                id == *peer_id
            } else {
                false
            }
        })
    }

    pub fn remove_known(&mut self, id: PeerId) -> () {
        self.known_nodes.retain(|v| !v.id.eq(&id));
    }

    pub fn get_known(&self, id: PeerId) -> Option<KnownNode> {
        for node in &self.known_nodes {
            if node.id == id {
                return Some(node.clone());
            }
        }
        None
    }

    pub fn ensure_known(&mut self, id: PeerId) -> KnownNode {
        if let Some(found) = self.get_known(id) {
            return found.clone();
        }
        return self.add_known(KnownNode {id, addresses: Vec::new()}).clone();
    }

    pub fn namespace(&self) -> PodResult<Namespace> {
        Ok(Namespace::new(self.node_info.class.clone()).or(Err(Error::InvalidClass(self.node_info.class.clone())))?)
    }

    async fn handle_event(&mut self, event: SwarmEvent<BehaviourEvent>) -> PodResult<()> {
        match event {
            SwarmEvent::NewExternalAddrOfPeer { peer_id, address } => {
                let result = self.add_known(KnownNode {
                    id: peer_id,
                    addresses: vec![address.clone()]
                });
                self.event(EventKind::PeerUpdate(result)).await;
                Ok(())
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                if self.is_rendezvous(peer_id) {
                    let ns = self.namespace()?;
                    self.swarm
                        .behaviour_mut()
                        .rendezvous
                        .register(
                            ns,
                            peer_id,
                            None,
                        )
                        .or_else(|e| {
                            Err(Error::RegistrationFailed {
                                peer: peer_id,
                                error: e.to_string(),
                            })
                        })?;
                    let node = self.ensure_known(peer_id);
                    self.event(EventKind::RegisteringAt(node)).await;
                }

                let node = self.ensure_known(peer_id);
                self.event(EventKind::Connected(node)).await;
                Ok(())
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                self.event(EventKind::Listening(address)).await;
                Ok(())
            }
            SwarmEvent::Behaviour(behaviour) => match behaviour {
                BehaviourEvent::Rendezvous(rsv) => match rsv {
                    RsvEvent::Registered { rendezvous_node, .. } => {
                        let ns = self.namespace()?;
                        self.swarm.behaviour_mut().rendezvous.discover(Some(ns), None, None, rendezvous_node);
                        let node = self.ensure_known(rendezvous_node);
                        self.event(EventKind::RegisteringAt(node)).await;
                        Ok(())
                    },
                    RsvEvent::RegisterFailed { rendezvous_node, error, .. } => {
                        let node = self.ensure_known(rendezvous_node);
                        self.event(EventKind::RegistrationFailed { node, reason: format!("{error:?}") }).await;
                        Ok(())
                    },
                    RsvEvent::Discovered { rendezvous_node, registrations, .. } => {
                        let rsv_node = self.ensure_known(rendezvous_node);
                        for reg in registrations {
                            let node = KnownNode {
                                id: reg.record.peer_id(),
                                addresses: reg.record.addresses().to_vec()
                            };
                            self.add_known(node.clone());
                            self.event(EventKind::Discovered { rendezvous: rsv_node.clone(), peer: node }).await;
                        }
                        Ok(())
                    }
                    _ => Ok(())
                },
                BehaviourEvent::RequestResponse(req) => match req {
                    ReqEvent::Message { peer, message } => match message {
                        Message::Request { request, channel, .. } => {
                            let node = self.ensure_known(peer);
                            self.channels.insert(request.id, channel);
                            self.event(EventKind::ReceivedRequest { source: node.clone(), request: request.clone() }).await;
                            Ok(())
                        },
                        Message::Response { response, .. } => {
                            self.event(EventKind::ReceivedResponse(response.clone())).await;
                            Ok(())
                        }
                    },
                    _ => Ok(())
                }
                _ => Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn handle_command(&mut self, command: Command) -> PodResult<()> {
        match command.command.clone() {
            CommandKind::GetKnownNodes => {
                command.reply(Ok(self.known_nodes.clone())).await;
            },
            CommandKind::SendRequest { target, request } => {
                self.swarm.behaviour_mut().request_response.send_request(&target, request.clone());
                command.reply(Ok(())).await;
            },
            CommandKind::SendResponse {response } => {
                if let Some(channel) = self.channels.remove(&response.clone().request_id) {
                    let _ = self.swarm.behaviour_mut().request_response.send_response(channel, response.clone());
                    command.reply(Ok(())).await;
                } else {
                    command.reply::<()>(Err(Error::ExpiredRequest)).await;
                }
            },
            CommandKind::RegisterListener(listener) => {
                let id = Uuid::new_v4();
                self.listeners.insert(id.clone(), listener);
                command.reply(Ok(id)).await;
            },
            CommandKind::DeregisterListener(id) => {
                if let Some(_) = self.listeners.remove(&id) {
                    command.reply(Ok(())).await;
                } else {
                    command.reply::<()>(Err(Error::UnknownListener(id.clone()))).await;
                }
            }
        }
        Ok(())
    }

    async fn event(&mut self, event: EventKind) {
        let _ = self.events.send(event.wrap()).await;
        let event_type = event.kind();
        for listener in self.listeners.values() {
            let evt = event.clone();
            match listener {
                Listener::Event { kind, listener } => {
                    if *kind == event_type {
                        listener(evt);
                    }
                },
                Listener::Request { path, listener } => {
                    if let EventKind::ReceivedRequest { request, .. } = evt {
                        if request.request == path.clone() {
                            if let Ok(func) = listener.lock() {
                                let response = func(request.clone());
                                if let Some(ch) = self.channels.remove(&response.request_id) {
                                    let _ = self.swarm.behaviour_mut().request_response.send_response(ch, response);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[allow(unreachable_code)]
    pub async fn run(&mut self) -> PodResult<()> {
        let listener = self
            .swarm
            .listen_on(
                self.address
                    .clone()
                    .parse::<Multiaddr>()
                    .or(Err(Error::InvalidMultiAddr(self.address.clone())))?,
            )
            .or_else(|e| Err(Error::TransportError(e.to_string())))?;
        loop {
            let mut pinned_commands = Box::pin(self.commands.clone());
            let result = tokio::select! {
                event = self.swarm.select_next_some() => self.handle_event(event).await,
                channel_event = pinned_commands.next() => if let Some(command) = channel_event {
                    self.handle_command(command).await
                } else {
                    Err(Error::ChannelClosed)
                }
            };
            if let Err(error) = result {
                let _ = self
                    .events
                    .send(EventKind::Error(error.clone()).wrap())
                    .await;
                if let Error::ChannelClosed = error {
                    break;
                }
            }
        }
        self.swarm.remove_listener(listener);
        Ok(())
    }

    pub fn new(
        swarm: Swarm<Behaviour>,
        commands: Receiver<Command>,
        events: Sender<Event>,
        address: String,
        info: NodeInfo,
    ) -> Self {
        EventLoop {
            swarm,
            commands,
            events,
            known_nodes: Vec::new(),
            address,
            node_info: info,
            channels: HashMap::new(),
            listeners: HashMap::new()
        }
    }
}
