//! RUN-04 P1: read-only session open and deterministic replay.
//!
//! PRD 22.1 makes reopening a persisted session a read-only operation:
//! it MUST NOT contact a model or decision provider, execute a tool,
//! retry a request, restore a file or dispatch a queued action. PRD 24
//! adds that replay is a rendering of persisted events, not a
//! re-execution, and that gaps and unknown outcomes stay visible.
//!
//! Two structural properties carry that contract rather than a
//! convention:
//!
//! 1. The reader only ever holds a `SQLITE_OPEN_READONLY` connection
//!    ([`Store::open_read_only`]), so a write fails at the driver.
//! 2. This module imports no provider, tool, process-spawn or dispatch
//!    type, so dispatching is not merely unused here, it is not
//!    expressible. Adding a dispatch path requires importing a module
//!    this file deliberately does not name.
//!
//! Records are hand-encoded JSON through `serde_json::Value`. The runtime
//! does not depend on `serde` derive in production, so the envelope is
//! written out explicitly here; the state fields reuse the core
//! `AttemptState` / `OperationState` enums, which already carry
//! `Interrupted` and `Unknown` for exactly these recovery cases.

use crate::store::{Store, StoreError, StoredEvent};
use fluzo_core::application::{AttemptState, OperationState};
use serde_json::{Map, Value};
use std::path::Path;

/// State-row kinds owned by this layer.
pub const SESSION_KIND: &str = "session";
pub const ATTEMPT_KIND: &str = "attempt";
pub const OPERATION_KIND: &str = "operation";

/// Envelope version of the records this layer writes and reads.
pub const RECORD_VERSION: u64 = 1;

/// Upper bound on records one read may load, so an inspection request
/// cannot be made to allocate proportionally to a hostile journal.
pub const MAX_RECORDS: i64 = 10_000;

/// Upper bound on journal records scanned for one replay.
pub const MAX_REPLAY_EVENTS: i64 = 50_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryError {
    Store(StoreError),
    /// The requested session id has no record.
    UnknownSession(String),
    /// The journal holds more rows than the read bound; the caller must
    /// narrow rather than the reader silently truncating a timeline.
    TooManyRecords {
        kind: &'static str,
        bound: i64,
    },
    /// A replay would exceed the event scan bound.
    ReplayTooLarge {
        bound: i64,
    },
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "store: {error}"),
            Self::UnknownSession(id) => write!(formatter, "unknown session {id:?}"),
            Self::TooManyRecords { kind, bound } => {
                write!(formatter, "more than {bound} {kind} records")
            }
            Self::ReplayTooLarge { bound } => {
                write!(formatter, "replay exceeds {bound} journal records")
            }
        }
    }
}

impl std::error::Error for RecoveryError {}

impl From<StoreError> for RecoveryError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Durable facts about one session that recovery needs.
///
/// Deliberately absent: any YOLO field. RUN-02 keeps session YOLO in
/// process memory with no settings key, so nothing here could restore
/// it even if a caller asked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionRecord {
    pub session_id: String,
    /// Workspace fingerprint recorded at write time, from
    /// `WorkspaceIdentity::fingerprint()`. Resume compares this against
    /// the live workspace and must not substitute another directory.
    pub workspace_fingerprint: String,
    pub git_branch: Option<String>,
    pub git_revision: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl SessionRecord {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("v".into(), Value::from(RECORD_VERSION));
        map.insert("session_id".into(), Value::from(self.session_id.clone()));
        map.insert(
            "workspace_fingerprint".into(),
            Value::from(self.workspace_fingerprint.clone()),
        );
        insert_optional(&mut map, "git_branch", self.git_branch.as_deref());
        insert_optional(&mut map, "git_revision", self.git_revision.as_deref());
        map.insert("created_at_ms".into(), Value::from(self.created_at_ms));
        map.insert("updated_at_ms".into(), Value::from(self.updated_at_ms));
        Value::Object(map)
    }

    /// Durable record of one attempt.
    pub fn encode(&self) -> String {
        self.to_value().to_string()
    }
}

/// Durable record of one attempt of one task within a session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub session_id: String,
    pub task_id: String,
    /// Links a resume-created attempt back to the attempt it followed.
    pub prior_attempt_id: Option<String>,
    pub state: AttemptState,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub budget: BudgetRecord,
}

