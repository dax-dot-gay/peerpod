use std::sync::{Arc, Mutex};

use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use derive_builder::Builder;
use libp2p::{identity::{Keypair, PublicKey}, Multiaddr, PeerId};
use serde::{Deserialize, Serialize};

use crate::types::{Error, PodResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PeerNode {
    Relay(Multiaddr),
    Rendezvous(Multiaddr),
    Direct(Multiaddr),
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
    pub known_nodes: Arc<Mutex<Vec<KnownNode>>>,
}

impl Node {
    pub fn from_info(info: NodeInfo) -> PodResult<Node> {
        let keypair = Keypair::from_protobuf_encoding(
            URL_SAFE
                .decode(info.key.clone())
                .or_else(|e| Err(Error::Base64DecodingError {error: e.to_string(), contents: info.key.clone()}))?
                .as_slice(),
        ).or(Err(Error::KeypairError("Failed to decode keypair from encoded bytes".to_string())))?;
        Ok(Node {
            key: keypair,
            class: info.class,
            bootstrap: info.bootstrap,
            friendly_name: info.friendly_name,
            known_nodes: Arc::new(Mutex::new(info.known_nodes))
        })
    }

    pub fn into_info(&self) -> PodResult<NodeInfo> {
        let encoded_key = URL_SAFE.encode(self.key.to_protobuf_encoding().or(Err(Error::KeypairError("Failed to encode keypair as bytes".to_string())))?);
        let known = self.known_nodes.lock().or(Err(Error::SyncError("Failed to unwrap known_nodes".to_string())))?.clone();
        Ok(NodeInfo {
            key: encoded_key,
            class: self.class.clone(),
            bootstrap: self.bootstrap.clone(),
            friendly_name: self.friendly_name.clone(),
            known_nodes: known
        })
    }

    pub fn public_key(&self) -> PublicKey {
        self.key.public()
    }

    pub fn peer_id(&self) -> PeerId {
        self.key.public().to_peer_id()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub key: String,
    pub class: String,
    pub bootstrap: Vec<PeerNode>,
    pub friendly_name: Option<String>,
    pub known_nodes: Vec<KnownNode>,
}
