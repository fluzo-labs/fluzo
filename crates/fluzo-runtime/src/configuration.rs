use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

use fluzo_core::configuration::*;
use fluzo_core::settings::{ApplicationRule, Privacy, SettingOrigin, SettingValue, Settings};

use crate::config::{ConfigError, encode_settings, parse_settings, update_batch};
pub use crate::configuration_file::WritePolicy;
use crate::configuration_file::{ConfigurationFile, Observation};

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

type Completion = (
    ConfigurationRequestId,
    ConfigurationRequestStatus,
    ConfigurationSnapshot,
);

pub struct ConfigurationService {
    sender: Option<SyncSender<ConfigurationRequest>>,
    receiver: Receiver<Result<Completion, ConfigurationError>>,
    records: Vec<(ConfigurationRequest, ConfigurationRequestStatus)>,
    projection: Option<ConfigurationSnapshot>,
    unavailable: bool,
}

impl ConfigurationService {
    pub fn start(
        workspace: PathBuf,
        explicit: Option<PathBuf>,
        overrides: Vec<Edit>,
        policy: WritePolicy,
    ) -> Result<Self, ConfigurationError> {
        validate_edits(&overrides)?;
        let instance = NEXT_INSTANCE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| ConfigurationError::Capacity)?;
        let (sender, requests) = mpsc::sync_channel::<ConfigurationRequest>(MAX_PENDING);
        let (results, receiver) = mpsc::sync_channel(MAX_PENDING + 1);
        std::thread::Builder::new()
            .name("fluzo-configuration".into())
            .spawn(move || {
                let mut state =
                    match State::open(&workspace, explicit.as_deref(), overrides, policy, instance)
                    {
                        Ok(state) => state,
                        Err(error) => {
                            let _ = results.send(Err(error));
                            return;
                        }
                    };
                if results
                    .send(Ok((
                        ConfigurationRequestId(0),
                        ConfigurationRequestStatus::Unknown,
                        state.snapshot(),
                    )))
                    .is_err()
                {
                    return;
                }
                while let Ok(request) = requests.recv() {
                    let status = match state.execute(&request) {
                        Ok(outcome) => {
                            state.record(request.id, outcome.clone());
                            ConfigurationRequestStatus::Completed {
                                version: state.version,
                                outcome,
                            }
                        }
                        Err(error) => ConfigurationRequestStatus::Failed(error),
                    };
                    if results
                        .send(Ok((request.id, status, state.snapshot())))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|_| ConfigurationError::Unavailable)?;
        Ok(Self {
            sender: Some(sender),
            receiver,
            records: Vec::new(),
            projection: None,
            unavailable: false,
        })
    }

    pub fn quiesce(&mut self) {
        self.sender = None;
    }

    fn collect(&mut self) -> Result<(), ConfigurationError> {
        loop {
            match self.receiver.try_recv() {
                Ok(Ok((id, status, snapshot))) => {
                    if let Some((_, existing)) = self
                        .records
                        .iter_mut()
                        .find(|(request, _)| request.id == id)
                    {
                        *existing = status;
                    }
                    self.projection = Some(snapshot);
                }
                Ok(Err(error)) => {
                    self.unavailable = true;
                    return Err(error);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.unavailable = true;
                    for (_, status) in &mut self.records {
                        if *status == ConfigurationRequestStatus::Accepted {
                            *status =
                                ConfigurationRequestStatus::Failed(ConfigurationError::Uncertain);
                        }
                    }
                    break;
                }
            }
        }
        Ok(())
    }
}

impl ConfigurationPort for ConfigurationService {
    fn submit(
        &mut self,
        request: ConfigurationRequest,
    ) -> Result<ConfigurationRequestId, ConfigurationError> {
        self.collect()?;
        validate_request(&request)?;
        if let Some((existing, _)) = self
            .records
            .iter()
            .find(|(existing, _)| existing.id == request.id)
        {
            return if existing == &request {
                Ok(request.id)
            } else {
                Err(ConfigurationError::InvalidRequest)
            };
        }
        if self.unavailable {
            return Err(ConfigurationError::Unavailable);
        }
        if self.records.len() == MAX_REQUESTS {
            return Err(ConfigurationError::Capacity);
        }
        let id = request.id;
        self.sender
            .as_ref()
            .ok_or(ConfigurationError::Unavailable)?
            .try_send(request.clone())
            .map_err(|error| match error {
                TrySendError::Full(_) => ConfigurationError::Busy,
                TrySendError::Disconnected(_) => ConfigurationError::Unavailable,
            })?;
        self.records
            .push((request, ConfigurationRequestStatus::Accepted));
        Ok(id)
    }