impl AttemptRecord {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("v".into(), Value::from(RECORD_VERSION));
        map.insert("attempt_id".into(), Value::from(self.attempt_id.clone()));
        map.insert("session_id".into(), Value::from(self.session_id.clone()));
        map.insert("task_id".into(), Value::from(self.task_id.clone()));
        insert_optional(
            &mut map,
            "prior_attempt_id",
            self.prior_attempt_id.as_deref(),
        );
        map.insert(
            "state".into(),
            serde_json::to_value(self.state).unwrap_or(Value::Null),
        );
        map.insert("started_at_ms".into(), Value::from(self.started_at_ms));
        insert_optional_ms(&mut map, "ended_at_ms", self.ended_at_ms);
        map.insert("budget".into(), self.budget.to_value());
        Value::Object(map)
    }

    pub fn encode(&self) -> String {
        self.to_value().to_string()
    }

    /// The state recovery should treat this attempt as being in, without
    /// rewriting the stored record.
    ///
    /// `Running` with no terminal record means the writer stopped before
    /// recording an end. That is `Interrupted`, not "still running":
    /// this reader cannot run anything, so nothing it reads is live.
    /// Reading a record is not evidence of liveness.
    pub fn effective_state(&self) -> AttemptState {
        match self.state {
            AttemptState::Running if self.ended_at_ms.is_none() => AttemptState::Interrupted,
            other => other,
        }
    }
}

/// Consumed budget carried by a durable attempt record.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BudgetRecord {
    pub tokens: u64,
    pub active_seconds: u64,
    pub tool_calls: u64,
    pub turns: u64,
    /// Set when the totals could not be confirmed at the crash boundary.
    /// Uncertain usage is disclosed and never silently trusted as a
    /// known figure that leaves room to continue.
    pub uncertain: bool,
}

impl BudgetRecord {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("tokens".into(), Value::from(self.tokens));
        map.insert("active_seconds".into(), Value::from(self.active_seconds));
        map.insert("tool_calls".into(), Value::from(self.tool_calls));
        map.insert("turns".into(), Value::from(self.turns));
        map.insert("uncertain".into(), Value::from(self.uncertain));
        Value::Object(map)
    }
}

/// Durable record of one operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationRecord {
    pub operation_id: String,
    pub attempt_id: String,
    pub session_id: String,
    /// Stable subject description, e.g. `file:src/lib.rs` or
    /// `command:/bin/cat`. Recovery reports it; it is not a grant.
    pub subject: String,
    pub state: OperationState,
    /// A side effect may have been performed. `true` with
    /// `result_known == false` is the crash-after-effect boundary and
    /// must never be read as "safe to repeat".
    pub effect_committed: bool,
    pub result_known: bool,
    /// The grant consumed for this operation, if one was. Recovery uses
    /// this to refuse a later reuse rather than carrying it forward.
    pub grant_id: Option<String>,
}

impl OperationRecord {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("v".into(), Value::from(RECORD_VERSION));
        map.insert(
            "operation_id".into(),
            Value::from(self.operation_id.clone()),
        );
        map.insert("attempt_id".into(), Value::from(self.attempt_id.clone()));
        map.insert("session_id".into(), Value::from(self.session_id.clone()));
        map.insert("subject".into(), Value::from(self.subject.clone()));
        map.insert(
            "state".into(),
            serde_json::to_value(self.state).unwrap_or(Value::Null),
        );
        map.insert(
            "effect_committed".into(),
            Value::from(self.effect_committed),
        );
        map.insert("result_known".into(), Value::from(self.result_known));
        insert_optional(&mut map, "grant_id", self.grant_id.as_deref());
        Value::Object(map)
    }

    pub fn encode(&self) -> String {
        self.to_value().to_string()
    }

    /// Same rule as attempts: a dispatched operation with no confirmed
    /// result is `Unknown`, never `Succeeded`, `Failed` or repeatable.
    pub fn effective_state(&self) -> OperationState {
        match self.state {
            OperationState::Dispatched if !self.result_known => OperationState::Unknown,
            other => other,
        }
    }
}

/// One rendered line of a session replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineEntry {
    /// Per-session ordinal, not the global journal sequence.
    pub ordinal: u64,
    pub at_ms: u64,
    pub kind: String,
    pub text: String,
    /// Terminal status rendered from durable state when the record has
    /// one; `None` means nothing durable says how it ended.
    pub status: Option<String>,
}

