use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::settings::{
    SettingDescriptor, SettingOrigin, SettingValue, TuiSettings, ValidationError,
};

pub const CONFIGURATION_PROTOCOL: u32 = 4;
pub const MAX_EDITS: usize = 256;
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_REQUESTS: usize = 64;
pub const MAX_PENDING: usize = 8;
pub const MAX_CHANGES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub instance: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationRequestId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    Set { key: String, value: SettingValue },
    AddModel { name: String },
    RemoveModel { name: String },
    AddPool { name: String },
    RemovePool { name: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConfigurationAction {
    Reload,
    PrepareSetup {
        expected: Version,
        edits: Vec<Edit>,
    },
    ConfirmSetup {
        expected: Version,
    },
    Edit {
        expected: Version,
        edits: Vec<Edit>,
    },
    Cancel {
        expected: Version,
    },
    Save {
        expected: Version,
        keys: Vec<String>,
    },
    Apply {
        expected: Version,
        keys: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationRequest {
    pub protocol: u32,
    pub id: ConfigurationRequestId,
    pub action: ConfigurationAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConfigErrorCode {
    TooLarge,
    Syntax,
    UnknownField,
    InvalidType,
    InvalidValue,
    MissingVersion,
    Validation,
    Serialization,
    InvalidEncoding,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConfigurationError {
    UnsupportedProtocol,
    InvalidRequest,
    Capacity,
    Busy,
    Unavailable,
    UnsupportedPath,
    ReadOnly,
    Conflict,
    Invalid {
        code: ConfigErrorCode,
        key: String,
        span: Option<std::ops::Range<usize>>,
        errors: Vec<ValidationError>,
    },
    Inaccessible,
    WriteFailed,
    Uncertain,
    CliLocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Discovery {
    Missing,
    Valid,
    Invalid,
    Inaccessible,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConfigurationOutcome {
    Reloaded,
    DraftUpdated,
    Cancelled,
    Saved,
    Applied,
    RestartPending,
    SetupPrepared,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConfigurationRequestStatus {
    Unknown,
    Accepted,
    Completed {
        version: Version,
        outcome: ConfigurationOutcome,
    },
    Failed(ConfigurationError),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationValue {
    pub descriptor: SettingDescriptor,
    pub saved: SettingValue,
    pub draft: SettingValue,
    pub effective: SettingValue,
    pub saved_origin: SettingOrigin,
    pub effective_origin: SettingOrigin,
    pub draft_origin: SettingOrigin,
    pub cli_locked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationChange {
    pub version: Version,
    pub request: ConfigurationRequestId,
    pub outcome: ConfigurationOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationSnapshot {
    pub save_unavailable: Option<ConfigurationError>,
    pub apply_unavailable: Option<ConfigurationError>,
    pub remaining_requests: usize,
    pub next_request_id: ConfigurationRequestId,
    pub version: Version,
    pub discovery: Discovery,
    pub problem: Option<ConfigurationError>,
    pub values: BTreeMap<String, ConfigurationValue>,
    pub effective_ui: TuiSettings,
    pub pending_restart: Vec<String>,
    pub changes: Vec<ConfigurationChange>,
    pub dropped_changes: u64,
    pub setup: Option<SetupPreview>,
    pub backup: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetupPreview {
    pub replacing: bool,
    pub values: BTreeMap<String, SettingValue>,
}

pub trait ConfigurationPort {
    fn submit(
        &mut self,
        request: ConfigurationRequest,
    ) -> Result<ConfigurationRequestId, ConfigurationError>;
    fn status(
        &mut self,
        request: ConfigurationRequestId,
    ) -> Result<ConfigurationRequestStatus, ConfigurationError>;
    fn snapshot(&mut self) -> Result<ConfigurationSnapshot, ConfigurationError>;
}
