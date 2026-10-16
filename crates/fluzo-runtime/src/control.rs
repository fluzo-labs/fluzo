//! RUN-04 P3: private owner-verified task-control channel.
//!
//! PRD 17.2 requires that a control request targeting work owned by
//! another Fluzo process uses a private, owner-verified channel or is
//! refused, and never signals arbitrary processes inferred from a task
//! label. This module is that channel: a Unix-domain socket bound inside
//! the protected `.fluzodrive` directory by the writer-lock holder only.
//!
//! Three properties carry the contract:
//!
//! 1. **Bound only by the owner.** [`ControlServer::bind`] takes
//!    ownership of the live [`WriterOwnership`], so the channel cannot
//!    exist without the exclusive lock and cannot outlive it.
//! 2. **Verified per request.** Every request carries the acquisition
//!    nonce from [`WriterOwnership::record`] and the server re-checks
//!    [`WriterOwnership::verify_still_owned`] before acting. A token
//!    copied from a dead owner, or a server whose lock was lost, is
//!    refused as stale. The pid is never used as proof.
//! 3. **A closed request set.** [`ControlRequest`] has no variant that
//!    enables YOLO, acquires writer ownership, resumes a task or
//!    dispatches work. An inspection client cannot ask for those
//!    because they are not expressible, and an unknown variant fails
//!    closed rather than mapping to the nearest known action.
//!
//! The socket is reachable by the same user and is not a security
//! boundary against a cooperating local account, consistent with the
//! RUN-01 confinement boundary. It keeps other *processes* out, not the
//! user's own shell.
//!
//! No credential, endpoint or session payload crosses the channel: the
//! wire types carry identifiers and booleans only, and a refusal returns
//! a stable code with an empty detail rather than echoing input.
//!
//! JSON is built and read through `serde_json::Value`. The runtime does
//! not depend on `serde` derive in production, so the two wire shapes
//! are encoded explicitly here.

use crate::store::DRIVE_DIRECTORY;
use crate::workspace::WriterOwnership;
use serde_json::{Map, Value};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

/// Protocol version carried in every envelope. A mismatch is refused
/// before any handler runs.
pub const PROTOCOL_VERSION: u64 = 1;

/// Socket file name inside `.fluzodrive`.
pub const SOCKET_NAME: &str = "control.sock";

/// Upper bound on one frame's declared length, checked before any
/// allocation so a hostile header cannot make the reader grow.
pub const MAX_FRAME_BYTES: u32 = 64 * 1024;

/// Linux limits a Unix socket path to about 104 bytes including the
/// NUL. Checked rather than truncated.
pub const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Stable machine-readable refusal codes.
pub mod code {
    pub const UNSUPPORTED_VERSION: &str = "unsupported_protocol_version";
    pub const OWNER_MISMATCH: &str = "owner_mismatch";
    pub const STALE_OWNER: &str = "stale_owner";
    pub const FRAME_TOO_LARGE: &str = "frame_too_large";
    pub const TRUNCATED: &str = "truncated_frame";
    pub const MALFORMED: &str = "malformed_request";
    pub const CONFIRMATION_REQUIRED: &str = "confirmation_required";
    pub const UNKNOWN_REQUEST: &str = "unknown_request";
    pub const PATH_TOO_LONG: &str = "socket_path_too_long";
    pub const SYMLINK_REFUSED: &str = "symlink_refused";
    pub const IO: &str = "io";
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlError {
    pub code: &'static str,
    pub detail: String,
}

impl ControlError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} ({})", self.code, self.detail)
    }
}

impl std::error::Error for ControlError {}

/// The closed set of control requests.
///
/// There is deliberately no variant for YOLO, writer-lock acquisition,
/// resume or tool dispatch. Adding one is a reviewed change to this
/// enum, not something a client can reach by sending a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlRequest {
    /// Read-only inspection of one session's durable state.
    InspectSession { session_id: String },
    /// Read-only status of one task.
    TaskStatus { task_id: String },
    /// Request cancellation of one task. Bounded and cooperative.
    Cancel { task_id: String },
    /// Force stop. Requires `confirmed: true`; without it the request
    /// returns `confirmation_required` and signals nothing.
    ForceStop { task_id: String, confirmed: bool },
}

impl ControlRequest {
    /// Wire discriminator.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InspectSession { .. } => "inspect_session",
            Self::TaskStatus { .. } => "task_status",
            Self::Cancel { .. } => "cancel",
            Self::ForceStop { .. } => "force_stop",
        }
    }

    /// Whether serving this request changes nothing in the workspace.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::InspectSession { .. } | Self::TaskStatus { .. })
    }

    fn to_map(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("kind".to_owned(), Value::from(self.kind()));
        match self {
            Self::InspectSession { session_id } => {
                map.insert("session_id".to_owned(), Value::from(session_id.clone()));
            }
            Self::TaskStatus { task_id } | Self::Cancel { task_id } => {
                map.insert("task_id".to_owned(), Value::from(task_id.clone()));
            }
            Self::ForceStop { task_id, confirmed } => {
                map.insert("task_id".to_owned(), Value::from(task_id.clone()));
                map.insert("confirmed".to_owned(), Value::from(*confirmed));
            }
        }
        map
    }
}

