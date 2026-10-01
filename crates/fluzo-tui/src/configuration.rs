use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::configuration::*;
use fluzo_core::settings::{
    Constraint, Privacy, SettingDescriptor, SettingKind, SettingValue, Settings,
};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::setup::{diagnostic, safe_text};
use crate::shell::Editor;
use crate::visual::Theme;

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Intent {
    Validate,
    Save,
    Apply,
}

struct Pending {
    id: ConfigurationRequestId,
    intent: Intent,
    editing: bool,
    keys: Vec<String>,
}

enum Input {
    Field(String),
    Model,
    Pool,
}

pub struct ConfigurationView {
    pub open: bool,
    pub snapshot: Option<ConfigurationSnapshot>,
    pub status: String,
    pub filter: String,
    pub target: String,
    help: bool,
    selected: String,
    edits: BTreeMap<String, SettingValue>,
    collections: BTreeMap<String, bool>,
    checked: BTreeSet<String>,
    staged: BTreeSet<String>,
    unapplied: BTreeSet<String>,
    input: Option<Input>,
    editor: Editor,
    pending: Option<Pending>,
    next_id: u64,
    confirm: Option<KeyCode>,
    detail_scroll: u16,
    base: Option<Version>,
}

impl Default for ConfigurationView {
    fn default() -> Self {
        Self {
            open: false,
            snapshot: None,
            status: "Loading configuration...".into(),
            filter: String::new(),
            target: String::new(),
            help: false,
            selected: String::new(),
            edits: BTreeMap::new(),
            collections: BTreeMap::new(),
            checked: BTreeSet::new(),
            staged: BTreeSet::new(),
            unapplied: BTreeSet::new(),
            input: None,
            editor: Editor::default(),
            pending: None,
            next_id: 1,
            confirm: None,
            detail_scroll: 0,
            base: None,
        }
    }
}