    fn status(
        &mut self,
        request: ConfigurationRequestId,
    ) -> Result<ConfigurationRequestStatus, ConfigurationError> {
        self.collect()?;
        Ok(self
            .records
            .iter()
            .find(|(existing, _)| existing.id == request)
            .map_or(ConfigurationRequestStatus::Unknown, |(_, status)| {
                status.clone()
            }))
    }

    fn snapshot(&mut self) -> Result<ConfigurationSnapshot, ConfigurationError> {
        self.collect()?;
        self.projection.clone().ok_or(if self.unavailable {
            ConfigurationError::Unavailable
        } else {
            ConfigurationError::Busy
        })
    }
}

fn validate_request(request: &ConfigurationRequest) -> Result<(), ConfigurationError> {
    if request.protocol != CONFIGURATION_PROTOCOL {
        return Err(ConfigurationError::UnsupportedProtocol);
    }
    if request.id.0 == 0 {
        return Err(ConfigurationError::InvalidRequest);
    }
    match &request.action {
        ConfigurationAction::Edit { edits, .. } => validate_edits(edits),
        ConfigurationAction::Save { keys, .. } | ConfigurationAction::Apply { keys, .. } => {
            if keys.len() > MAX_EDITS
                || keys.iter().any(|key| key.len() > 512)
                || keys.iter().collect::<BTreeSet<_>>().len() != keys.len()
            {
                return Err(ConfigurationError::Capacity);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_edits(edits: &[Edit]) -> Result<(), ConfigurationError> {
    if edits.len() > MAX_EDITS {
        return Err(ConfigurationError::Capacity);
    }
    let mut bytes = 0usize;
    for edit in edits {
        let size = match edit {
            Edit::Set { key, value } => {
                if key.len() > 512 {
                    return Err(ConfigurationError::Capacity);
                }
                let size = match value {
                    SettingValue::Text(text) => text.len(),
                    SettingValue::TextList(values) => values
                        .iter()
                        .map(|value| value.len().saturating_add(32))
                        .fold(0usize, usize::saturating_add),
                    SettingValue::TextMap(values) => values
                        .iter()
                        .map(|(key, value)| {
                            key.len().saturating_add(value.len()).saturating_add(64)
                        })
                        .fold(0usize, usize::saturating_add),
                    SettingValue::Fraction(value) if !value.is_finite() => {
                        return Err(ConfigurationError::InvalidRequest);
                    }
                    _ => 32,
                };
                key.len().saturating_add(size)
            }
            Edit::AddModel { name }
            | Edit::RemoveModel { name }
            | Edit::AddPool { name }
            | Edit::RemovePool { name } => name.len(),
        };
        bytes = bytes.saturating_add(size).saturating_add(64);
        if bytes > MAX_REQUEST_BYTES {
            return Err(ConfigurationError::Capacity);
        }
    }
    Ok(())
}

fn invalid(error: ConfigError) -> ConfigurationError {
    ConfigurationError::Invalid {
        code: error.code,
        key: error.key,
        span: error.span,
        errors: error.validation,
    }
}

struct State {
    file: ConfigurationFile,
    observed: Option<Observation>,
    discovery: Discovery,
    problem: Option<ConfigurationError>,
    saved_source: String,
    saved_has_file: bool,
    draft_source: String,
    saved: Settings,
    draft: Settings,
    effective: Settings,
    overrides: Vec<Edit>,
    version: Version,
    file_origin: SettingOrigin,
    saved_keys: BTreeSet<String>,
    effective_origins: BTreeMap<String, SettingOrigin>,
    pending_restart: Vec<String>,
    changes: VecDeque<ConfigurationChange>,
    dropped_changes: u64,
}

impl State {
    fn open(
        root: &Path,
        explicit: Option<&Path>,
        overrides: Vec<Edit>,
        policy: WritePolicy,
        instance: u64,
    ) -> Result<Self, ConfigurationError> {
        if overrides
            .iter()
            .any(|edit| !matches!(edit, Edit::Set { .. }))
        {
            return Err(ConfigurationError::InvalidRequest);
        }
        let defaults = encode_settings(&Settings::default()).map_err(invalid)?;
        let mut state = Self {
            file: ConfigurationFile::open(root, explicit, policy)?,
            observed: None,
            discovery: Discovery::Missing,
            problem: None,
            saved_source: defaults.clone(),
            saved_has_file: false,
            draft_source: defaults,
            saved: Settings::default(),
            draft: Settings::default(),
            effective: Settings::default(),
            overrides,
            version: Version {
                instance,
                revision: 0,
            },
            file_origin: if explicit.is_some() {
                SettingOrigin::ExplicitFile
            } else {
                SettingOrigin::Repository
            },
            saved_keys: BTreeSet::new(),
            effective_origins: BTreeMap::new(),
            pending_restart: Vec::new(),
            changes: VecDeque::new(),
            dropped_changes: 0,
        };
        state.reload()?;
        let effective = update_batch(&state.saved_source, &state.overrides).map_err(invalid)?;
        state.effective = parse_settings(&effective).map_err(invalid)?;
        state.effective_origins = state
            .snapshot()
            .values
            .into_iter()
            .map(|(key, value)| (key, value.effective_origin))
            .collect();
        Ok(state)
    }

    fn reload(&mut self) -> Result<(), ConfigurationError> {
        let observation = match self.file.read() {
            Ok(observation) => observation,
            Err(error) => {
                self.observed = None;
                self.discovery = if matches!(
                    error,
                    ConfigurationError::Invalid { .. } | ConfigurationError::Capacity
                ) {
                    Discovery::Invalid
                } else {
                    Discovery::Inaccessible
                };
                self.problem = Some(error);
                self.bump()?;
                return Ok(());
            }
        };
        let source = match &observation.source {
            Some(source) => source.clone(),
            None => encode_settings(&Settings::default()).map_err(invalid)?,
        };
        let settings = match parse_settings(&source) {
            Ok(settings) => settings,
            Err(error) => {
                self.observed = Some(observation);
                self.discovery = Discovery::Invalid;
                self.problem = Some(invalid(error));
                self.bump()?;
                return Ok(());
            }
        };
        self.discovery = if observation.source.is_some() {
            Discovery::Valid
        } else {
            Discovery::Missing
        };
        self.saved_has_file = observation.source.is_some();
        self.observed = Some(observation);
        self.problem = None;
        self.saved_source = source.clone();
        self.draft_source = source;
        self.saved = settings.clone();
        self.draft = settings;
        self.saved_keys.clear();
        self.bump()?;
        Ok(())
    }

    fn bump(&mut self) -> Result<(), ConfigurationError> {
        self.version.revision = self
            .version
            .revision
            .checked_add(1)
            .ok_or(ConfigurationError::Capacity)?;
        Ok(())
    }

    fn check(&self, expected: Version) -> Result<(), ConfigurationError> {
        if expected != self.version {
            return Err(ConfigurationError::Conflict);
        }
        if let Some(error) = &self.problem {
            return Err(error.clone());
        }
        Ok(())
    }

    fn execute(
        &mut self,
        request: &ConfigurationRequest,
    ) -> Result<ConfigurationOutcome, ConfigurationError> {
        validate_request(request)?;
        if self.version.revision == u64::MAX {
            return Err(ConfigurationError::Capacity);
        }
        match &request.action {
            ConfigurationAction::Reload => {
                self.reload()?;
                Ok(ConfigurationOutcome::Reloaded)
            }
            ConfigurationAction::Edit { expected, edits } => {
                self.check(*expected)?;
                let source = update_batch(&self.draft_source, edits).map_err(invalid)?;
                self.draft = parse_settings(&source).map_err(invalid)?;
                self.draft_source = source;
                self.bump()?;
                Ok(ConfigurationOutcome::DraftUpdated)
            }
            ConfigurationAction::Cancel { expected } => {
                if *expected != self.version {
                    return Err(ConfigurationError::Conflict);
                }
                self.draft = self.saved.clone();
                self.draft_source = self.saved_source.clone();
                self.bump()?;
                Ok(ConfigurationOutcome::Cancelled)
            }
            ConfigurationAction::Save { expected, keys } => {
                self.check(*expected)?;
                let edits = selected(&self.saved, &self.draft, keys)?;
                let source = update_batch(&self.saved_source, &edits).map_err(invalid)?;
                let settings = parse_settings(&source).map_err(invalid)?;
                let observed = self
                    .observed
                    .as_ref()
                    .ok_or(ConfigurationError::Unavailable)?;
                match self.file.save(observed, &source) {
                    Ok(observation) => self.observed = Some(observation),
                    Err(error) => {
                        if error == ConfigurationError::Uncertain {
                            self.problem = Some(error.clone());
                            self.bump()?;
                        }
                        return Err(error);
                    }
                }
                self.saved_source = source;
                self.saved_has_file = true;
                self.saved = settings;
                self.saved_keys.extend(keys.iter().cloned());
                self.discovery = Discovery::Valid;
                self.bump()?;
                Ok(ConfigurationOutcome::Saved)
            }
            ConfigurationAction::Apply { expected, keys } => {
                self.check(*expected)?;
                let entries = self.draft.entries();
                let mut edits = Vec::new();
                let mut pending = Vec::new();
                for key in keys {
                    if self
                        .overrides
                        .iter()
                        .any(|edit| matches!(edit, Edit::Set { key: locked, .. } if locked == key))
                    {
                        return Err(ConfigurationError::CliLocked);
                    }
                    let (descriptor, value) = entries
                        .iter()
                        .find(|(descriptor, _)| &descriptor.key == key)
                        .ok_or(ConfigurationError::InvalidRequest)?;
                    match descriptor.application {
                        ApplicationRule::Presentation => edits.push(Edit::Set {
                            key: key.clone(),
                            value: value.clone(),
                        }),
                        ApplicationRule::Restart => {
                            let effective = self
                                .effective
                                .entries()
                                .into_iter()
                                .find(|(entry, _)| entry.key == *key)
                                .map(|(_, value)| value);
                            if effective.as_ref() != Some(value) {
                                pending.push(key.clone());
                            }
                        }
                        _ => return Err(ConfigurationError::Unavailable),
                    }
                }
                let presentation = Settings {
                    tui: self.effective.tui.clone(),
                    ..Settings::default()
                };
                let source =
                    update_batch(&encode_settings(&presentation).map_err(invalid)?, &edits)
                        .map_err(invalid)?;
                let mut candidate = self.effective.clone();
                candidate.tui = parse_settings(&source).map_err(invalid)?.tui;
                candidate
                    .validate()
                    .map_err(|errors| ConfigurationError::Invalid {
                        code: ConfigErrorCode::Validation,
                        key: String::new(),
                        span: None,
                        errors,
                    })?;
                self.effective = candidate;
                self.effective_origins
                    .extend(edits.iter().filter_map(|edit| {
                        if let Edit::Set { key, .. } = edit {
                            Some((key.clone(), SettingOrigin::ActiveSnapshot))
                        } else {
                            None
                        }
                    }));
                for key in pending {
                    if !self.pending_restart.contains(&key) {
                        self.pending_restart.push(key);
                    }
                }
                self.bump()?;
                Ok(if self.pending_restart.is_empty() {
                    ConfigurationOutcome::Applied
                } else {
                    ConfigurationOutcome::RestartPending
                })
            }
        }
    }

    fn record(&mut self, request: ConfigurationRequestId, outcome: ConfigurationOutcome) {
        if self.changes.len() == MAX_CHANGES {
            self.changes.pop_front();
            self.dropped_changes = self.dropped_changes.saturating_add(1);
        }
        self.changes.push_back(ConfigurationChange {
            version: self.version,
            request,
            outcome,
        });
    }

    fn snapshot(&self) -> ConfigurationSnapshot {
        let saved: BTreeMap<_, _> = self
            .saved
            .entries()
            .into_iter()
            .map(|(descriptor, value)| (descriptor.key, value))
            .collect();
        let effective: BTreeMap<_, _> = self
            .effective
            .entries()
            .into_iter()
            .map(|(descriptor, value)| (descriptor.key, value))
            .collect();
        let mut entries: BTreeMap<_, _> = self
            .saved
            .entries()
            .into_iter()
            .map(|(descriptor, value)| (descriptor.key.clone(), (descriptor, value)))
            .collect();
        entries.extend(
            self.draft
                .entries()
                .into_iter()
                .map(|(descriptor, value)| (descriptor.key.clone(), (descriptor, value))),
        );
        entries.extend(
            self.effective
                .entries()
                .into_iter()
                .filter(|(descriptor, _)| !entries.contains_key(&descriptor.key))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(descriptor, value)| (descriptor.key.clone(), (descriptor, value))),
        );
        let draft_values: BTreeMap<_, _> = self
            .draft
            .entries()
            .into_iter()
            .map(|(descriptor, value)| (descriptor.key, value))
            .collect();
        let source_document = self
            .saved_has_file
            .then(|| self.saved_source.parse::<toml_edit::DocumentMut>().ok())
            .flatten();
        let values = entries
            .into_iter()
            .map(|(key, (descriptor, _))| {
                let saved_value = saved.get(&key).cloned().unwrap_or(SettingValue::Unset);
                let draft_value = draft_values
                    .get(&key)
                    .cloned()
                    .unwrap_or(SettingValue::Unset);
                let effective_value = effective.get(&key).cloned().unwrap_or(SettingValue::Unset);
                let locked = self.overrides.iter().any(
                    |edit| matches!(edit, Edit::Set { key: candidate, .. } if candidate == &key),
                );
                let saved_origin =
                    if self.saved_keys.iter().any(|selected| {
                        key == *selected || key.starts_with(&format!("{selected}."))
                    }) {
                        SettingOrigin::SavedFuture
                    } else if source_document
                        .as_ref()
                        .is_some_and(|document| explicitly_set(document, &key))
                    {
                        self.file_origin
                    } else {
                        SettingOrigin::BuiltIn
                    };
                let effective_origin = if locked {
                    SettingOrigin::CommandLine
                } else {
                    self.effective_origins
                        .get(&key)
                        .copied()
                        .unwrap_or(saved_origin)
                };
                let draft_origin = if draft_value != saved_value {
                    SettingOrigin::Draft
                } else {
                    saved_origin
                };
                let redact = |value| {
                    if descriptor.privacy == Privacy::Public || value == SettingValue::Unset {
                        safe_value(value)
                    } else {
                        SettingValue::Text("[redacted]".into())
                    }
                };
                (
                    key,
                    ConfigurationValue {
                        saved: redact(saved_value),
                        draft: redact(draft_value),
                        effective: redact(effective_value),
                        saved_origin,
                        effective_origin,
                        draft_origin,
                        cli_locked: locked,
                    },
                )
            })
            .collect();
        ConfigurationSnapshot {
            version: self.version,
            discovery: self.discovery.clone(),
            problem: self.problem.clone(),
            values,
            effective_ui: self.effective.tui.clone(),
            pending_restart: self.pending_restart.clone(),
            changes: self.changes.iter().cloned().collect(),
            dropped_changes: self.dropped_changes,
        }
    }
}

fn explicitly_set(document: &toml_edit::DocumentMut, key: &str) -> bool {
    let mut item = document.as_item();
    for part in key.split('.') {
        let Some(next) = item.get(part) else {
            return false;
        };
        item = next;
    }
    true
}

fn safe_value(value: SettingValue) -> SettingValue {
    fn text(value: String) -> String {
        value.chars().filter(|character| !character.is_control()
            && !matches!(*character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).collect()
    }
    match value {
        SettingValue::Text(value) => SettingValue::Text(text(value)),
        SettingValue::TextList(values) => {
            SettingValue::TextList(values.into_iter().map(text).collect())
        }
        SettingValue::TextMap(values) => SettingValue::TextMap(
            values
                .into_iter()
                .map(|(key, value)| (text(key), text(value)))
                .collect(),
        ),
        other => other,
    }
}

fn selected(
    saved: &Settings,
    draft: &Settings,
    keys: &[String],
) -> Result<Vec<Edit>, ConfigurationError> {
    let entries = draft.entries();
    let mut edits = Vec::new();
    for key in keys {
        let parts: Vec<_> = key.split('.').collect();
        if parts.len() == 2 && matches!(parts[0], "models" | "capacity_pools") {
            let model = parts[0] == "models";
            let existed = if model {
                saved.models.contains_key(parts[1])
            } else {
                saved.capacity_pools.contains_key(parts[1])
            };
            let exists = if model {
                draft.models.contains_key(parts[1])
            } else {
                draft.capacity_pools.contains_key(parts[1])
            };
            let name = parts[1].to_owned();
            if !exists {
                if !existed {
                    return Err(ConfigurationError::InvalidRequest);
                }
                edits.push(if model {
                    Edit::RemoveModel { name }
                } else {
                    Edit::RemovePool { name }
                });
            } else {
                if !existed {
                    edits.push(if model {
                        Edit::AddModel { name }
                    } else {
                        Edit::AddPool { name }
                    });
                }
                edits.extend(
                    entries
                        .iter()
                        .filter(|(descriptor, _)| descriptor.key.starts_with(&format!("{key}.")))
                        .map(|(descriptor, value)| Edit::Set {
                            key: descriptor.key.clone(),
                            value: value.clone(),
                        }),
                );
            }
        } else {
            let (_, value) = entries
                .iter()
                .find(|(descriptor, _)| descriptor.key == *key)
                .ok_or(ConfigurationError::InvalidRequest)?;
            edits.push(Edit::Set {
                key: key.clone(),
                value: value.clone(),
            });
        }
    }
    validate_edits(&edits)?;
    Ok(edits)
}

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;