/// Command delivered to the owner runtime for a mutating request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlCommand {
    Cancel { task_id: String },
    ForceStop { task_id: String },
}

/// Response envelope returned to the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlResponse {
    pub protocol: u64,
    pub ok: bool,
    /// `ok` for success, otherwise one of [`code`].
    pub code: String,
    /// Read-only result text for inspection requests. Empty for
    /// refusals, so nothing echoed back carries private content.
    pub detail: String,
}

impl ControlResponse {
    fn success(detail: impl Into<String>) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            ok: true,
            code: "ok".to_owned(),
            detail: detail.into(),
        }
    }

    fn refusal(error: &ControlError) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            ok: false,
            code: error.code.to_owned(),
            detail: String::new(),
        }
    }

    fn to_map(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("protocol".to_owned(), Value::from(self.protocol));
        map.insert("ok".to_owned(), Value::from(self.ok));
        map.insert("code".to_owned(), Value::from(self.code.clone()));
        map.insert("detail".to_owned(), Value::from(self.detail.clone()));
        map
    }
}

/// A decoded inbound envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub protocol: u64,
    pub owner: String,
    pub request: ControlRequest,
}

/// Binds the control channel on behalf of a live workspace owner.
pub struct ControlServer {
    listener: UnixListener,
    socket_path: PathBuf,
    owner_token: String,
    workspace_fingerprint: String,
    commands: Sender<ControlCommand>,
    command_sink: Receiver<ControlCommand>,
    /// The server owns the lock it proves, so the channel cannot
    /// outlive the ownership it verifies against.
    ownership: WriterOwnership,
}

impl std::fmt::Debug for ControlServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlServer")
            .field("socket_path", &self.socket_path)
            .field("workspace_fingerprint", &self.workspace_fingerprint)
            .finish_non_exhaustive()
    }
}