impl ConfigurationView {
    pub fn stage(&mut self, edit: Edit) -> Result<(), &'static str> {
        if self.pending.is_some() {
            return Err("Request pending.");
        }
        let adding = matches!(edit, Edit::AddModel { .. } | Edit::AddPool { .. });
        match edit {
            Edit::Set { key, value } => {
                let descriptors = self.descriptors();
                let descriptor = descriptors.get(&key).ok_or("Unknown setting.")?;
                if !mutable(descriptor) {
                    return Err("Fixed setting.");
                }
                if !self.put(key, value) {
                    return Err("Draft capacity reached.");
                }
            }
            Edit::AddModel { name } | Edit::RemoveModel { name } => {
                return self.stage_collection("models", &name, adding);
            }
            Edit::AddPool { name } | Edit::RemovePool { name } => {
                return self.stage_collection("capacity_pools", &name, adding);
            }
        }
        Ok(())
    }

    fn stage_collection(&mut self, group: &str, name: &str, add: bool) -> Result<(), &'static str> {
        if !fluzo_core::settings::valid_identifier(name) {
            return Err("Invalid collection name.");
        }
        if self.edits.len() + self.collections.len() >= MAX_EDITS {
            return Err("Draft capacity reached.");
        }
        self.begin_edit();
        self.collections.insert(format!("{group}.{name}"), add);
        Ok(())
    }

    pub fn save(&mut self, port: &mut dyn ConfigurationPort) {
        self.request(port, Intent::Save);
    }
    pub fn apply(&mut self, port: &mut dyn ConfigurationPort) {
        self.request(port, Intent::Apply);
    }
    pub fn validate(&mut self, port: &mut dyn ConfigurationPort) {
        self.request(port, Intent::Validate);
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn show(&mut self, filter: &str) {
        self.open = true;
        self.filter = filter.into();
        self.confirm = None;
        self.select_first();
    }

    fn descriptors(&self) -> BTreeMap<String, SettingDescriptor> {
        let mut descriptors: BTreeMap<_, _> = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.descriptor.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut additions = Settings::default();
        for (key, add) in &self.collections {
            if let Some(name) = key.strip_prefix("models.") {
                if *add {
                    additions.models.insert(name.into(), Default::default());
                }
            } else if let Some(name) = key.strip_prefix("capacity_pools.")
                && *add
            {
                additions
                    .capacity_pools
                    .insert(name.into(), Default::default());
            }
        }
        for descriptor in additions.descriptors() {
            descriptors
                .entry(descriptor.key.clone())
                .or_insert(descriptor);
        }
        descriptors
    }

    fn keys(&self) -> Vec<String> {
        self.descriptors()
            .into_keys()
            .filter(|key| {
                key.to_lowercase().contains(&self.filter.to_lowercase())
                    && !self
                        .collections
                        .iter()
                        .any(|(collection, add)| !add && key.starts_with(&format!("{collection}.")))
            })
            .collect()
    }

    fn select_first(&mut self) {
        let keys = self.keys();
        if !keys.contains(&self.selected) {
            self.selected = keys.first().cloned().unwrap_or_default();
        }
    }

    fn changed_keys(&self, intent: Intent) -> Vec<String> {
        let retained = if intent == Intent::Apply {
            &self.unapplied
        } else {
            &self.staged
        };
        let keys: BTreeSet<_> = retained
            .iter()
            .cloned()
            .chain(self.edits.keys().cloned())
            .chain(self.collections.keys().cloned())
            .collect();
        keys.iter()
            .filter(|key| {
                !keys.iter().any(|parent| {
                    parent.split('.').count() == 2
                        && (parent.starts_with("models.") || parent.starts_with("capacity_pools."))
                        && key.starts_with(&format!("{parent}."))
                })
            })
            .cloned()
            .collect()
    }

    fn batch(&self) -> Vec<Edit> {
        let mut edits: Vec<_> = self
            .collections
            .iter()
            .map(|(key, add)| {
                if let Some(name) = key.strip_prefix("models.") {
                    if *add {
                        Edit::AddModel { name: name.into() }
                    } else {
                        Edit::RemoveModel { name: name.into() }
                    }
                } else {
                    let name = key.trim_start_matches("capacity_pools.").into();
                    if *add {
                        Edit::AddPool { name }
                    } else {
                        Edit::RemovePool { name }
                    }
                }
            })
            .collect();
        edits.extend(self.edits.iter().map(|(key, value)| Edit::Set {
            key: key.clone(),
            value: value.clone(),
        }));
        edits
    }

    fn begin_edit(&mut self) {
        if self.base.is_none() {
            self.base = self.snapshot.as_ref().map(|snapshot| snapshot.version);
        }
    }

    fn put(&mut self, key: String, value: SettingValue) -> bool {
        fn size(value: &SettingValue) -> usize {
            match value {
                SettingValue::Text(value) => value.len(),
                SettingValue::TextList(values) => values.iter().map(String::len).sum(),
                SettingValue::TextMap(values) => values
                    .iter()
                    .map(|(key, value)| key.len().saturating_add(value.len()))
                    .sum(),
                _ => 16,
            }
        }
        let bytes = self.edits.iter().filter(|(entry, _)| *entry != &key).fold(
            key.len().saturating_add(size(&value)),
            |total, (key, value)| total.saturating_add(key.len()).saturating_add(size(value)),
        );
        if bytes > MAX_REQUEST_BYTES
            || (!self.edits.contains_key(&key)
                && self.edits.len() + self.collections.len() >= MAX_EDITS)
        {
            self.status =
                "Draft capacity reached; change rejected without partial submission.".into();
            return false;
        }
        self.begin_edit();
        self.edits.insert(key, value);
        true
    }

    fn send(
        &mut self,
        port: &mut dyn ConfigurationPort,
        action: ConfigurationAction,
        intent: Intent,
        editing: bool,
        keys: Vec<String>,
    ) {
        let id = ConfigurationRequestId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        match port.submit(ConfigurationRequest {
            protocol: CONFIGURATION_PROTOCOL,
            id,
            action,
        }) {
            Ok(id) => {
                self.pending = Some(Pending {
                    id,
                    intent,
                    editing,
                    keys,
                });
                self.status =
                    "Accepted; waiting for confirmed completion. Closing does not undo a write."
                        .into();
            }
            Err(error) => self.status = diagnostic(&error),
        }
    }

    fn request(&mut self, port: &mut dyn ConfigurationPort, intent: Intent) {
        if self.pending.is_some() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let reason = match intent {
            Intent::Save => snapshot.save_unavailable.as_ref(),
            Intent::Apply => snapshot.apply_unavailable.as_ref(),
            Intent::Validate => snapshot.problem.as_ref(),
        };
        if let Some(reason) = reason {
            self.status = if *reason == ConfigurationError::Capacity {
                "Request capacity exhausted; draft retained. Reconcile retained outcomes before explicitly reopening; no automatic retry.".into()
            } else {
                diagnostic(reason)
            };
            return;
        }
        let edits = self.batch();
        let required = if edits.is_empty() || intent == Intent::Validate {
            1
        } else {
            2
        };
        if snapshot.remaining_requests < required {
            self.status = "Request capacity exhausted; draft retained. Resolve pending outcomes, then explicitly reopen the application. No automatic retry.".into();
            return;
        }
        if edits.len() > MAX_EDITS {
            self.status = "Too many edits; atomic change not split. Draft retained.".into();
            return;
        }
        if self.base.is_some_and(|base| base != snapshot.version) {
            self.status = "Conflict: configuration changed; draft retained. Explicit Reload discards local changes.".into();
            return;
        }
        let keys = if self.checked.is_empty() {
            self.changed_keys(intent)
        } else {
            self.checked.iter().cloned().collect()
        };
        let version = snapshot.version;
        if !edits.is_empty() {
            self.send(
                port,
                ConfigurationAction::Edit {
                    expected: version,
                    edits,
                },
                intent,
                true,
                keys,
            );
        } else {
            self.finish_request(port, intent, version, keys);
        }
    }

    fn finish_request(
        &mut self,
        port: &mut dyn ConfigurationPort,
        intent: Intent,
        expected: Version,
        keys: Vec<String>,
    ) {
        let action = match intent {
            Intent::Validate => {
                self.status = "Draft validated; no file or effective settings changed.".into();
                return;
            }
            Intent::Save => ConfigurationAction::Save {
                expected,
                keys: keys.clone(),
            },
            Intent::Apply => ConfigurationAction::Apply {
                expected,
                keys: keys.clone(),
            },
        };
        self.send(port, action, intent, false, keys);
    }

    pub fn poll(&mut self, port: &mut dyn ConfigurationPort) -> bool {
        let mut changed = false;
        match port.snapshot() {
            Ok(snapshot) => {
                self.next_id = self.next_id.max(snapshot.next_request_id.0);
                if self.snapshot.as_ref() != Some(&snapshot) {
                    self.snapshot = Some(snapshot);
                    self.select_first();
                    changed = true;
                }
            }
            Err(ConfigurationError::Busy) => {}
            Err(error) => {
                let text = diagnostic(&error);
                changed |= self.status != text;
                self.status = text;
            }
        }
        if let Some(pending) = self.pending.take() {
            match port.status(pending.id) {
                Ok(ConfigurationRequestStatus::Accepted) => self.pending = Some(pending),
                Ok(ConfigurationRequestStatus::Completed { version, outcome }) => {
                    changed = true;
                    if pending.editing {
                        self.staged.extend(self.edits.keys().cloned());
                        self.staged.extend(self.collections.keys().cloned());
                        let descriptors = self.descriptors();
                        self.unapplied.extend(
                            self.edits
                                .keys()
                                .filter(|key| {
                                    descriptors.get(*key).is_some_and(|descriptor| {
                                        matches!(
                                            descriptor.application,
                                            fluzo_core::settings::ApplicationRule::Presentation
                                                | fluzo_core::settings::ApplicationRule::Restart
                                        )
                                    }) && !self
                                        .snapshot
                                        .as_ref()
                                        .and_then(|snapshot| snapshot.values.get(*key))
                                        .is_some_and(|value| value.cli_locked)
                                })
                                .cloned(),
                        );
                        for (collection, add) in &self.collections {
                            if !add {
                                self.unapplied
                                    .retain(|key| !key.starts_with(&format!("{collection}.")));
                            }
                        }
                        self.edits.clear();
                        self.collections.clear();
                        self.base = None;
                        if self.open {
                            self.finish_request(port, pending.intent, version, pending.keys);
                        } else {
                            self.status =
                                "Draft validated after closing; Save/Apply not dispatched.".into();
                        }
                    } else {
                        let covered = |key: &String| {
                            pending.keys.iter().any(|selected| {
                                key == selected || key.starts_with(&format!("{selected}."))
                            })
                        };
                        match outcome {
                            ConfigurationOutcome::Saved => self.staged.retain(|key| !covered(key)),
                            ConfigurationOutcome::Applied
                            | ConfigurationOutcome::RestartPending => {
                                self.unapplied.retain(|key| !covered(key))
                            }
                            _ => {}
                        }
                        if matches!(
                            outcome,
                            ConfigurationOutcome::Saved
                                | ConfigurationOutcome::Applied
                                | ConfigurationOutcome::RestartPending
                        ) {
                            self.checked.retain(|key| !covered(key));
                        }
                        if matches!(
                            outcome,
                            ConfigurationOutcome::Reloaded | ConfigurationOutcome::Cancelled
                        ) {
                            self.edits.clear();
                            self.collections.clear();
                            self.staged.clear();
                            self.unapplied.clear();
                            self.checked.clear();
                            self.base = None;
                            self.input = None;
                        }
                        self.status =
                            format!("Completed: {outcome:?}. Save and Apply are separate.");
                    }
                }
                Ok(ConfigurationRequestStatus::Unknown) => {
                    self.status = "Unknown outcome. Do not replay; reconcile explicitly before further writes.".into();
                    self.base = Some(Version {
                        instance: 0,
                        revision: 0,
                    });
                    changed = true;
                }
                Ok(ConfigurationRequestStatus::Failed(error)) | Err(error) => {
                    self.status = format!(
                        "{}; draft retained. No automatic retry.",
                        diagnostic(&error)
                    );
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn paste(&mut self, text: &str) {
        if self.input.is_some() {
            if !self.editor.insert(text) {
                self.status = "Input limit reached; paste rejected.".into();
            }
        } else if self.filter.len().saturating_add(safe_text(text).len()) <= 256 {
            self.filter.push_str(&safe_text(text));
            self.select_first();
        }
        self.confirm = None;
    }

    fn accept_input(&mut self) {
        let result = match self.input.as_ref() {
            Some(Input::Field(key)) => self
                .descriptors()
                .get(key)
                .ok_or("Setting no longer exists.")
                .and_then(|descriptor| parse_value(descriptor, &self.editor.text))
                .map(|value| (Some(key.clone()), value)),
            Some(Input::Model | Input::Pool) => {
                let name = &self.editor.text;
                if !fluzo_core::settings::valid_identifier(name) {
                    Err("Use 1..128 ASCII letters, digits, underscores or hyphens.")
                } else {
                    Ok((None, SettingValue::Text(name.clone())))
                }
            }
            None => return,
        };
        match result {
            Ok((Some(key), value)) => {
                if !self.put(key, value) {
                    return;
                }
                self.input = None;
            }
            Ok((None, SettingValue::Text(name))) => {
                let model = matches!(self.input, Some(Input::Model));
                let key = format!("{}.{name}", if model { "models" } else { "capacity_pools" });
                let exists_in_draft = self.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.values.iter().any(|(field, value)| {
                        field.starts_with(&format!("{key}.")) && value.draft != SettingValue::Unset
                    })
                });
                if self.collections.get(&key) == Some(&true) || exists_in_draft {
                    self.status = "Collection already exists; choose another name.".into();
                    return;
                }
                if let Err(error) = self.stage_collection(
                    if model { "models" } else { "capacity_pools" },
                    &name,
                    true,
                ) {
                    self.status = error.into();
                    return;
                }
                self.filter = key;
                self.input = None;
                self.select_first();
            }
            Ok(_) => {}
            Err(error) => {
                self.status = error.into();
                return;
            }
        }
        self.editor = Editor::default();
        self.status =
            "Local draft changed; Ctrl+V validates, Ctrl+S saves, Ctrl+A applies. No effect yet."
                .into();
    }

    pub fn key(&mut self, key: KeyEvent, port: &mut dyn ConfigurationPort) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::F(1) {
            self.help = !self.help;
            self.detail_scroll = 0;
            return;
        }
        if self.help {
            match key.code {
                KeyCode::Esc => self.help = false,
                KeyCode::PageDown | KeyCode::Down => {
                    self.detail_scroll = self.detail_scroll.saturating_add(1)
                }
                KeyCode::PageUp | KeyCode::Up => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1)
                }
                _ => {}
            }
            return;
        }
        if self.input.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.input = None;
                    self.editor = Editor::default();
                }
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.accept_input()
                }
                KeyCode::Enter => {
                    self.editor.insert("\n");
                }
                KeyCode::Char('u') if control => self.editor = Editor::default(),
                KeyCode::Char(character)
                    if !control && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.editor.insert(&character.to_string());
                }
                code => self.editor.key(code),
            }
            return;
        }
        if control && matches!(key.code, KeyCode::Char('r' | 'x')) {
            if self.pending.is_some() {
                self.status =
                    "Wait for pending completion; no replay or implicit cancellation.".into();
                return;
            }
            if self.confirm != Some(key.code) {
                self.confirm = Some(key.code);
                self.status = "Press the same key again to discard draft. Prior Save/Apply will not be undone.".into();
                return;
            }
            self.confirm = None;
            if let Some(snapshot) = &self.snapshot {
                let action = if key.code == KeyCode::Char('r') {
                    ConfigurationAction::Reload
                } else {
                    ConfigurationAction::Cancel {
                        expected: snapshot.version,
                    }
                };
                self.send(port, action, Intent::Validate, false, vec![]);
            }
            return;
        }
        if key.code == KeyCode::Esc {
            if let Some(pending) = &mut self.pending
                && pending.editing
            {
                pending.intent = Intent::Validate;
            }
            self.open = false;
            self.confirm = None;
            self.status = "View closed; draft retained. Cancel explicitly discards it; dispatched writes are not rolled back.".into();
            return;
        }
        self.confirm = None;
        if self.pending.is_some() {
            return;
        }
        if control {
            match key.code {
                KeyCode::Char('s') => self.request(port, Intent::Save),
                KeyCode::Char('a') => self.request(port, Intent::Apply),
                KeyCode::Char('v') => self.request(port, Intent::Validate),
                KeyCode::Char('n') | KeyCode::Char('p') => {
                    self.input = Some(if key.code == KeyCode::Char('n') {
                        Input::Model
                    } else {
                        Input::Pool
                    });
                    self.editor = Editor::default();
                }
                KeyCode::Char('d') => {
                    if let Some(descriptor) = self.descriptors().get(&self.selected)
                        && mutable(descriptor)
                    {
                        self.put(self.selected.clone(), descriptor.default.clone());
                    }
                }
                KeyCode::Char('u') => {
                    self.filter.clear();
                    self.select_first();
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                let keys = self.keys();
                let index = keys
                    .iter()
                    .position(|key| key == &self.selected)
                    .unwrap_or(0);
                let next = if matches!(key.code, KeyCode::Up | KeyCode::BackTab) {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(keys.len().saturating_sub(1))
                };
                self.selected = keys.get(next).cloned().unwrap_or_default();
                self.detail_scroll = 0;
            }
            KeyCode::Left | KeyCode::Right => {
                if let Some(descriptor) = self.descriptors().get(&self.selected) {
                    if !mutable(descriptor) {
                        self.status = "Fixed schema value.".into();
                        return;
                    }
                    let value = self
                        .edits
                        .get(&self.selected)
                        .cloned()
                        .or_else(|| {
                            self.snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.values.get(&self.selected))
                                .map(|value| value.draft.clone())
                        })
                        .unwrap_or_else(|| descriptor.default.clone());
                    let next = match (&descriptor.kind, value) {
                        (SettingKind::Boolean, SettingValue::Boolean(value)) => {
                            Some(SettingValue::Boolean(!value))
                        }
                        (SettingKind::Choice(options), SettingValue::Text(value)) => {
                            let index = options
                                .iter()
                                .position(|option| option == &value)
                                .unwrap_or(0);
                            let next = if key.code == KeyCode::Right {
                                (index + 1) % options.len()
                            } else {
                                (index + options.len() - 1) % options.len()
                            };
                            Some(SettingValue::Text(options[next].clone()))
                        }
                        (SettingKind::Integer, SettingValue::Integer(value)) => {
                            let minimum = u64::from(descriptor.constraint == Constraint::Positive);
                            let next = if key.code == KeyCode::Right {
                                value
                                    .saturating_add(1)
                                    .min(descriptor.integer_maximum.unwrap_or(i64::MAX as u64))
                            } else {
                                value.saturating_sub(1).max(minimum)
                            };
                            Some(SettingValue::Integer(next))
                        }
                        _ => None,
                    };
                    if let Some(value) = next {
                        self.put(self.selected.clone(), value);
                    }
                }
            }
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(3),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(3),
            KeyCode::Char(' ') => {
                if !self.checked.remove(&self.selected) {
                    self.checked.insert(self.selected.clone());
                }
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.select_first();
            }
            KeyCode::Delete => {
                let mut parts = self.selected.split('.');
                if let (Some(group @ ("models" | "capacity_pools")), Some(name)) =
                    (parts.next(), parts.next())
                {
                    let key = format!("{group}.{name}");
                    if self.edits.len() + self.collections.len() >= MAX_EDITS {
                        self.status = "Draft capacity reached; removal not staged.".into();
                        return;
                    }
                    self.begin_edit();
                    self.collections.insert(key.clone(), false);
                    self.edits
                        .retain(|field, _| !field.starts_with(&format!("{key}.")));
                    self.staged.insert(key);
                    self.select_first();
                    self.status =
                        "Collection removal staged only; update references before validation/save."
                            .into();
                }
            }
            KeyCode::Enter => {
                if let Some(descriptor) = self.descriptors().get(&self.selected) {
                    if !mutable(descriptor) {
                        self.status = "Fixed schema value; no supported alternative.".into();
                        return;
                    }
                    self.editor = Editor::default();
                    let value = self.edits.get(&self.selected).cloned().or_else(|| {
                        self.snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.values.get(&self.selected))
                            .map(|value| value.draft.clone())
                    });
                    if matches!(
                        descriptor.kind,
                        SettingKind::Boolean
                            | SettingKind::Integer
                            | SettingKind::Fraction
                            | SettingKind::Choice(_)
                    ) && let Some(value) = value
                    {
                        self.editor.insert(&display(&value));
                    }
                    self.input = Some(Input::Field(self.selected.clone()));
                    self.status = "Explicit replacement input. Esc keeps prior value; Ctrl+U clears input; Enter stages only.".into();
                }
            }
            KeyCode::Char(character) if self.filter.len() < 256 => {
                self.filter.push(character);
                self.select_first();
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, color: bool, ascii: bool) {
        let area = frame.area();
        let settings = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_ui.clone())
            .unwrap_or_default();
        let theme = Theme::new(&settings, color);
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(theme.base), area);
        if area.width < 60 || area.height < 16 {
            frame.render_widget(
                Paragraph::new(
                    "Configuration: resize required. Minimum 60x16. Esc returns; Ctrl+C exits.",
                )
                .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if ascii {
                ratatui::widgets::BorderType::Plain
            } else {
                ratatui::widgets::BorderType::Rounded
            })
            .title(" FLUZO / Configuration ")
            .style(theme.base);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let available = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                format!(
                    "Save: {} | Apply: {} | requests left: {}",
                    snapshot
                        .save_unavailable
                        .as_ref()
                        .map(diagnostic)
                        .unwrap_or_else(|| "available".into()),
                    snapshot
                        .apply_unavailable
                        .as_ref()
                        .map(diagnostic)
                        .unwrap_or_else(|| "presentation only".into()),
                    snapshot.remaining_requests
                )
            })
            .unwrap_or_else(|| "Loading...".into());
        let header = vec![
            Line::from(format!(
                "Search: {} | Target: {}",
                safe_text(&self.filter),
                safe_text(&self.target)
            )),
            Line::from(available),
        ];
        frame.render_widget(
            Paragraph::new(header).style(theme.muted),
            Rect::new(inner.x, inner.y, inner.width, 2),
        );
        let list_height = inner.height.saturating_sub(10).max(1);
        let keys = self.keys();
        let selected = keys
            .iter()
            .position(|key| key == &self.selected)
            .unwrap_or(0);
        let first = selected.saturating_sub(usize::from(list_height).saturating_sub(1));
        let descriptors = self.descriptors();
        let lines: Vec<_> = keys
            .iter()
            .skip(first)
            .take(usize::from(list_height))
            .map(|key| {
                let value = self.edits.get(key).cloned().or_else(|| {
                    self.snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.values.get(key))
                        .map(|value| value.draft.clone())
                });
                let private = descriptors
                    .get(key)
                    .is_some_and(|descriptor| descriptor.privacy != Privacy::Public);
                let text = if private {
                    "[redacted]".into()
                } else {
                    value
                        .as_ref()
                        .map(display)
                        .unwrap_or_else(|| "default".into())
                };
                Line::from(Span::styled(
                    format!(
                        "{} [{}] {} = {}",
                        if key == &self.selected { ">" } else { " " },
                        if self.checked.contains(key) { "x" } else { " " },
                        safe_text(key),
                        safe_text(&text)
                    ),
                    if key == &self.selected {
                        theme.accent
                    } else {
                        theme.base
                    },
                ))
            })
            .collect();
        frame.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y + 2, inner.width, list_height),
        );
        let mut details = Vec::new();
        if let Some(descriptor) = self.descriptors().get(&self.selected) {
            details.push(Line::from(format!(
                "{} | {:?} | {} | default {}",
                safe_text(&descriptor.key),
                descriptor.kind,
                descriptor.unit,
                safe_text(&display(&descriptor.default))
            )));
            details.push(Line::from(format!(
                "{} | {:?}",
                descriptor.description, descriptor.application
            )));
            if let Some(value) = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.values.get(&self.selected))
            {
                details.push(Line::from(format!(
                    "Saved {} ({:?}); draft {} ({:?})",
                    safe_text(&display(&value.saved)),
                    value.saved_origin,
                    safe_text(&display(&value.draft)),
                    value.draft_origin
                )));
                details.push(Line::from(format!(
                    "Effective {} ({:?}); CLI Apply lock: {}",
                    safe_text(&display(&value.effective)),
                    value.effective_origin,
                    value.cli_locked
                )));
            }
        }
        if let Some(snapshot) = &self.snapshot {
            details.push(Line::from(format!(
                "Restart pending: {}",
                snapshot.pending_restart.join(", ")
            )));
        }
        frame.render_widget(
            Paragraph::new(details)
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0)),
            Rect::new(inner.x, inner.y + 2 + list_height, inner.width, 4),
        );
        let footer = vec![
            Line::from(safe_text(&self.status)),
            Line::from("Enter edit | Left/Right change | Space select | F1 help"),
            Line::from("Ctrl+S save | Ctrl+A apply | Ctrl+V validate | Esc back"),
            Line::from("Ctrl+X cancel | Ctrl+R reload | PgUp/PgDn details"),
        ];
        frame.render_widget(
            Paragraph::new(footer).style(theme.muted),
            Rect::new(inner.x, inner.bottom().saturating_sub(4), inner.width, 4),
        );
        if self.help {
            frame.render_widget(Clear, inner);
            frame.render_widget(Paragraph::new("Configuration help\nType to search; Ctrl+U shows all basic/advanced fields.\nUp/Down/Tab select; Enter replaces; Left/Right toggles or steps.\nSpace selects keys for Save/Apply; otherwise changed keys are used.\nCtrl+D stages the selected default; Delete stages collection removal.\nCtrl+N adds a model; Ctrl+P adds a pool. Update references together.\nCtrl+V validates; Ctrl+S saves future values; Ctrl+A applies presentation.\nCtrl+X twice cancels draft; Ctrl+R twice reloads and discards draft.\nEsc closes retaining draft. Pending writes may still complete.\nInput: Ctrl+U clears; Esc keeps prior value; Shift+Enter inserts newline.\nUnchanged redacted fields remain untouched; inputs replace whole values.\nOrdinary hosts cannot replace files; no confirmation bypasses this.\nAt request exhaustion, reconcile outcomes then explicitly reopen; local drafts are not persisted automatically.\nPgUp/PgDn scroll help. F1 or Esc closes. Ctrl+C exits.").wrap(Wrap { trim: false }).scroll((self.detail_scroll, 0)).style(theme.base), inner);
            return;
        }
        if let Some(input) = &self.input {
            let popup = Rect::new(area.x + 2, area.y + 2, area.width - 4, area.height - 4);
            frame.render_widget(Clear, popup);
            let title = match input {
                Input::Field(key) => safe_text(key),
                Input::Model => "Add model name".into(),
                Input::Pool => "Add pool name".into(),
            };
            let private = match input {
                Input::Field(key) => self
                    .descriptors()
                    .get(key)
                    .is_some_and(|descriptor| descriptor.privacy != Privacy::Public),
                _ => false,
            };
            let text = if private {
                "*".repeat(self.editor.text.chars().count().min(80))
            } else {
                safe_text(&self.editor.text)
            };
            let hint = match input {
                Input::Field(key) => self.descriptors().get(key).map(|descriptor| match &descriptor.kind {
                    SettingKind::Choice(options) => format!("Choices: {}", options.join(" | ")),
                    SettingKind::Boolean => "Enter true or false".into(),
                    SettingKind::TextList => "Whole list replacement: one escaped item per line; Shift+Enter adds line. Empty input clears list.".into(),
                    SettingKind::TextMap => "Whole map replacement: escaped key=value per line. Empty input clears map.".into(),
                    _ => format!("Unit: {}. Optional empty input clears: {}", descriptor.unit, descriptor.optional),
                }).unwrap_or_default(),
                _ => "Validated collection name; configure related fields before validating.".into(),
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(hint),
                    Line::from(
                        "Escapes in lists/maps: \\n \\r \\t \\\\ \\=; \\e is an empty item.",
                    ),
                    Line::from(text),
                    Line::from(safe_text(&self.status)),
                    Line::from(
                        "Enter stages | Shift+Enter newline | Ctrl+U clear | Esc keep previous",
                    ),
                ])
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .style(theme.base),
                ),
                popup,
            );
        }
    }
}