/// A hole or unreadable record in a replay, surfaced rather than hidden.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayGap {
    /// Per-session ordinals `from..to` are absent from the journal.
    MissingRecords { from: u64, to: u64, count: u64 },
    /// A record names an attempt with no state row.
    MissingAttempt { attempt_id: String, ordinal: u64 },
    /// A record names an operation with no state row.
    MissingOperation { operation_id: String, ordinal: u64 },
    /// A record could not be decoded, so its content is unknown.
    Undecodable { ordinal: u64, reason: String },
}

/// A rendered replay plus the holes found while rendering it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Replay {
    pub entries: Vec<TimelineEntry>,
    pub gaps: Vec<ReplayGap>,
    /// Highest per-session ordinal rendered, for cursor handoff.
    pub last_ordinal: u64,
}

/// A session plus everything linked to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredSession {
    pub session: SessionRecord,
    pub attempts: Vec<AttemptRecord>,
    pub operations: Vec<OperationRecord>,
}

/// A read-only view of one workspace journal.
///
/// Obtained only through [`RecoveryReader::open`], which never creates,
/// never migrates and never takes the writer lock.
#[derive(Debug)]
pub struct RecoveryReader {
    store: Store,
}

impl RecoveryReader {
    /// Opens the existing journal read-only. See
    /// [`Store::open_read_only`] for what is refused and why.
    pub async fn open(workspace_root: &Path) -> Result<Self, RecoveryError> {
        Ok(Self {
            store: Store::open_read_only(workspace_root).await?,
        })
    }

    /// Deterministic close, for the same reason `History::close` exists:
    /// a dropped connection defers the real close and can leave the WAL
    /// index lock held against the next opener.
    pub async fn close(self) -> Result<(), StoreError> {
        self.store.close().await
    }

    /// Lists recorded sessions ordered by id, deterministically.
    pub async fn list(&mut self) -> Result<Vec<SessionRecord>, RecoveryError> {
        let rows = self
            .store
            .states_of_kind(SESSION_KIND, MAX_RECORDS + 1)
            .await?;
        if rows.len() as i64 > MAX_RECORDS {
            return Err(RecoveryError::TooManyRecords {
                kind: SESSION_KIND,
                bound: MAX_RECORDS,
            });
        }
        let mut out = Vec::with_capacity(rows.len());
        for (id, payload) in rows {
            out.push(decode_session(&id, &payload)?);
        }
        out.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(out)
    }

    /// Loads one session with its attempts and operations.
    pub async fn session(&mut self, session_id: &str) -> Result<RecoveredSession, RecoveryError> {
        let session = match self.store.state(session_id).await? {
            Some((_, payload, _)) => decode_session(session_id, &payload)?,
            None => return Err(RecoveryError::UnknownSession(session_id.to_owned())),
        };
        let mut attempts = Vec::new();
        for (id, payload) in self
            .store
            .states_of_kind(ATTEMPT_KIND, MAX_RECORDS + 1)
            .await?
        {
            if payload.contains(&format!("\"session_id\":\"{session_id}\"")) {
                attempts.push(decode_attempt(&id, &payload)?);
            }
        }
        let mut operations = Vec::new();
        for (id, payload) in self
            .store
            .states_of_kind(OPERATION_KIND, MAX_RECORDS + 1)
            .await?
        {
            if payload.contains(&format!("\"session_id\":\"{session_id}\"")) {
                operations.push(decode_operation(&id, &payload)?);
            }
        }
        attempts.sort_by(|left, right| left.attempt_id.cmp(&right.attempt_id));
        operations.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        Ok(RecoveredSession {
            session,
            attempts,
            operations,
        })
    }