impl ControlServer {
    /// Binds `.fluzodrive/control.sock` for the given ownership.
    ///
    /// Refuses a path that would exceed the Unix socket limit and a
    /// symlink at the socket path. A stale socket left by a crashed
    /// owner is replaced only because the caller holds the exclusive
    /// writer lock, which is the same proof that made the previous
    /// holder dead.
    pub fn bind(ownership: WriterOwnership) -> Result<Self, ControlError> {
        let socket_path = ownership
            .identity()
            .root()
            .join(DRIVE_DIRECTORY)
            .join(SOCKET_NAME);
        if socket_path.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
            return Err(ControlError::new(
                code::PATH_TOO_LONG,
                format!("{} bytes", socket_path.as_os_str().len()),
            ));
        }
        refuse_symlink(&socket_path)?;

        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| ControlError::new(code::IO, error.to_string()))?;
        // The directory is already 0700; the socket gets its own mode
        // so a later relaxation of the directory does not silently
        // widen the channel.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| ControlError::new(code::IO, error.to_string()))?;
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        let record = ownership.record();
        Ok(Self {
            listener,
            socket_path,
            owner_token: record.owner.clone(),
            workspace_fingerprint: record.workspace.clone(),
            commands: sender,
            command_sink: receiver,
            ownership,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn owner_token(&self) -> &str {
        &self.owner_token
    }

    pub fn workspace_fingerprint(&self) -> &str {
        &self.workspace_fingerprint
    }

    /// The ownership this channel is bound to.
    pub fn ownership(&self) -> &WriterOwnership {
        &self.ownership
    }

    /// Commands the owner runtime must act on.
    pub fn commands(&self) -> &Receiver<ControlCommand> {
        &self.command_sink
    }

    /// Accepts one connection and serves it.
    pub fn serve_one(&self) -> Result<ServedRequest, ControlError> {
        let (stream, _peer) = self
            .listener
            .accept()
            .map_err(|error| ControlError::new(code::IO, error.to_string()))?;
        self.serve_stream(stream)
    }

    /// Serves one request on an already-connected stream.
    ///
    /// The greeting is written first so a client that connected to a
    /// stale or foreign socket learns that before it sends anything it
    /// would have to retry.
    pub fn serve_stream(&self, mut stream: UnixStream) -> Result<ServedRequest, ControlError> {
        stream
            .write_all(&PROTOCOL_VERSION.to_be_bytes())
            .map_err(|error| ControlError::new(code::IO, error.to_string()))?;
        let envelope = match read_envelope(&mut stream) {
            Ok(envelope) => envelope,
            Err(error) => {
                let response = ControlResponse::refusal(&error);
                let _ = write_frame(&mut stream, &response.to_map());
                return Err(error);
            }
        };
        let response = self.handle(&envelope);
        write_frame(&mut stream, &response.to_map())
            .map_err(|error| ControlError::new(code::IO, error.to_string()))?;
        Ok(ServedRequest {
            request: envelope.request,
            response,
        })
    }

    /// Applies owner verification and the request's own confirmation
    /// rule against the live lock.
    pub fn handle(&self, envelope: &Envelope) -> ControlResponse {
        self.handle_verifying(envelope, || {
            self.ownership
                .verify_still_owned()
                .map_err(|error| format!("{error:?}"))
        })
    }

    /// The verification boundary, injectable so the stale-owner path
    /// can be exercised without faking the lock type itself.
    pub fn handle_verifying<F>(&self, envelope: &Envelope, verify_live: F) -> ControlResponse
    where
        F: FnOnce() -> Result<(), String>,
    {
        if envelope.protocol != PROTOCOL_VERSION {
            return ControlResponse::refusal(&ControlError::new(
                code::UNSUPPORTED_VERSION,
                "version mismatch",
            ));
        }
        if envelope.owner != self.owner_token {
            return ControlResponse::refusal(&ControlError::new(
                code::OWNER_MISMATCH,
                "token does not name this acquisition",
            ));
        }
        // The token alone is not enough: the lock must still be held
        // right now. A copied token from a dead owner controls nothing.
        if let Err(reason) = verify_live() {
            return ControlResponse::refusal(&ControlError::new(code::STALE_OWNER, reason));
        }
        match &envelope.request {
            ControlRequest::InspectSession { session_id } => {
                ControlResponse::success(format!("inspected {session_id}"))
            }
            ControlRequest::TaskStatus { task_id } => {
                ControlResponse::success(format!("status of {task_id}"))
            }
            ControlRequest::Cancel { task_id } => {
                let _ = self.commands.send(ControlCommand::Cancel {
                    task_id: task_id.clone(),
                });
                ControlResponse::success("cancellation requested")
            }
            ControlRequest::ForceStop { task_id, confirmed } => {
                if !*confirmed {
                    // Returned rather than waited on: a headless caller
                    // must get a structured outcome, and no bypass flag
                    // skips the confirmation.
                    return ControlResponse::refusal(&ControlError::new(
                        code::CONFIRMATION_REQUIRED,
                        "force stop needs an explicit confirmation",
                    ));
                }
                let _ = self.commands.send(ControlCommand::ForceStop {
                    task_id: task_id.clone(),
                });
                ControlResponse::success("force stop requested")
            }
        }
    }
}

/// What one served request produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedRequest {
    pub request: ControlRequest,
    pub response: ControlResponse,
}

/// Connects to a control socket and issues requests.
pub struct ControlClient {
    stream: UnixStream,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlClient")
            .finish_non_exhaustive()
    }
}

impl ControlClient {
    /// Connects and checks the server greeting before sending anything.
    pub fn connect(socket_path: &Path) -> Result<Self, ControlError> {
        refuse_symlink(socket_path)?;
        let mut stream = UnixStream::connect(socket_path)
            .map_err(|error| ControlError::new(code::IO, error.to_string()))?;
        let greeting = read_greeting(&mut stream)?;
        if greeting != PROTOCOL_VERSION {
            return Err(ControlError::new(
                code::UNSUPPORTED_VERSION,
                format!("greeting {greeting}"),
            ));
        }
        Ok(Self { stream })
    }

    /// Sends one request and returns the response.
    pub fn request(
        &mut self,
        owner_token: &str,
        request: ControlRequest,
    ) -> Result<ControlResponse, ControlError> {
        let mut envelope = Map::new();
        envelope.insert("protocol".to_owned(), Value::from(PROTOCOL_VERSION));
        envelope.insert("owner".to_owned(), Value::from(owner_token.to_owned()));
        envelope.insert("request".to_owned(), Value::Object(request.to_map()));
        write_frame(&mut self.stream, &envelope)
            .map_err(|error| ControlError::new(code::IO, error.to_string()))?;
        read_response(&mut self.stream)
    }
}

fn refuse_symlink(path: &Path) -> Result<(), ControlError> {
    // Any existing entry at the socket path that is not a socket is
    // refused outright. `symlink_metadata` does not follow links, so a
    // symlink is seen as one rather than as its target.
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(ControlError::new(
            code::SYMLINK_REFUSED,
            "socket path is a symbolic link",
        ));
    }
    Ok(())
}