fn mutable(descriptor: &SettingDescriptor) -> bool {
    !matches!(
        descriptor.constraint,
        Constraint::FixedFalse | Constraint::SchemaVersion
    ) && !matches!(&descriptor.kind, SettingKind::Choice(options) if options.len() < 2)
}

pub fn display(value: &SettingValue) -> String {
    match value {
        SettingValue::Boolean(value) => value.to_string(),
        SettingValue::Integer(value) => value.to_string(),
        SettingValue::Fraction(value) => value.to_string(),
        SettingValue::Text(value) => value.clone(),
        SettingValue::TextList(value) => format!("{value:?}"),
        SettingValue::TextMap(value) => format!("{value:?}"),
        SettingValue::Unset => "unset".into(),
    }
}

fn unescape(text: &str) -> Result<String, &'static str> {
    if text == "\\e" {
        return Ok(String::new());
    }
    let mut result = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        result.push(if character == '\\' {
            match characters.next() {
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some('\\') => '\\',
                Some('=') => '=',
                _ => return Err("Invalid escape; use \\n, \\r, \\t, \\\\, \\= or whole-item \\e."),
            }
        } else {
            character
        });
    }
    Ok(result)
}

fn parse_value(descriptor: &SettingDescriptor, text: &str) -> Result<SettingValue, &'static str> {
    if descriptor.optional && text.is_empty() {
        return Ok(SettingValue::Unset);
    }
    let value = match &descriptor.kind {
        SettingKind::Boolean => {
            SettingValue::Boolean(text.parse().map_err(|_| "Expected true or false.")?)
        }
        SettingKind::Integer => {
            let value = text
                .parse()
                .map_err(|_| "Expected a nonnegative integer.")?;
            if descriptor
                .integer_maximum
                .is_some_and(|maximum| value > maximum)
                || (descriptor.constraint == Constraint::Positive && value == 0)
            {
                return Err("Integer outside the shared setting bounds.");
            }
            SettingValue::Integer(value)
        }
        SettingKind::Fraction => {
            let value: f64 = text.parse().map_err(|_| "Expected a fraction.")?;
            if !value.is_finite() || value <= 0.0 || value >= 1.0 {
                return Err("Expected a finite fraction strictly between zero and one.");
            }
            SettingValue::Fraction(value)
        }
        SettingKind::Choice(options) => {
            if !options.iter().any(|option| option == text) {
                return Err("Choose a supported value.");
            }
            SettingValue::Text(text.into())
        }
        SettingKind::Text | SettingKind::CredentialReference => SettingValue::Text(text.into()),
        SettingKind::TextList => SettingValue::TextList(if text.is_empty() {
            vec![]
        } else {
            text.split('\n').map(unescape).collect::<Result<_, _>>()?
        }),
        SettingKind::TextMap => {
            let mut values = BTreeMap::new();
            if !text.is_empty() {
                for line in text.split('\n') {
                    let mut escaped = false;
                    let separator = line
                        .char_indices()
                        .find_map(|(index, character)| {
                            if escaped {
                                escaped = false;
                                None
                            } else if character == '\\' {
                                escaped = true;
                                None
                            } else if character == '=' {
                                Some(index)
                            } else {
                                None
                            }
                        })
                        .ok_or("Map entries require key=value.")?;
                    if values
                        .insert(
                            unescape(&line[..separator])?,
                            unescape(&line[separator + 1..])?,
                        )
                        .is_some()
                    {
                        return Err("Duplicate map key.");
                    }
                }
            }
            SettingValue::TextMap(values)
        }
    };
    Ok(value)
}
