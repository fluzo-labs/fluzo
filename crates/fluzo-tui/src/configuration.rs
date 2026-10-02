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
    widgets::{Paragraph, Wrap},
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
    boolean_input: Option<(String, bool)>,
    choice_input: Option<(String, usize)>,
    editor: Editor,
    replace_input: bool,
    pending: Option<Pending>,
    next_id: u64,
    confirm: Option<KeyCode>,
    close_prompt: Option<usize>,
    close_after_save: bool,
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
            boolean_input: None,
            choice_input: None,
            editor: Editor::default(),
            replace_input: false,
            pending: None,
            next_id: 1,
            confirm: None,
            close_prompt: None,
            close_after_save: false,
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

    pub fn stage_discovered_models(
        &mut self,
        selections: &[crate::model_wizard::Selection],
    ) -> Result<(), &'static str> {
        let edits = self.edits.clone();
        let collections = self.collections.clone();
        let base = self.base;
        let filter = self.filter.clone();
        let selected = self.selected.clone();
        let open = self.open;
        let result = (|| {
            for selection in selections {
                self.stage_discovered_model(
                    &selection.alias,
                    &selection.endpoint,
                    &selection.model.id,
                )?;
                for edit in crate::model_wizard::selection_edits(selection)
                    .into_iter()
                    .skip(6)
                {
                    self.stage(edit)?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.edits = edits;
            self.collections = collections;
            self.base = base;
            self.filter = filter;
            self.selected = selected;
            self.open = open;
        }
        result
    }

    pub fn stage_discovered_model(
        &mut self,
        alias: &str,
        endpoint: &str,
        model: &str,
    ) -> Result<(), &'static str> {
        if self.pending.is_some() {
            return Err("Configuration request pending.");
        }
        if !fluzo_core::settings::valid_identifier(alias) {
            return Err("Invalid alias.");
        }
        for prefix in [format!("models.{alias}"), format!("capacity_pools.{alias}")] {
            if self
                .descriptors()
                .keys()
                .any(|key| key.starts_with(&format!("{prefix}.")))
                || self.collections.contains_key(&prefix)
            {
                return Err("Alias or pool already exists; choose a new alias.");
            }
        }
        let old_edits = self.edits.clone();
        let old_collections = self.collections.clone();
        let old_base = self.base;
        for edit in crate::model_wizard::model_edits(alias, endpoint, model) {
            if let Err(error) = self.stage(edit) {
                self.edits = old_edits;
                self.collections = old_collections;
                self.base = old_base;
                return Err(error);
            }
        }
        self.show(&format!("models.{alias}."));
        self.status = "Discovered model staged. Confirm suggested limits, then Validate and Save explicitly. No inference started.".into();
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

    pub(crate) fn preview_theme(&self) -> Option<&str> {
        if !self.open {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        let current = snapshot.values.get("tui.theme")?;
        if current.cli_locked {
            return None;
        }
        let value = self
            .edits
            .get("tui.theme")
            .or_else(|| self.staged.contains("tui.theme").then_some(&current.draft))?;
        match value {
            SettingValue::Text(theme) if matches!(theme.as_str(), "default" | "high-contrast") => {
                Some(theme)
            }
            _ => None,
        }
    }

    fn has_unsaved_changes(&self) -> bool {
        !self.collections.is_empty()
            || self.changed_keys(Intent::Save).iter().any(|key| {
                let Some(value) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.values.get(key))
                else {
                    return true;
                };
                let draft = self.edits.get(key).unwrap_or(&value.draft);
                if value.descriptor.privacy != Privacy::Public && self.edits.contains_key(key) {
                    return true;
                }
                draft != &value.saved
            })
    }

    pub fn show(&mut self, filter: &str) {
        self.open = true;
        self.filter = filter.into();
        self.confirm = None;
        self.close_prompt = None;
        self.close_after_save = false;
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
                        if self.close_after_save {
                            self.close_after_save = false;
                            if outcome == ConfigurationOutcome::Saved && !self.has_unsaved_changes()
                            {
                                self.close_prompt = None;
                                self.open = false;
                            }
                        }
                    }
                }
                Ok(ConfigurationRequestStatus::Unknown) => {
                    self.close_after_save = false;
                    self.status = "Unknown outcome. Do not replay; reconcile explicitly before further writes.".into();
                    self.base = Some(Version {
                        instance: 0,
                        revision: 0,
                    });
                    changed = true;
                }
                Ok(ConfigurationRequestStatus::Failed(error)) | Err(error) => {
                    self.close_after_save = false;
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
        if self.close_prompt.is_some() {
            return;
        }
        if self.boolean_input.is_some() || self.choice_input.is_some() {
            return;
        }
        if self.input.is_some() {
            if self.replace_input {
                let mut replacement = Editor::default();
                if !replacement.insert(text) {
                    self.status = "Input limit reached; paste rejected.".into();
                    return;
                }
                self.editor = replacement;
                self.replace_input = false;
            } else if !self.editor.insert(text) {
                self.status = "Input limit reached; paste rejected.".into();
            }
        } else if self.filter.len().saturating_add(safe_text(text).len()) <= 256 {
            self.filter.push_str(&safe_text(text));
            self.select_first();
        }
        self.confirm = None;
    }

    fn accept_input(&mut self) {
        if self.replace_input && matches!(self.input, Some(Input::Field(_))) {
            self.input = None;
            self.editor = Editor::default();
            self.replace_input = false;
            return;
        }
        let result = match self.input.as_ref() {
            Some(Input::Field(key)) => self
                .descriptors()
                .get(key)
                .ok_or("Setting no longer exists.")
                .and_then(|descriptor| {
                    if self.editor.text.is_empty() {
                        Ok(descriptor.default.clone())
                    } else {
                        parse_value(descriptor, &self.editor.text)
                    }
                })
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
                if let Err(error) = self.stage(Edit::Set { key, value }) {
                    self.status = error.into();
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
        if let Some(selection) = self.close_prompt {
            match key.code {
                KeyCode::Esc => {
                    self.close_prompt = None;
                    self.close_after_save = false;
                }
                KeyCode::Left | KeyCode::Up => {
                    self.close_prompt = Some(selection.saturating_sub(1))
                }
                KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                    self.close_prompt = Some((selection + 1).min(2))
                }
                KeyCode::Enter if key.modifiers.is_empty() && self.pending.is_none() => {
                    match selection {
                        0 => {
                            let checked = std::mem::take(&mut self.checked);
                            self.request(port, Intent::Save);
                            self.checked = checked;
                            self.close_after_save = self.pending.is_some();
                        }
                        1 => {
                            self.close_prompt = None;
                            self.open = false;
                            self.status = "Closed without saving; draft retained in memory. Theme preview ended.".into();
                        }
                        _ => self.close_prompt = None,
                    }
                }
                _ => {}
            }
            return;
        }
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
        if let Some((field, selection)) = self.choice_input.clone() {
            let descriptors = self.descriptors();
            let Some(descriptor) = descriptors.get(&field) else {
                self.choice_input = None;
                self.status = "Setting no longer exists.".into();
                return;
            };
            let SettingKind::Choice(options) = &descriptor.kind else {
                self.choice_input = None;
                return;
            };
            match key.code {
                KeyCode::Esc => {
                    self.choice_input = None;
                    self.status = "Selection cancelled; previous draft retained.".into();
                }
                KeyCode::Left => {
                    self.choice_input = Some((field, selection.saturating_sub(1)));
                }
                KeyCode::Right => {
                    self.choice_input =
                        Some((field, (selection + 1).min(options.len().saturating_sub(1))));
                }
                KeyCode::Enter if key.modifiers.is_empty() => {
                    if let Some(value) = options.get(selection) {
                        match self.stage(Edit::Set {
                            key: field,
                            value: SettingValue::Text(value.clone()),
                        }) {
                            Ok(()) => {
                                self.choice_input = None;
                                self.status =
                                    "Local draft changed; Save and Apply remain explicit.".into();
                            }
                            Err(error) => self.status = error.into(),
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some((field, value)) = self.boolean_input.clone() {
            match key.code {
                KeyCode::Esc => {
                    self.boolean_input = None;
                    self.status = "Selection cancelled; previous draft retained.".into();
                }
                KeyCode::Left | KeyCode::Right => {
                    self.boolean_input = Some((field, key.code == KeyCode::Left));
                }
                KeyCode::Enter if key.modifiers.is_empty() => {
                    match self.stage(Edit::Set {
                        key: field,
                        value: SettingValue::Boolean(value),
                    }) {
                        Ok(()) => {
                            self.boolean_input = None;
                            self.status =
                                "Local draft changed; Save and Apply remain explicit.".into();
                        }
                        Err(error) => self.status = error.into(),
                    }
                }
                _ => {}
            }
            return;
        }
        if self.input.is_some() {
            if self.replace_input && matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
                self.editor = Editor::default();
                self.replace_input = false;
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    self.input = None;
                    self.editor = Editor::default();
                }
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.accept_input()
                }
                KeyCode::Enter => {
                    self.replace_input = false;
                    self.editor.insert("\n");
                }
                KeyCode::Char('u') if control => {
                    self.editor = Editor::default();
                    self.replace_input = false;
                }
                KeyCode::Char(character)
                    if !control && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.paste(&character.to_string());
                }
                code => {
                    self.replace_input = false;
                    self.editor.key(code);
                }
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
            if self.pending.is_none() && self.has_unsaved_changes() {
                self.close_prompt = Some(2);
                self.status = "Unsaved changes. Save before leaving settings?".into();
                return;
            }
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
                    self.replace_input = false;
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
                        (SettingKind::Boolean, _) => {
                            self.status =
                                "Enter edits this choice; Left/Right then choose true/false."
                                    .into();
                            None
                        }
                        (SettingKind::Choice(_), _) => {
                            self.status =
                                "Enter edits this choice; Left/Right then choose an option.".into();
                            None
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
                    if descriptor.kind == SettingKind::Boolean {
                        let value = value.unwrap_or_else(|| descriptor.default.clone());
                        self.boolean_input =
                            Some((self.selected.clone(), value == SettingValue::Boolean(true)));
                        self.status = "Left/Right choose | Enter stages | Esc cancels".into();
                        return;
                    }
                    if let SettingKind::Choice(options) = &descriptor.kind {
                        let value = value.unwrap_or_else(|| descriptor.default.clone());
                        let selection = options
                            .iter()
                            .position(|option| value == SettingValue::Text(option.clone()))
                            .unwrap_or(0);
                        self.choice_input = Some((self.selected.clone(), selection));
                        self.status = "Left/Right choose | Enter stages | Esc cancels".into();
                        return;
                    }
                    if descriptor.privacy == Privacy::Public {
                        let value = value.unwrap_or_else(|| descriptor.default.clone());
                        self.editor.insert(&input_value(&value));
                    }
                    self.input = Some(Input::Field(self.selected.clone()));
                    self.replace_input = true;
                    self.status =
                        "Enter stages | Esc cancels | Empty restores default | F1 help".into();
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
        let mut shell =
            crate::shell::Shell::configuration_shell().expect("built-in presentation defaults");
        let mut settings = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_ui.clone())
            .unwrap_or_default();
        if let Some(theme) = self.preview_theme() {
            settings.theme = theme.into();
        }
        shell.preferences = crate::visual::Preferences::new(crate::visual::VisualOptions {
            settings,
            ascii,
            ..Default::default()
        })
        .expect("validated presentation snapshot");
        self.render_dialog(frame, &shell, color);
    }

    pub fn render_dialog(&self, frame: &mut Frame, shell: &crate::shell::Shell, color: bool) {
        let viewport = frame.area();
        let width = viewport.width.saturating_sub(2).min(100);
        let height = viewport.height.saturating_sub(2).min(34);
        let area = Rect::new(
            viewport.x + (viewport.width - width) / 2,
            viewport.y + (viewport.height - height) / 2,
            width,
            height,
        );
        let mut settings = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_ui.clone())
            .unwrap_or_default();
        if let Some(theme) = self.preview_theme() {
            settings.theme = theme.into();
        }
        let theme = Theme::new(&settings, color);
        if viewport.width < 60 || viewport.height < 16 {
            frame.render_widget(
                Paragraph::new(
                    "Configuration: resize required. Minimum 60x16. Esc returns; Ctrl+C exits.",
                )
                .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        shell.render_dialog_frame(frame, area, Theme::new(&settings, color));
        if let Some(selection) = self.close_prompt {
            shell.render_dialog_title(
                frame,
                Rect::new(area.x + 2, area.y + 1, area.width - 4, 1),
                "Save changes?",
                color,
            );
            let reason = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.save_unavailable.as_ref())
                .map(|error| {
                    format!(
                        "Save unavailable: {}. Draft will be retained.",
                        diagnostic(error)
                    )
                })
                .unwrap_or_else(|| {
                    "Save writes all unsaved changes. Wait for confirmed completion.".into()
                });
            let mut lines = vec![
                Line::from("You have unsaved configuration changes."),
                Line::from(reason),
                Line::from(""),
            ];
            for (index, action) in [
                "Save and leave",
                "Leave without saving (keep draft)",
                "Back to editing",
            ]
            .iter()
            .enumerate()
            {
                lines.push(Line::styled(
                    format!("{} {action}", if index == selection { ">" } else { " " }),
                    if index == selection {
                        theme.accent
                    } else {
                        theme.base
                    },
                ));
            }
            lines.push(Line::from(safe_text(&self.status)));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .style(theme.base),
                Rect::new(area.x + 2, area.y + 3, area.width - 4, area.height - 6),
            );
            frame.render_widget(
                Paragraph::new("Arrows select | Enter confirms | Esc back").style(theme.muted),
                Rect::new(area.x + 2, area.bottom() - 2, area.width - 4, 1),
            );
            crate::visual::terminal_colors(frame.buffer_mut(), color, shell.truecolor);
            return;
        }
        let category = if self.filter.starts_with("harness.") {
            "Limits"
        } else if self.filter.starts_with("capacity_pools.") {
            "Pools"
        } else if self.filter.starts_with("models.") {
            "Models"
        } else if self.filter.starts_with("storage.") {
            "Storage"
        } else if self.filter.starts_with("telemetry.") {
            "Telemetry"
        } else if self.filter.starts_with("tui.notifications.") {
            "Notifications"
        } else if self.filter.starts_with("tui.theme") {
            "Theme"
        } else if self.filter.starts_with("tui.dev_menu") {
            "Developer preferences"
        } else if self.filter.starts_with("tui.") {
            "Presentation"
        } else {
            "All settings"
        };
        shell.render_dialog_title(
            frame,
            Rect::new(area.x + 2, area.y + 1, area.width - 4, 1),
            &format!("FLUZO / Configuration / {category}"),
            color,
        );
        let inner = Rect::new(area.x + 2, area.y + 3, area.width - 4, area.height - 4);
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
        let detail_height = inner.height.saturating_sub(7).min(4);
        let list_height = inner.height.saturating_sub(6 + detail_height).max(1);
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
                let descriptor = descriptors.get(key);
                let label = if descriptor.is_some_and(|field| field.kind == SettingKind::Boolean) {
                    safe_text(key.rsplit('.').next().unwrap_or(key)).replace('_', " ")
                } else {
                    safe_text(key)
                };
                let mut text = safe_text(&text);
                if descriptor.is_some_and(|field| !mutable(field)) {
                    text.push_str(" [fixed]");
                }
                if self.checked.contains(key) {
                    text.push_str(" [save/apply]");
                }
                let mut row = setting_row(
                    &label,
                    &text,
                    self.boolean_input
                        .as_ref()
                        .filter(|(field, _)| field == key)
                        .map(|(_, value)| *value),
                    inner.width,
                    key == &self.selected,
                    theme,
                    color,
                );
                if descriptor.is_some_and(|field| !mutable(field)) || self.pending.is_some() {
                    mute_disabled_value(&mut row, color);
                }
                row
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
            Rect::new(
                inner.x,
                inner.y + 2 + list_height,
                inner.width,
                detail_height,
            ),
        );
        let footer = vec![
            Line::from(safe_text(&self.status)),
            Line::from(
                if self.boolean_input.is_some() || self.choice_input.is_some() {
                    "Left/Right choose | Enter stages | Esc cancels"
                } else if matches!(self.input, Some(Input::Field(_))) {
                    "Enter stages | Esc cancels | Empty restores default"
                } else {
                    "Enter edit | Left/Right step | Space select | F1 help"
                },
            ),
            Line::from("Ctrl+S save | Ctrl+A apply | Ctrl+V validate | Esc back"),
            Line::from("Ctrl+X cancel | Ctrl+R reload | PgUp/PgDn details"),
        ];
        frame.render_widget(
            Paragraph::new(footer).style(theme.muted),
            Rect::new(inner.x, inner.bottom().saturating_sub(4), inner.width, 4),
        );
        if self.help {
            shell.render_dialog_frame(frame, area, Theme::new(&settings, color));
            shell.render_dialog_title(
                frame,
                Rect::new(area.x + 2, area.y + 1, area.width - 4, 1),
                "Configuration help",
                color,
            );
            frame.render_widget(Paragraph::new("Configuration help\nType to search; Ctrl+U shows all basic/advanced fields.\nUp/Down/Tab select; Enter edits. Booleans and choices: Left/Right choose, Enter stages, Esc cancels. Numbers: Left/Right steps.\nSpace selects keys for Save/Apply; otherwise changed keys are used.\nCtrl+D stages the selected default; Delete stages collection removal.\nCtrl+N adds a model; Ctrl+P adds a pool. Update references together.\nCtrl+V validates; Ctrl+S saves future values; Ctrl+A applies presentation.\nCtrl+X twice cancels draft; Ctrl+R twice reloads and discards draft.\nEsc closes retaining draft. Pending writes may still complete.\nInline input: Enter stages; empty restores the default; Esc cancels. Ctrl+U clears; Shift+Enter inserts newline. Lists: one escaped item per line. Maps: escaped key=value per line. Escapes: \\n \\r \\t \\\\ \\=; \\e is an empty item.\nUnchanged redacted fields remain untouched; inputs replace whole values.\nOrdinary hosts cannot replace files; no confirmation bypasses this.\nAt request exhaustion, reconcile outcomes then explicitly reopen; local drafts are not persisted automatically.\nPgUp/PgDn scroll help. F1 or Esc closes. Ctrl+C exits.").wrap(Wrap { trim: false }).scroll((self.detail_scroll, 0)).style(theme.base), inner);
            frame.render_widget(
                Paragraph::new("PgUp/PgDn scroll | F1 / Esc back").style(theme.accent),
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
            );
            crate::visual::terminal_colors(frame.buffer_mut(), color, shell.truecolor);
            return;
        }
        if let Some((key, selection)) = &self.choice_input
            && let Some(descriptor) = descriptors.get(key)
            && let SettingKind::Choice(options) = &descriptor.kind
        {
            render_choice_input(
                frame,
                Rect::new(
                    inner.x,
                    inner.y + 2 + (selected - first) as u16,
                    inner.width,
                    1,
                ),
                options,
                *selection,
                theme,
                color,
            );
        }
        if let Some(Input::Field(key)) = &self.input {
            let private = descriptors
                .get(key)
                .is_some_and(|field| field.privacy != Privacy::Public);
            render_inline_input(
                frame,
                Rect::new(
                    inner.x,
                    inner.y + 2 + (selected - first) as u16,
                    inner.width,
                    1,
                ),
                &self.editor.text,
                self.editor.cursor,
                private,
                color,
            );
        }
        if let Some(input @ (Input::Model | Input::Pool)) = &self.input {
            let popup = area;
            shell.render_dialog_frame(frame, popup, Theme::new(&settings, color));
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
            shell.render_dialog_title(
                frame,
                Rect::new(popup.x + 2, popup.y + 1, popup.width - 4, 1),
                &title,
                color,
            );
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
                .style(theme.base),
                inner,
            );
            frame.render_widget(
                Paragraph::new("Enter stages | Ctrl+U clear | Esc keep previous")
                    .style(theme.accent),
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
            );
        }
        crate::visual::terminal_colors(frame.buffer_mut(), color, shell.truecolor);
    }
}

fn input_value(value: &SettingValue) -> String {
    let escape = |text: &str| {
        if text.is_empty() {
            "\\e".into()
        } else {
            text.replace('\\', "\\\\")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
                .replace('=', "\\=")
        }
    };
    match value {
        SettingValue::TextList(items) => items
            .iter()
            .map(|item| escape(item))
            .collect::<Vec<_>>()
            .join("\n"),
        SettingValue::TextMap(items) => items
            .iter()
            .map(|(key, value)| format!("{}={}", escape(key), escape(value)))
            .collect::<Vec<_>>()
            .join("\n"),
        SettingValue::Unset => String::new(),
        _ => display(value),
    }
}

pub(crate) fn render_choice_input(
    frame: &mut Frame,
    row: Rect,
    options: &[String],
    selection: usize,
    theme: Theme,
    color: bool,
) {
    let start = 4 + row.width.saturating_sub(4) / 2;
    let width = row.width.saturating_sub(start);
    let selected_style = if color {
        theme
            .base
            .bg(ratatui::style::Color::Cyan)
            .fg(ratatui::style::Color::Black)
    } else {
        theme.accent
    };
    let labels: Vec<_> = options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let option = safe_text(option);
            if color {
                format!(" {option} ")
            } else {
                format!("[{} {option}]", if index == selection { "x" } else { " " })
            }
        })
        .collect();
    use unicode_width::UnicodeWidthStr;
    let mut first = selection.min(labels.len().saturating_sub(1));
    let mut used = labels.get(first).map_or(0, |label| label.width());
    while first > 0 && used + labels[first - 1].width() < usize::from(width) {
        first -= 1;
        used += labels[first].width() + 1;
    }
    let mut spans = Vec::new();
    for (index, label) in labels.into_iter().enumerate().skip(first) {
        if !spans.is_empty() {
            spans.push(Span::styled(" ", theme.base));
        }
        spans.push(Span::styled(
            label,
            if index == selection {
                selected_style
            } else {
                theme.muted
            },
        ));
    }
    let area = Rect::new(row.x + start, row.y, width, 1);
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme.base), area);
}