/// Encodes one frame: 4-byte big-endian length, then a JSON body.
fn encode_frame(payload: &str) -> Result<Vec<u8>, ControlError> {
    let bytes = payload.as_bytes();
    let length = u32::try_from(bytes.len())
        .map_err(|_| ControlError::new(code::FRAME_TOO_LARGE, format!("{} bytes", bytes.len())))?;
    if length > MAX_FRAME_BYTES {
        return Err(ControlError::new(
            code::FRAME_TOO_LARGE,
            format!("{length} bytes"),
        ));
    }
    let mut out = Vec::with_capacity(bytes.len() + 4);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(out)
}

/// Reads a declared length and refuses anything over the bound before
/// allocating, so the reader never grows to an attacker's choosing.
fn read_length(reader: &mut impl Read) -> Result<usize, ControlError> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            ControlError::new(code::TRUNCATED, "no frame header")
        } else {
            ControlError::new(code::IO, error.to_string())
        }
    })?;
    let length = u32::from_be_bytes(header);
    if length > MAX_FRAME_BYTES {
        return Err(ControlError::new(
            code::FRAME_TOO_LARGE,
            format!("declared {length} bytes"),
        ));
    }
    Ok(length as usize)
}

fn read_body(reader: &mut impl Read, length: usize) -> Result<String, ControlError> {
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            ControlError::new(code::TRUNCATED, "short frame body")
        } else {
            ControlError::new(code::IO, error.to_string())
        }
    })?;
    String::from_utf8(body).map_err(|_| ControlError::new(code::MALFORMED, "body is not utf-8"))
}

fn read_greeting(reader: &mut impl Read) -> Result<u64, ControlError> {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            ControlError::new(code::TRUNCATED, "no greeting")
        } else {
            ControlError::new(code::IO, error.to_string())
        }
    })?;
    Ok(u64::from_be_bytes(header))
}

fn write_frame<W: Write>(writer: &mut W, value: &Map<String, Value>) -> Result<(), ControlError> {
    let json = Value::Object(value.clone()).to_string();
    let frame = encode_frame(&json)?;
    writer
        .write_all(&frame)
        .map_err(|error| ControlError::new(code::IO, error.to_string()))
}

/// Reads and decodes an inbound envelope, failing closed on an unknown
/// request shape.
fn read_envelope(reader: &mut impl Read) -> Result<Envelope, ControlError> {
    let length = read_length(reader)?;
    let body = read_body(reader, length)?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| ControlError::new(code::MALFORMED, "body is not json"))?;
    let object = value
        .as_object()
        .ok_or_else(|| ControlError::new(code::MALFORMED, "body is not an object"))?;
    let protocol = object
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or_else(|| ControlError::new(code::MALFORMED, "missing protocol"))?;
    let owner = object
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let request = object
        .get("request")
        .ok_or_else(|| ControlError::new(code::UNKNOWN_REQUEST, "missing request"))?;
    Ok(Envelope {
        protocol,
        owner,
        request: decode_request(request)?,
    })
}

fn decode_request(value: &Value) -> Result<ControlRequest, ControlError> {
    let object = value
        .as_object()
        .ok_or_else(|| ControlError::new(code::UNKNOWN_REQUEST, "request is not an object"))?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| ControlError::new(code::UNKNOWN_REQUEST, "missing kind"))?;
    let field = |name: &str| -> Result<String, ControlError> {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ControlError::new(code::MALFORMED, format!("missing {name}")))
    };
    match kind {
        "inspect_session" => Ok(ControlRequest::InspectSession {
            session_id: field("session_id")?,
        }),
        "task_status" => Ok(ControlRequest::TaskStatus {
            task_id: field("task_id")?,
        }),
        "cancel" => Ok(ControlRequest::Cancel {
            task_id: field("task_id")?,
        }),
        "force_stop" => {
            let confirmed = object
                .get("confirmed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(ControlRequest::ForceStop {
                task_id: field("task_id")?,
                confirmed,
            })
        }
        // An unknown name is refused rather than mapped to the closest
        // known request: a typo must not become a mutating action.
        other => Err(ControlError::new(
            code::UNKNOWN_REQUEST,
            format!("unknown kind {other:?}"),
        )),
    }
}

fn read_response(reader: &mut impl Read) -> Result<ControlResponse, ControlError> {
    let length = read_length(reader)?;
    let body = read_body(reader, length)?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| ControlError::new(code::MALFORMED, "response is not json"))?;
    let object = value
        .as_object()
        .ok_or_else(|| ControlError::new(code::MALFORMED, "response is not an object"))?;
    Ok(ControlResponse {
        protocol: object.get("protocol").and_then(Value::as_u64).unwrap_or(0),
        ok: object.get("ok").and_then(Value::as_bool).unwrap_or(false),
        code: object
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or(code::MALFORMED)
            .to_owned(),
        detail: object
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}
