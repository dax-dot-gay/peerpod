use std::fmt::Display;

use async_channel::Sender;
use libp2p::{Multiaddr, PeerId};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::event_loop::KnownNode;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SwarmErrorType {
    Initialization,
    RelaySetup,
    BehaviourSetup
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Error {
    Base64EncodingError { error: String, contents: Vec<u8> },
    Base64DecodingError { error: String, contents: String },
    KeypairError(String),
    SyncError(String),
    JsonDecodingError { error: String, contents: Value },
    JsonEncodingError(String),
    SwarmError{kind: SwarmErrorType, reason: String},
    DialError{error: String, address: Multiaddr},
    AlreadyInitialized,
    ChannelClosed,
    InvalidMultiAddr(String),
    InvalidPeerId(String),
    InvalidClass(String),
    TransportError(String),
    RegistrationFailed{peer: PeerId, error: String},
    ExpiredRequest,
    NotInitialized,
    ChannelFailure(String)
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Internal error occurred: {self:?}")
    }
}

pub type PodResult<T> = Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CommandKind {
    GetKnownNodes,
    SendRequest{target: PeerId, request: NodeRequest},
    SendResponse{response: NodeResponse}
}

#[derive(Clone, Debug)]
pub struct Command {
    pub command: CommandKind,
    pub response_channel: Sender<PodResult<Value>>
}

impl Command {
    pub async fn reply<T: Serialize + DeserializeOwned>(&self, result: PodResult<T>) {
        let _ = self.response_channel.send(result.and_then(|v| serde_json::to_value(v).or_else(|e| Err(Error::JsonEncodingError(e.to_string()))))).await;
        ()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventKind {
    Error(Error),
    PeerUpdate(KnownNode),
    RegisteringAt(KnownNode),
    RegisteredAt(KnownNode),
    RegistrationFailed {node: KnownNode, reason: String},
    Listening(Multiaddr),
    Connected(KnownNode),
    Discovered {rendezvous: KnownNode, peer: KnownNode},
    ReceivedRequest {source: KnownNode, request: NodeRequest},
    ReceivedResponse(NodeResponse)
}

impl EventKind {
    pub fn wrap(&self) -> Event {
        Event {
            id: Uuid::new_v4(),
            event: self.clone()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub event: EventKind
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct NodeRequest {
    pub id: Uuid,
    pub source: PeerId,
    pub content: Value,
}

impl NodeRequest {
    pub fn unwrap<T: Serialize + DeserializeOwned>(&self) -> PodResult<T> {
        Ok(
            serde_json::from_value::<T>(self.content.clone()).or_else(|e| {
                Err(Error::JsonDecodingError {
                    error: e.to_string(),
                    contents: self.content.clone(),
                })
            })?,
        )
    }

    pub fn new<T: Serialize + DeserializeOwned>(
        source: PeerId,
        content: T,
    ) -> PodResult<NodeRequest> {
        Ok(NodeRequest {
            id: Uuid::new_v4(),
            source: source.clone(),
            content: serde_json::to_value(content)
                .or_else(|e| Err(Error::JsonEncodingError(e.to_string())))?,
        })
    }

    pub fn respond<T: Serialize + DeserializeOwned, E: Serialize + DeserializeOwned>(&self, result: Result<T, E>) -> PodResult<NodeResponse> {
        NodeResponse::respond(self.clone(), result)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct NodeResponse {
    pub request_id: Uuid,
    pub content: Result<Value, Value>,
}

impl NodeResponse {
    pub fn respond<T: Serialize + DeserializeOwned, E: Serialize + DeserializeOwned>(
        request: NodeRequest,
        result: Result<T, E>,
    ) -> PodResult<NodeResponse> {
        let encoded_result = match result {
            Ok(t) => Ok(serde_json::to_value(t)
                .or_else(|e| Err(Error::JsonEncodingError(e.to_string())))?),
            Err(e) => Err(serde_json::to_value(e)
                .or_else(|e| Err(Error::JsonEncodingError(e.to_string())))?),
        };

        Ok(NodeResponse {
            request_id: request.id.clone(),
            content: encoded_result,
        })
    }
}
