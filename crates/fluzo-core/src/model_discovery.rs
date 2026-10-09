use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 3;
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

/// Where a discovery endpoint's resolved addresses actually live.
///
/// Classification is deliberately conservative: a name that resolves to any
/// public address is `Public`, and a mix of loopback and non-loopback addresses is
/// `Public` too, so a caller can never under-report how far the traffic may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AddressClass {
    Loopback,
    Private,
    Public,
    Unresolved,
}

/// What the operator is about to send before any request is made.
///
/// `resolved` holds address literals and `endpoint` is the normalized URL the
/// operator typed. Neither field may carry a credential: `credential_attached`
/// says whether an authorization header will be attached without naming or
/// including its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub endpoint: String,
    pub resolved: Vec<String>,
    pub class: AddressClass,
    pub credential_attached: bool,
}

/// Resolve-only request: the runtime answers with a [`Notice`] and never connects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub protocol: u32,
    pub id: u64,
    pub endpoint: String,
    pub authorization_env: Option<String>,
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
    /// A non-loopback endpoint was submitted with no acceptance for that exact
    /// endpoint and address class. Carries no endpoint and no credential
    /// reference, so a declined attempt is recordable without leaking either.
    ConsentRequired,
    /// The address class resolved at connect time differs from the accepted one.
    /// No request was sent.
    AddressClassChanged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Unknown,
    Pending,
    /// Resolution finished and no connect was performed.
    Notice(Notice),
    Complete(Result<Vec<Model>, Error>),
}

pub trait ModelDiscoveryPort {
    /// Resolve and classify an endpoint without connecting. Delivered as
    /// [`Status::Notice`]; a probe never establishes a socket.
    fn probe(&mut self, probe: Probe) -> Result<(), Error>;
    /// Record that the operator accepted `endpoint` as class `class` for this run
    /// only. Acceptance is never persisted and never transfers to another endpoint.
    fn accept(&mut self, endpoint: &str, class: AddressClass) -> Result<(), Error>;
    fn submit(&mut self, request: Request) -> Result<(), Error>;
    fn status(&mut self, id: u64) -> Status;
    fn cancel(&mut self, id: u64);
}

pub struct Unavailable;

impl ModelDiscoveryPort for Unavailable {
    fn probe(&mut self, _: Probe) -> Result<(), Error> {
        Err(Error::Unavailable)
    }
    fn accept(&mut self, _: &str, _: AddressClass) -> Result<(), Error> {
        Err(Error::Unavailable)
    }
    fn submit(&mut self, _: Request) -> Result<(), Error> {
        Err(Error::Unavailable)
    }
    fn status(&mut self, _: u64) -> Status {
        Status::Unknown
    }
    fn cancel(&mut self, _: u64) {}
}
