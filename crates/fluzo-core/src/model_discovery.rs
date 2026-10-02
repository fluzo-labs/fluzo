use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 2;
pub const MAX_MODELS: usize = 256;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub protocol: u32,
    pub id: u64,
    pub endpoint: String,
    pub authorization_env: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub slots: Option<u32>,
}

impl From<String> for Model {
    fn from(id: String) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }
}
impl From<&str> for Model {
    fn from(id: &str) -> Self {
        id.to_owned().into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    UnsupportedProtocol,
    InvalidEndpoint,
    InvalidAuthorization,
    AuthorizationUnavailable,
    Busy,
    Capacity,
    Unavailable,
    Cancelled,
    Timeout,
    Connection,
    Http(u16),
    TooLarge,
    InvalidResponse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Unknown,
    Pending,
    Complete(Result<Vec<Model>, Error>),
}

pub trait ModelDiscoveryPort {
    fn submit(&mut self, request: Request) -> Result<(), Error>;
    fn status(&mut self, id: u64) -> Status;
    fn cancel(&mut self, id: u64);
}

pub struct Unavailable;

impl ModelDiscoveryPort for Unavailable {
    fn submit(&mut self, _: Request) -> Result<(), Error> {
        Err(Error::Unavailable)
    }
    fn status(&mut self, _: u64) -> Status {
        Status::Unknown
    }
    fn cancel(&mut self, _: u64) {}
}