pub(crate) fn render_inline_input(
    frame: &mut Frame,
    row: Rect,
    text: &str,
    cursor: usize,
    private: bool,
    color: bool,
) {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;

    let start = 4 + row.width.saturating_sub(4) / 2;
    let width = row.width.saturating_sub(start);
    if width == 0 {
        return;
    }
    let display = |text: &str| {
        if private {
            "*".repeat(text.graphemes(true).count())
        } else {
            safe_text(text)
        }
    };
    let before = display(&text[..cursor]);
    let after = display(&text[cursor..]);
    let mut visible = Vec::new();
    let mut cells = 0;
    for grapheme in before.graphemes(true).rev() {
        if cells + grapheme.width() >= usize::from(width) {
            break;
        }
        cells += grapheme.width();
        visible.push(grapheme);
    }
    let value = visible.into_iter().rev().collect::<String>() + &after;
    let style = if color {
        ratatui::style::Style::default()
            .bg(ratatui::style::Color::Cyan)
            .fg(ratatui::style::Color::Black)
    } else {
        ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::UNDERLINED)
    };
    let input_area = Rect::new(row.x + start, row.y, width, 1);
    frame.render_widget(ratatui::widgets::Clear, input_area);
    frame.render_widget(Paragraph::new(value).style(style), input_area);
    frame.set_cursor_position((row.x + start + cells as u16, row.y));
}