    /// Renders the persisted journal for one session.
    ///
    /// Deterministic reconstruction from the same recorded events: no
    /// provider call, no tool run, no dispatch, no re-execution. Two
    /// calls over an unchanged journal return the same value.
    pub async fn replay(&mut self, session_id: &str) -> Result<Replay, RecoveryError> {
        let session = self.session(session_id).await?;
        let attempts: std::collections::BTreeMap<&str, &AttemptRecord> = session
            .attempts
            .iter()
            .map(|record| (record.attempt_id.as_str(), record))
            .collect();
        let operations: std::collections::BTreeMap<&str, &OperationRecord> = session
            .operations
            .iter()
            .map(|record| (record.operation_id.as_str(), record))
            .collect();

        let mut replay = Replay::default();
        let mut previous: Option<u64> = None;
        let mut after = 0_i64;
        loop {
            let batch = self
                .store
                .events_after(after, MAX_REPLAY_EVENTS + 1)
                .await?;
            if batch.len() as i64 > MAX_REPLAY_EVENTS {
                return Err(RecoveryError::ReplayTooLarge {
                    bound: MAX_REPLAY_EVENTS,
                });
            }
            if batch.is_empty() {
                break;
            }
            let last_seq = batch.last().map(|event| event.seq).unwrap_or(after);
            for event in batch {
                if event.state_id != session_id {
                    continue;
                }
                match decode_journal_record(&event) {
                    Ok(record) => {
                        if let Some(previous) = previous {
                            if record.ordinal <= previous {
                                replay.gaps.push(ReplayGap::Undecodable {
                                    ordinal: record.ordinal,
                                    reason: "ordinal is not ahead of the previous record"
                                        .to_owned(),
                                });
                                continue;
                            }
                            if record.ordinal > previous + 1 {
                                replay.gaps.push(ReplayGap::MissingRecords {
                                    from: previous + 1,
                                    to: record.ordinal - 1,
                                    count: record.ordinal - previous - 1,
                                });
                            }
                        }
                        if let Some(attempt_id) = record.attempt_id.as_deref()
                            && !attempts.contains_key(attempt_id)
                        {
                            replay.gaps.push(ReplayGap::MissingAttempt {
                                attempt_id: attempt_id.to_owned(),
                                ordinal: record.ordinal,
                            });
                        }
                        if let Some(operation_id) = record.operation_id.as_deref()
                            && !operations.contains_key(operation_id)
                        {
                            replay.gaps.push(ReplayGap::MissingOperation {
                                operation_id: operation_id.to_owned(),
                                ordinal: record.ordinal,
                            });
                        }
                        let status = record
                            .attempt_id
                            .as_deref()
                            .and_then(|id| attempts.get(id))
                            .map(|record| format!("{:?}", record.effective_state()))
                            .or_else(|| {
                                record
                                    .operation_id
                                    .as_deref()
                                    .and_then(|id| operations.get(id))
                                    .map(|record| format!("{:?}", record.effective_state()))
                            });
                        replay.entries.push(TimelineEntry {
                            ordinal: record.ordinal,
                            at_ms: record.at_ms,
                            kind: event.kind.clone(),
                            text: record.text,
                            status,
                        });
                        replay.last_ordinal = record.ordinal;
                        previous = Some(record.ordinal);
                    }
                    Err(reason) => {
                        replay.gaps.push(ReplayGap::Undecodable {
                            ordinal: u64::try_from(event.seq).unwrap_or(0),
                            reason,
                        });
                    }
                }
            }
            after = last_seq;
        }
        Ok(replay)
    }
}

/// One decoded journal record.
struct JournalRecord {
    ordinal: u64,
    at_ms: u64,
    text: String,
    attempt_id: Option<String>,
    operation_id: Option<String>,
}

fn decode_journal_record(event: &StoredEvent) -> Result<JournalRecord, String> {
    let value: Value =
        serde_json::from_str(&event.payload).map_err(|_| "payload is not JSON".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "payload is not a JSON object".to_owned())?;
    let ordinal = object
        .get("ordinal")
        .and_then(Value::as_u64)
        .ok_or_else(|| "missing ordinal".to_owned())?;
    let at_ms = object.get("at_ms").and_then(Value::as_u64).unwrap_or(0);
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(JournalRecord {
        ordinal,
        at_ms,
        text,
        attempt_id: string_field(object, "attempt_id"),
        operation_id: string_field(object, "operation_id"),
    })
}

/// Builds the JSON payload for one journal record, so writers and this
/// reader agree on the shape without a shared derive.
pub fn journal_record_payload(
    ordinal: u64,
    at_ms: u64,
    text: &str,
    attempt_id: Option<&str>,
    operation_id: Option<&str>,
) -> String {
    let mut map = Map::new();
    map.insert("v".into(), Value::from(RECORD_VERSION));
    map.insert("ordinal".into(), Value::from(ordinal));
    map.insert("at_ms".into(), Value::from(at_ms));
    map.insert("text".into(), Value::from(text));
    insert_optional(&mut map, "attempt_id", attempt_id);
    insert_optional(&mut map, "operation_id", operation_id);
    Value::Object(map).to_string()
}

