use async_channel::{Receiver, Sender};
use libp2p::{PeerId, Swarm};

use crate::{node::{Behaviour, KnownNode}, types::{Command, Event, PodResult}};

pub struct EventLoop {
    pub swarm: Swarm<Behaviour>,
    pub commands: Receiver<Command>,
    pub events: Sender<Event>,
    pub known_nodes: Vec<KnownNode>
}

impl EventLoop {
    pub fn add_known(&mut self, node: KnownNode) -> PodResult<()> {
        self.known_nodes.push(node.clone());
        self.known_nodes.dedup_by(|a, b| a.id.eq(&b.id));
        Ok(())
    }

    pub fn remove_known(&mut self, id: PeerId) -> PodResult<()> {
        self.known_nodes.retain(|v| !v.id.eq(&id));
        Ok(())
    }

    pub async fn run(&mut self) {
        ()
    }

    pub fn new(swarm: Swarm<Behaviour>, commands: Receiver<Command>, events: Sender<Event>) -> Self {
        EventLoop {
            swarm,
            commands,
            events,
            known_nodes: Vec::new()
        }
    }
}