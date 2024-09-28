use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Error {
    Base64EncodingError{error: String, contents: Vec<u8>},
    Base64DecodingError{error: String, contents: String},
    KeypairError(String),
    SyncError(String)
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Internal error occurred: {self:?}")
    }
}

pub type PodResult<T> = Result<T, Error>;