fn decode_session(id: &str, payload: &str) -> Result<SessionRecord, RecoveryError> {
    let object = parse_object(payload)?;
    Ok(SessionRecord {
        session_id: string_field(&object, "session_id").unwrap_or_else(|| id.to_owned()),
        workspace_fingerprint: string_field(&object, "workspace_fingerprint").unwrap_or_default(),
        git_branch: string_field(&object, "git_branch"),
        git_revision: string_field(&object, "git_revision"),
        created_at_ms: number_field(&object, "created_at_ms"),
        updated_at_ms: number_field(&object, "updated_at_ms"),
    })
}

fn decode_attempt(id: &str, payload: &str) -> Result<AttemptRecord, RecoveryError> {
    let object = parse_object(payload)?;
    Ok(AttemptRecord {
        attempt_id: string_field(&object, "attempt_id").unwrap_or_else(|| id.to_owned()),
        session_id: string_field(&object, "session_id").unwrap_or_default(),
        task_id: string_field(&object, "task_id").unwrap_or_default(),
        prior_attempt_id: string_field(&object, "prior_attempt_id"),
        state: attempt_state_field(&object, "state")?,
        started_at_ms: number_field(&object, "started_at_ms"),
        ended_at_ms: object.get("ended_at_ms").and_then(Value::as_u64),
        budget: decode_budget(object.get("budget")),
    })
}

fn decode_operation(id: &str, payload: &str) -> Result<OperationRecord, RecoveryError> {
    let object = parse_object(payload)?;
    Ok(OperationRecord {
        operation_id: string_field(&object, "operation_id").unwrap_or_else(|| id.to_owned()),
        attempt_id: string_field(&object, "attempt_id").unwrap_or_default(),
        session_id: string_field(&object, "session_id").unwrap_or_default(),
        subject: string_field(&object, "subject").unwrap_or_default(),
        state: operation_state_field(&object, "state")?,
        effect_committed: object
            .get("effect_committed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        result_known: object
            .get("result_known")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        grant_id: string_field(&object, "grant_id"),
    })
}

fn decode_budget(value: Option<&Value>) -> BudgetRecord {
    let Some(object) = value.and_then(Value::as_object) else {
        return BudgetRecord {
            uncertain: true,
            ..BudgetRecord::default()
        };
    };
    BudgetRecord {
        tokens: number_field(object, "tokens"),
        active_seconds: number_field(object, "active_seconds"),
        tool_calls: number_field(object, "tool_calls"),
        turns: number_field(object, "turns"),
        uncertain: object
            .get("uncertain")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    }
}

fn parse_object(payload: &str) -> Result<Map<String, Value>, RecoveryError> {
    let value: Value =
        serde_json::from_str(payload).map_err(|_| RecoveryError::Store(StoreError::Corrupt))?;
    value
        .as_object()
        .cloned()
        .ok_or(RecoveryError::Store(StoreError::Corrupt))
}

fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.is_empty() && value != "none")
}

fn number_field(object: &Map<String, Value>, key: &str) -> u64 {
    object.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Concrete decoders rather than a generic over `serde::DeserializeOwned`:
/// the runtime does not depend on `serde` directly in production, so the
/// trait must not be named here. The impls come from `fluzo-core` and are
/// reached through `serde_json`.
fn attempt_state_field(
    object: &Map<String, Value>,
    key: &str,
) -> Result<AttemptState, RecoveryError> {
    let value = object
        .get(key)
        .cloned()
        .ok_or(RecoveryError::Store(StoreError::Corrupt))?;
    serde_json::from_value(value).map_err(|_| RecoveryError::Store(StoreError::Corrupt))
}

fn operation_state_field(
    object: &Map<String, Value>,
    key: &str,
) -> Result<OperationState, RecoveryError> {
    let value = object
        .get(key)
        .cloned()
        .ok_or(RecoveryError::Store(StoreError::Corrupt))?;
    serde_json::from_value(value).map_err(|_| RecoveryError::Store(StoreError::Corrupt))
}

fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    map.insert(
        key.to_owned(),
        value.map(Value::from).unwrap_or(Value::from("none")),
    );
}

fn insert_optional_ms(map: &mut Map<String, Value>, key: &str, value: Option<u64>) {
    match value {
        Some(value) => {
            map.insert(key.to_owned(), Value::from(value));
        }
        None => {
            map.insert(key.to_owned(), Value::Null);
        }
    }
}