pub(crate) fn mute_disabled_value(row: &mut Line<'_>, color: bool) {
    for span in row.spans.iter_mut().skip(1) {
        if color {
            span.style = span.style.fg(ratatui::style::Color::Gray);
        } else {
            span.style = span.style.add_modifier(ratatui::style::Modifier::DIM);
        }
    }
}

pub(crate) fn setting_row(
    label: &str,
    value: &str,
    editing: Option<bool>,
    width: u16,
    focused: bool,
    theme: Theme,
    color: bool,
) -> Line<'static> {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;

    let style = if focused && color {
        theme
            .base
            .bg(theme.accent.fg.unwrap_or(ratatui::style::Color::Magenta))
            .fg(if theme.base.bg == Some(ratatui::style::Color::Black) {
                ratatui::style::Color::Black
            } else {
                ratatui::style::Color::Rgb(255, 250, 241)
            })
    } else if focused {
        theme.accent
    } else {
        theme.base
    };
    let key_width = usize::from(width.saturating_sub(4)) / 2;
    let mut key = String::new();
    let mut used = 0;
    for grapheme in label.graphemes(true) {
        let cells = grapheme.width();
        if used + cells > key_width {
            break;
        }
        key.push_str(grapheme);
        used += cells;
    }
    key.push_str(&" ".repeat(key_width - used));
    let mut line = Line::from(vec![Span::styled(
        format!("{}{}  ", if focused { "> " } else { "  " }, key),
        style,
    )])
    .style(style);
    if let Some(choice) = editing {
        let options = boolean_options("", choice, theme, color);
        if color {
            line.spans.extend(options.spans.into_iter().skip(1));
        } else {
            line.spans.push(Span::styled(
                boolean_control("", choice).trim_start().to_owned(),
                style,
            ));
        }
    } else {
        line.spans.push(Span::styled(value.to_owned(), style));
    }
    line.spans.push(Span::styled(
        " ".repeat(usize::from(width).saturating_sub(line.width())),
        style,
    ));
    line
}

pub(crate) fn boolean_options(key: &str, value: bool, theme: Theme, color: bool) -> Line<'static> {
    if !color {
        return Line::styled(boolean_control(key, value), theme.base);
    }
    let selected = theme
        .base
        .bg(ratatui::style::Color::Cyan)
        .fg(ratatui::style::Color::Black);
    Line::from(vec![
        Span::styled(
            format!(
                "{} ",
                safe_text(key.rsplit('.').next().unwrap_or(key)).replace('_', " ")
            ),
            theme.base,
        ),
        Span::styled(" true ", if value { selected } else { theme.muted }),
        Span::styled(" ", theme.base),
        Span::styled(" false ", if value { theme.muted } else { selected }),
    ])
}

pub(crate) fn boolean_control(key: &str, value: bool) -> String {
    format!(
        "{} [{} true] [{} false]",
        safe_text(key.rsplit('.').next().unwrap_or(key)).replace('_', " "),
        if value { "x" } else { " " },
        if value { " " } else { "x" },
    )
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
