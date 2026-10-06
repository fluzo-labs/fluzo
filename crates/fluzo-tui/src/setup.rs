use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::configuration::*;
use fluzo_core::settings::{Privacy, SettingDescriptor, SettingKind, SettingValue, Settings};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;

const KEYS: &[&str] = &[
    "models.coder.base_url",
    "models.coder.model",
    "models.coder.auth",
    "models.coder.api_key_env",
    "models.coder.credential_ref",
    "models.coder.context_window",
    "models.coder.max_output_tokens",
    "models.coder.max_in_flight",
    "capacity_pools.local.max_in_flight",
    "capacity_pools.local.foreground_reserved_slots",
    "decision.enabled",
    "models.shadow.endpoint",
    "models.shadow.model",
    "models.shadow.auth",
    "models.shadow.api_key_env",
    "models.shadow.credential_ref",
    "models.shadow.context_window",
    "models.shadow.max_output_tokens",
    "models.shadow.max_in_flight",
    "storage.session_max_bytes",
    "storage.diagnostic_max_bytes",
    "storage.min_free_bytes",
    "telemetry.export_enabled",
    "telemetry.otlp_endpoint",
    "telemetry.capture_content",
];

struct Field {
    descriptor: SettingDescriptor,
    text: String,
}

pub struct Setup {
    fields: Vec<Field>,
    discovered: Vec<Edit>,
    selected: usize,
    editing: bool,
    boolean_choice: Option<bool>,
    enum_choice: Option<usize>,
    original_input: Option<String>,
    replace_input: bool,
    advanced: bool,
    welcome: bool,
    pub quit: bool,
    pub saved: bool,
    pub snapshot: Option<ConfigurationSnapshot>,
    pub status: String,
    pending: Option<ConfigurationRequestId>,
    next_id: u64,
    target: String,
    explicit: bool,
    scroll: u16,
}

pub fn safe_text(text: &str) -> String {
    text.chars().flat_map(|character| {
        if character.is_control() || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            character.escape_unicode().collect::<Vec<_>>()
        } else { vec![character] }
    }).collect()
}

pub fn diagnostic(error: &ConfigurationError) -> String {
    match error {
        ConfigurationError::Invalid {
            code,
            key,
            span,
            errors,
        } => {
            let detail = errors
                .first()
                .map(|error| format!("{} {:?}", safe_text(&error.key), error.code))
                .unwrap_or_default();
            format!("{code:?}: {} {span:?} {detail}", safe_text(key))
        }
        other => format!("{other:?}"),
    }
}

fn value_text(value: &SettingValue) -> String {
    match value {
        SettingValue::Unset => String::new(),
        SettingValue::Text(text) => text.clone(),
        SettingValue::Integer(value) => value.to_string(),
        SettingValue::Boolean(value) => value.to_string(),
        SettingValue::Fraction(value) => value.to_string(),
        SettingValue::TextList(values) => format!("{values:?}"),
        SettingValue::TextMap(values) => format!("{values:?}"),
    }
}

impl Setup {
    pub fn new(target: String, explicit: bool) -> Self {
        let defaults = Settings::default();
        let fields = KEYS
            .iter()
            .map(|key| {
                let template = key
                    .replace(".coder.", ".<id>.")
                    .replace(".shadow.", ".<id>.")
                    .replace(".local.", ".<id>.");
                let mut descriptor = defaults
                    .descriptors()
                    .into_iter()
                    .chain(Settings::collection_descriptors())
                    .find(|entry| entry.key == template)
                    .expect("setup keys belong to the shared registry");
                descriptor.key = (*key).into();
                let text = value_text(&descriptor.default);
                Field { descriptor, text }
            })
            .collect();
        Self {
            fields,
            discovered: Vec::new(),
            selected: 0,
            editing: false,
            boolean_choice: None,
            enum_choice: None,
            original_input: None,
            replace_input: false,
            advanced: false,
            welcome: true,
            quit: false,
            saved: false,
            snapshot: None,
            status: "Loading selected configuration...".into(),
            pending: None,
            next_id: 1,
            target: safe_text(&target),
            explicit,
            scroll: 0,
        }
    }

    pub fn stage_discovered_models(
        &mut self,
        selections: &[crate::model_wizard::Selection],
    ) -> Result<(), &'static str> {
        let previous = self.discovered.clone();
        let result = (|| {
            for selection in selections {
                self.stage_discovered_model(
                    &selection.alias,
                    &selection.endpoint,
                    &selection.model.id,
                )?;
                self.discovered.extend(
                    crate::model_wizard::selection_edits(selection)
                        .into_iter()
                        .skip(6),
                );
                if self.discovered.len() + self.fields.len() + 7 > MAX_EDITS {
                    return Err("Setup draft capacity reached.");
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.discovered = previous;
        }
        result
    }

    pub fn stage_discovered_model(
        &mut self,
        alias: &str,
        endpoint: &str,
        model: &str,
    ) -> Result<(), &'static str> {
        if self.pending.is_some()
            || self.saved
            || self.snapshot.as_ref().is_none_or(|snapshot| {
                snapshot.discovery != Discovery::Missing || snapshot.setup.is_some()
            })
        {
            return Err("Return to the editable setup form before adding a model.");
        }
        if !fluzo_core::settings::valid_identifier(alias)
            || matches!(alias, "coder" | "shadow" | "local")
        {
            return Err(
                "Choose a valid alias other than the reserved setup names coder, shadow and local.",
            );
        }
        if self
            .discovered
            .iter()
            .any(|edit| matches!(edit, Edit::AddModel { name } if name == alias))
        {
            return Err("Alias already staged.");
        }
        if self.discovered.len() + self.fields.len() + 7 > MAX_EDITS {
            return Err("Setup draft capacity reached.");
        }
        self.discovered
            .extend(crate::model_wizard::model_edits(alias, endpoint, model));
        self.status =
            "Discovered model staged. Ctrl+S reviews defaults and model limits before creation."
                .into();
        Ok(())
    }

    fn visible(&self) -> usize {
        if self.advanced { self.fields.len() } else { 10 }
    }

    pub fn poll(&mut self, port: &mut dyn ConfigurationPort) -> bool {
        let mut changed = false;
        match port.snapshot() {
            Ok(snapshot) => {
                if self.snapshot.as_ref() != Some(&snapshot) {
                    if self
                        .snapshot
                        .as_ref()
                        .is_some_and(|previous| previous.setup.is_some())
                        && snapshot.setup.is_none()
                    {
                        self.scroll = 0;
                    }
                    self.snapshot = Some(snapshot);
                    changed = true;
                    if self.pending.is_none() {
                        self.status = self.discovery_text();
                    }
                }
            }
            Err(ConfigurationError::Busy) => {}
            Err(error) => {
                let message = format!("Configuration unavailable: {}", diagnostic(&error));
                changed |= self.status != message;
                self.status = message;
            }
        }
        if let Some(id) = self.pending {
            match port.status(id) {
                Ok(ConfigurationRequestStatus::Accepted) => {}
                Ok(ConfigurationRequestStatus::Completed { outcome, .. }) => {
                    self.pending = None;
                    self.status = match outcome {
                        ConfigurationOutcome::SetupPrepared => "Review every value and target. Ctrl+S confirms creation; Esc returns without saving.".into(),
                        ConfigurationOutcome::Saved => {
                            self.saved = true;
                            if self.explicit { "Configuration saved offline. Agent runtime is not implemented; no execution started." } else { "Configuration saved offline. Ctrl+P opens settings; Esc exits. Agent runtime is not implemented." }.into()
                        }
                        _ => self.discovery_text(),
                    };
                    changed = true;
                }
                Ok(ConfigurationRequestStatus::Failed(error)) | Err(error) => {
                    self.pending = None;
                    self.status = format!(
                        "{}; original preserved unless outcome is Uncertain. Ctrl+R reloads explicitly.",
                        diagnostic(&error)
                    );
                    changed = true;
                }
                Ok(ConfigurationRequestStatus::Unknown) => {
                    self.pending = None;
                    self.status =
                        "Unknown request outcome; do not retry a write. Reload explicitly.".into();
                    changed = true;
                }
            }
        }
        changed
    }

    fn discovery_text(&self) -> String {
        let Some(snapshot) = &self.snapshot else {
            return "Loading selected configuration...".into();
        };
        if let Some(error) = &snapshot.problem {
            return format!(
                "{}; existing file preserved. Replacement unavailable here.",
                diagnostic(error)
            );
        }
        if snapshot.discovery == Discovery::Valid {
            if self.explicit { "Existing configuration: replacement unavailable without a controlled host. No file changed." }
            else { "Configuration loaded. Setup skipped. Agent runtime is not implemented; no execution started." }.into()
        } else {
            "Welcome to Fluzo. Enter configures this workspace offline; Esc cancels without saving."
                .into()
        }
    }

    fn send(&mut self, port: &mut dyn ConfigurationPort, action: ConfigurationAction) {
        if self.pending.is_some() {
            return;
        }
        let id = ConfigurationRequestId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        match port.submit(ConfigurationRequest {
            protocol: CONFIGURATION_PROTOCOL,
            id,
            action,
        }) {
            Ok(id) => {
                self.pending = Some(id);
                self.status = "Request accepted; waiting for completion. Closing does not undo a dispatched write.".into();
            }
            Err(error) => self.status = diagnostic(&error),
        }
    }

    fn edits(&self) -> Result<Vec<Edit>, String> {
        let mut values = BTreeMap::new();
        for field in &self.fields {
            let text = &field.text;
            let descriptor = &field.descriptor;
            let value = if text.is_empty() && descriptor.optional {
                SettingValue::Unset
            } else {
                match &descriptor.kind {
                    SettingKind::Boolean => SettingValue::Boolean(
                        text.parse()
                            .map_err(|_| format!("{} requires true or false", descriptor.key))?,
                    ),
                    SettingKind::Integer => SettingValue::Integer(text.parse().map_err(|_| {
                        format!("{} requires a nonnegative integer", descriptor.key)
                    })?),
                    SettingKind::Choice(options) if !options.contains(text) => {
                        return Err(format!(
                            "{} requires {}",
                            descriptor.key,
                            options.join(" / ")
                        ));
                    }
                    _ => SettingValue::Text(text.clone()),
                }
            };
            values.insert(descriptor.key.clone(), value);
        }
        let changed_model = |prefix: &str| {
            self.fields.iter().any(|field| {
                field.descriptor.key.starts_with(prefix)
                    && values[&field.descriptor.key] != field.descriptor.default
            })
        };
        let coder = changed_model("models.coder.");
        let shadow = values["decision.enabled"] == SettingValue::Boolean(true)
            || changed_model("models.shadow.");
        let pool = coder || shadow || changed_model("capacity_pools.local.");
        let mut edits = Vec::new();
        if pool {
            edits.push(Edit::AddPool {
                name: "local".into(),
            });
        }
        for (enabled, name, provider) in [
            (coder, "coder", "openai_compatible"),
            (shadow, "shadow", "laya_systemone"),
        ] {
            if enabled {
                edits.push(Edit::AddModel { name: name.into() });
                for (suffix, value) in [
                    ("provider", provider),
                    ("capacity_id", name),
                    ("pool", "local"),
                ] {
                    edits.push(Edit::Set {
                        key: format!("models.{name}.{suffix}"),
                        value: SettingValue::Text(value.into()),
                    });
                }
                edits.push(Edit::Set {
                    key: if name == "coder" {
                        "agent.model"
                    } else {
                        "decision.model"
                    }
                    .into(),
                    value: SettingValue::Text(name.into()),
                });
            }
        }
        for (key, value) in values {
            if (key.starts_with("models.coder.") && !coder)
                || (key.starts_with("models.shadow.") && !shadow)
                || (key.starts_with("capacity_pools.") && !pool)
            {
                continue;
            }
            edits.push(Edit::Set { key, value });
        }
        edits.extend(self.discovered.clone());
        if !coder
            && let Some(Edit::AddModel { name }) = self
                .discovered
                .iter()
                .find(|edit| matches!(edit, Edit::AddModel { .. }))
        {
            edits.push(Edit::Set {
                key: "agent.model".into(),
                value: SettingValue::Text(name.clone()),
            });
        }
        Ok(edits)
    }

    pub fn paste(&mut self, text: &str) {
        if !self.editing || self.boolean_choice.is_some() || self.enum_choice.is_some() {
            return;
        }
        let field = &mut self.fields[self.selected];
        if self.replace_input {
            field.text.clear();
            self.replace_input = false;
        }
        for grapheme in text.graphemes(true) {
            if grapheme.chars().any(|character| character.is_control()) {
                continue;
            }
            if field.text.len() + grapheme.len() > 2048 {
                break;
            }
            field.text.push_str(grapheme);
        }
    }

    pub fn key(&mut self, key: KeyEvent, port: &mut dyn ConfigurationPort) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'q'))
        {
            self.quit = true;
            return;
        }
        if let Some(selection) = self.enum_choice {
            let field = &mut self.fields[self.selected];
            if let SettingKind::Choice(options) = &field.descriptor.kind {
                match key.code {
                    KeyCode::Esc => {
                        self.enum_choice = None;
                        self.editing = false;
                    }
                    KeyCode::Left => self.enum_choice = Some(selection.saturating_sub(1)),
                    KeyCode::Right => {
                        self.enum_choice =
                            Some((selection + 1).min(options.len().saturating_sub(1)))
                    }
                    KeyCode::Enter if key.modifiers.is_empty() => {
                        if let Some(value) = options.get(selection) {
                            field.text = value.clone();
                            self.enum_choice = None;
                            self.editing = false;
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        if let Some(value) = self.boolean_choice {
            match key.code {
                KeyCode::Esc => {
                    self.boolean_choice = None;
                    self.editing = false;
                }
                KeyCode::Left | KeyCode::Right => {
                    self.boolean_choice = Some(key.code == KeyCode::Left);
                }
                KeyCode::Enter if key.modifiers.is_empty() => {
                    self.fields[self.selected].text = value.to_string();
                    self.boolean_choice = None;
                    self.editing = false;
                }
                _ => {}
            }
            return;
        }
        if self.editing {
            match key.code {
                KeyCode::Esc => {
                    if let Some(original) = self.original_input.take() {
                        self.fields[self.selected].text = original;
                    }
                    self.editing = false;
                }
                KeyCode::Enter => {
                    let field = &mut self.fields[self.selected];
                    if field.text.is_empty() {
                        field.text = value_text(&field.descriptor.default);
                    }
                    self.original_input = None;
                    self.editing = false;
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    if self.replace_input {
                        self.fields[self.selected].text.clear();
                        self.replace_input = false;
                        return;
                    }
                    let field = &mut self.fields[self.selected];
                    if let Some((offset, _)) = field.text.grapheme_indices(true).next_back() {
                        field.text.truncate(offset);
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.fields[self.selected].text.clear();
                    self.replace_input = false;
                }
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.paste(&character.to_string())
                }
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Esc {
            if self.pending.is_some() {
                self.quit = true;
                return;
            }
            if let Some(snapshot) = &self.snapshot
                && snapshot.setup.is_some()
            {
                self.send(
                    port,
                    ConfigurationAction::Cancel {
                        expected: snapshot.version,
                    },
                );
                return;
            }
            self.quit = true;
            return;
        }
        if self.pending.is_some() {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let expected = snapshot.version;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            self.send(port, ConfigurationAction::Reload);
            self.scroll = 0;
            return;
        }
        if matches!(key.code, KeyCode::PageDown | KeyCode::PageUp) {
            self.scroll = if key.code == KeyCode::PageDown {
                self.scroll.saturating_add(5).min(4096)
            } else {
                self.scroll.saturating_sub(5)
            };
            return;
        }
        if self.saved || snapshot.discovery != Discovery::Missing {
            return;
        }
        if snapshot.setup.is_some() {
            match key.code {
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.send(port, ConfigurationAction::ConfirmSetup { expected })
                }
                KeyCode::Down | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
                KeyCode::Up | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
                _ => {}
            }
            return;
        }
        if self.welcome {
            if key.code == KeyCode::Enter {
                self.welcome = false;
            }
            return;
        }
        match key.code {
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                match self.edits() {
                    Ok(edits) => {
                        self.scroll = 0;
                        self.send(port, ConfigurationAction::PrepareSetup { expected, edits });
                    }
                    Err(error) => self.status = error,
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.advanced = !self.advanced;
                self.selected = 0;
            }
            KeyCode::Down | KeyCode::Tab => self.selected = (self.selected + 1) % self.visible(),
            KeyCode::Up | KeyCode::BackTab => {
                self.selected = (self.selected + self.visible() - 1) % self.visible()
            }
            KeyCode::Enter => {
                let field = &self.fields[self.selected];
                let descriptor = &field.descriptor;
                if descriptor.constraint == fluzo_core::settings::Constraint::FixedFalse
                    || matches!(&descriptor.kind, SettingKind::Choice(options) if options.len() < 2)
                {
                    self.status = "Fixed setting; false is the only supported value.".into();
                    return;
                }
                self.enum_choice = if let SettingKind::Choice(options) = &descriptor.kind {
                    Some(
                        options
                            .iter()
                            .position(|option| option == &field.text)
                            .unwrap_or(0),
                    )
                } else {
                    None
                };
                self.original_input = Some(field.text.clone());
                self.replace_input = true;
                self.editing = true;
                self.boolean_choice =
                    (descriptor.kind == SettingKind::Boolean).then(|| field.text == "true");
                self.status = match &descriptor.kind {
                    SettingKind::Choice(_) => "Left/Right choose | Enter confirms | Esc cancels".into(),
                    SettingKind::Boolean => "Left/Right choose | Enter confirms | Esc cancels".into(),
                    SettingKind::Integer => format!("Nonnegative integer, maximum {:?}. Ctrl+U clears; Enter finishes.", descriptor.integer_maximum),
                    _ => "Enter a value (credential fields accept reference names only). Ctrl+U clears; Enter finishes.".into(),
                };
            }
            _ => {}
        }
    }

    pub fn dialog_key(&mut self, key: KeyEvent, port: &mut dyn ConfigurationPort) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.code == KeyCode::Enter && self.pending.is_none() {
            if self.saved {
                self.quit = self.explicit;
                return;
            }
            if self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.discovery == Discovery::Missing && snapshot.problem.is_none()
            }) {
                self.welcome = false;
                self.key(
                    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                    port,
                );
                return;
            }
        }
        if matches!(key.code, KeyCode::Esc | KeyCode::PageUp | KeyCode::PageDown)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c' | 'q' | 'r' | 's')))
        {
            if key.code == KeyCode::Char('s') {
                self.welcome = false;
            }
            self.key(key, port);
        }
    }

    pub fn render_dialog(&self, frame: &mut Frame, shell: &crate::shell::Shell, color: bool) {
        let area = frame.area();
        let settings = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_ui.clone())
            .unwrap_or_default();
        let theme = crate::visual::Theme::new(&settings, color);
        frame.render_widget(Block::default().style(theme.base), area);
        if area.width < 60 || area.height < 16 {
            frame.render_widget(
                Paragraph::new("Setup needs at least 60x16. Resize or Esc to cancel.")
                    .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        let width = area.width.saturating_sub(4).min(84);
        let height = area.height.saturating_sub(2).min(26);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        shell.render_dialog_frame(frame, popup, crate::visual::Theme::new(&settings, color));
        let preview = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.setup.as_ref());
        let problem = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.problem.as_ref());
        let title = if self.snapshot.is_none() {
            "Opening repository"
        } else if self.saved {
            "Ready to configure"
        } else if problem.is_some() {
            "Configuration needs attention"
        } else if preview.is_some() {
            "Create configuration?"
        } else if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.discovery == Discovery::Valid)
        {
            "Configuration already exists"
        } else {
            "Welcome to Fluzo"
        };
        shell.render_dialog_title(
            frame,
            Rect::new(popup.x + 2, popup.y + 1, width - 4, 1),
            title,
            color,
        );
        let mut lines = Vec::new();
        if self.snapshot.is_none() {
            lines.push(Line::from("Checking this repository..."));
        } else if self.saved {
            lines.push(Line::from("Your repository configuration is ready."));
            lines.push(Line::from(
                "You can now adjust preferences and connect models in Settings.",
            ));
            lines.push(Line::from("No task or inference has been started."));
        } else if problem.is_some() {
            lines.push(Line::from("We could not use the existing configuration."));
            lines.push(Line::from(
                "Your file has been preserved. Fix it outside Fluzo, then reload.",
            ));
        } else if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.discovery == Discovery::Valid)
        {
            lines.push(Line::from("This repository already has a configuration."));
            lines.push(Line::from(
                "Setup will not replace it. Open Fluzo normally to use Settings.",
            ));
        } else if let Some(preview) = preview {
            lines.push(Line::from(
                "Create a configuration file for this repository using recommended defaults.",
            ));
            lines.push(Line::from("An existing file will never be replaced."));
            lines.push(Line::default());
            let models: Vec<_> = preview
                .values
                .keys()
                .filter(|key| key.starts_with("models.") && key.ends_with(".provider"))
                .filter_map(|key| key.split('.').nth(1))
                .collect();
            lines.push(Line::from(format!("Models configured: {}", models.len())));
            for alias in models {
                lines.push(Line::from(format!(
                    "  {}: review context and output limits in Settings",
                    safe_text(alias)
                )));
            }
            lines.push(Line::from("Screen preferences: recommended defaults."));
            lines.push(Line::from(
                "Adding models does not start inference or authorize future work.",
            ));
        } else {
            lines.push(Line::from(
                "This repository does not have a configuration yet.",
            ));
            lines.push(Line::from("Would you like to create one?"));
            lines.push(Line::default());
            lines.push(Line::from(
                "Start with recommended defaults. You can change them later in Settings.",
            ));
            lines.push(Line::from(
                "Connect your models now, or leave that for later.",
            ));
            let count = self
                .discovered
                .iter()
                .filter(|edit| matches!(edit, Edit::AddModel { .. }))
                .count();
            if count > 0 {
                lines.push(Line::from(format!("Models ready to add: {count}")));
            }
            lines.push(Line::from("Nothing is written until you confirm creation."));
        }
        lines.push(Line::default());
        lines.push(Line::from("Configuration file:"));
        lines.push(Line::styled(self.target.clone(), theme.muted));
        let body = Rect::new(
            popup.x + 2,
            popup.y + 3,
            width - 4,
            height.saturating_sub(8),
        );
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .style(theme.base),
            body,
        );
        let message = if self.pending.is_some() {
            "Working... Please wait for confirmation."
        } else if problem.is_some()
            || self.status.contains("preserved unless")
            || self.status.contains("Unknown request")
        {
            self.status.as_str()
        } else {
            "PageUp/PageDown scroll | F4 advanced details"
        };
        frame.render_widget(
            Paragraph::new(message)
                .wrap(Wrap { trim: false })
                .style(theme.muted),
            Rect::new(popup.x + 2, popup.bottom() - 4, width - 4, 2),
        );
        let actions = if self.saved {
            if self.explicit {
                "Enter / Esc Close"
            } else {
                "Enter / Ctrl+P Open settings | Esc Close"
            }
        } else if preview.is_some() {
            "Enter Create file | Esc Back"
        } else if problem.is_some() {
            "Ctrl+R Reload | Esc Close"
        } else if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.discovery == Discovery::Valid)
        {
            "Esc Close"
        } else {
            "Enter Create | F3 Add models | Esc Not now"
        };
        frame.render_widget(
            Paragraph::new(actions).style(theme.accent),
            Rect::new(popup.x + 2, popup.bottom() - 2, width - 4, 1),
        );
        crate::visual::terminal_colors(frame.buffer_mut(), color, shell.truecolor);
    }

    pub fn render(&self, frame: &mut Frame, color: bool, ascii: bool) {
        self.render_with_capabilities(frame, color, ascii, false);
    }

    pub fn render_with_capabilities(&self, frame: &mut Frame, color: bool, ascii: bool, rgb: bool) {
        let viewport = frame.area();
        if viewport.width < 60 || viewport.height < 16 {
            frame.render_widget(
                Paragraph::new("Setup needs at least 60x16. Resize or Esc to cancel.")
                    .wrap(Wrap { trim: false }),
                viewport,
            );
            return;
        }
        let width = viewport.width.saturating_sub(2).min(100);
        let height = viewport.height.saturating_sub(2).min(34);
        let area = Rect::new(
            viewport.x + (viewport.width - width) / 2,
            viewport.y + (viewport.height - height) / 2,
            width,
            height,
        );
        let settings = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_ui.clone())
            .unwrap_or_default();
        let theme = crate::visual::Theme::new(&settings, color);
        let accent = theme.accent;
        let mut shell =
            crate::shell::Shell::configuration_shell().expect("built-in presentation defaults");
        shell.preferences = crate::visual::Preferences::new(crate::visual::VisualOptions {
            settings: settings.clone(),
            ascii,
            ..Default::default()
        })
        .expect("validated presentation snapshot");
        shell.truecolor = rgb;
        shell.render_dialog_frame(frame, area, crate::visual::Theme::new(&settings, color));
        shell.render_dialog_title(
            frame,
            Rect::new(area.x + 2, area.y + 1, area.width - 4, 1),
            "Configuration setup / Advanced",
            color,
        );
        let inner = Rect::new(area.x + 1, area.y + 2, area.width - 2, area.height - 3);
        let identity_height = if ascii { 1 } else { 3 };
        frame.render_widget(
            Paragraph::new(crate::identity::wordmark(
                std::time::Duration::ZERO,
                color,
                rgb,
                ascii,
            )),
            Rect::new(
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                identity_height,
            ),
        );
        let body = Rect::new(
            inner.x + 1,
            inner.y + identity_height + 1,
            inner.width.saturating_sub(2),
            inner.height.saturating_sub(identity_height + 6),
        );
        let mut input_row = None;
        let mut lines = vec![
            Line::from(format!("Target: {}", self.target)),
            Line::from("F3: explicit local/LAN model catalog. No inference or credential access."),
        ];
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.setup.is_some())
        {
            lines.push(Line::from(
                "Review: .fluzo can reveal infrastructure/preferences. Git is unchanged.",
            ));
            if let Some(snapshot) = &self.snapshot {
                for (key, value) in &snapshot.values {
                    if value.cli_locked {
                        lines.push(Line::from(format!("CLI locked: {key}; saved future value does not replace active override.")));
                    }
                }
            }
            if let Some(preview) = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.setup.as_ref())
            {
                for (key, value) in &preview.values {
                    lines.push(Line::from(format!(
                        "{key} = {}",
                        safe_text(&value_text(value))
                    )));
                }
            }
        } else if !self.welcome
            && !self.saved
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.discovery == Discovery::Missing)
        {
            if body.height < 5 {
                lines.clear();
            }
            let rows = usize::from(body.height)
                .saturating_sub(lines.len() + 2)
                .max(1);
            let first = self.selected.saturating_sub(rows - 1);
            for (index, field) in self
                .fields
                .iter()
                .take(self.visible())
                .enumerate()
                .skip(first)
                .take(rows)
            {
                let selected = index == self.selected;
                if selected && self.editing && self.boolean_choice.is_none() {
                    input_row = Some(lines.len() as u16);
                }
                let value = if field.descriptor.privacy != Privacy::Public && !field.text.is_empty()
                {
                    "[private value entered]".into()
                } else {
                    safe_text(&field.text)
                };
                let label = if field.descriptor.kind == SettingKind::Boolean {
                    field
                        .descriptor
                        .key
                        .rsplit('.')
                        .next()
                        .unwrap_or(&field.descriptor.key)
                        .replace('_', " ")
                } else {
                    field.descriptor.key.clone()
                };
                let mut row = crate::configuration::setting_row(
                    &label,
                    &value,
                    if selected { self.boolean_choice } else { None },
                    body.width,
                    selected,
                    theme,
                    color,
                );
                if self.pending.is_some()
                    || field.descriptor.constraint == fluzo_core::settings::Constraint::FixedFalse
                    || matches!(&field.descriptor.kind, SettingKind::Choice(options) if options.len() < 2)
                {
                    crate::configuration::mute_disabled_value(&mut row, color);
                }
                lines.push(row);
            }
            let field = &self.fields[self.selected];
            lines.push(Line::from(format!(
                "{}: {}",
                field.descriptor.key, field.descriptor.description
            )));
            lines.push(Line::from(format!(
                "Default: {} | {}",
                if field.descriptor.privacy == Privacy::Public {
                    value_text(&field.descriptor.default)
                } else {
                    "[private]".into()
                },
                if self.editing {
                    "Editing: Ctrl+U clears; Enter finishes"
                } else {
                    "Enter edits selected field"
                }
            )));
        } else {
            if let Some(snapshot) = &self.snapshot {
                for (key, value) in &snapshot.values {
                    if value.cli_locked {
                        lines.push(Line::from(format!(
                            "CLI override: {key} = {} (not saved)",
                            value_text(&value.effective)
                        )));
                    }
                }
            }
            lines.push(Line::from(
                "Create a project configuration offline. Endpoint/model may be left blank.",
            ));
            lines.push(Line::from(
                "A valid configuration does not mean execution is available or authorized.",
            ));
            lines.push(Line::from(
                "The isolated demo remains available through fluzo demo --interactive.",
            ));
        }
        let form = !self.welcome
            && !self.saved
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.discovery == Discovery::Missing && snapshot.setup.is_none()
            });
        let paragraph = Paragraph::new(lines).scroll((self.scroll, 0));
        frame.render_widget(
            if form {
                paragraph
            } else {
                paragraph.wrap(Wrap { trim: false })
            },
            body,
        );
        if let Some(row) = input_row.and_then(|row| row.checked_sub(self.scroll))
            && row < body.height
        {
            let field = &self.fields[self.selected];
            let area = Rect::new(body.x, body.y + row, body.width, 1);
            if let Some(selection) = self.enum_choice
                && let SettingKind::Choice(options) = &field.descriptor.kind
            {
                crate::configuration::render_choice_input(
                    frame, area, options, selection, theme, color,
                );
            } else {
                crate::configuration::render_inline_input(
                    frame,
                    area,
                    &field.text,
                    field.text.len(),
                    field.descriptor.privacy != Privacy::Public,
                    color,
                );
            }
        }
        frame.render_widget(
            Paragraph::new(self.status.as_str()).wrap(Wrap { trim: false }),
            Rect::new(
                inner.x + 1,
                inner.bottom().saturating_sub(4),
                inner.width.saturating_sub(2),
                3,
            ),
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Enter", accent),
                Span::raw(
                    if self.enum_choice.is_some() || self.boolean_choice.is_some() {
                        " confirms | Left/Right choose | Esc cancels"
                    } else if self.editing {
                        " confirms | Esc cancels | Empty restores default"
                    } else {
                        " edit | Tab next | F4 back | Esc exit | ^S save"
                    },
                ),
            ])),
            Rect::new(
                inner.x + 1,
                inner.bottom().saturating_sub(1),
                inner.width.saturating_sub(2),
                1,
            ),
        );
        crate::visual::terminal_colors(frame.buffer_mut(), color, rgb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    struct Port {
        snapshot: ConfigurationSnapshot,
        sent: Vec<ConfigurationRequest>,
    }
    impl ConfigurationPort for Port {
        fn submit(
            &mut self,
            request: ConfigurationRequest,
        ) -> Result<ConfigurationRequestId, ConfigurationError> {
            let id = request.id;
            self.sent.push(request);
            Ok(id)
        }
        fn snapshot(&mut self) -> Result<ConfigurationSnapshot, ConfigurationError> {
            Ok(self.snapshot.clone())
        }
        fn status(
            &mut self,
            _: ConfigurationRequestId,
        ) -> Result<ConfigurationRequestStatus, ConfigurationError> {
            Ok(ConfigurationRequestStatus::Accepted)
        }
    }
    fn port(discovery: Discovery) -> Port {
        Port {
            snapshot: ConfigurationSnapshot {
                save_unavailable: None,
                apply_unavailable: None,
                remaining_requests: MAX_REQUESTS,
                next_request_id: ConfigurationRequestId(1),
                version: Version {
                    instance: 1,
                    revision: 1,
                },
                discovery,
                problem: None,
                values: BTreeMap::new(),
                effective_ui: Default::default(),
                pending_restart: vec![],
                changes: vec![],
                dropped_changes: 0,
                setup: None,
                backup: None,
            },
            sent: vec![],
        }
    }
    #[test]
    fn review_regression_pool_edits_survive_without_models() {
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        assert!(
            !setup
                .edits()
                .unwrap()
                .iter()
                .any(|edit| matches!(edit, Edit::AddPool { .. }))
        );
        for (key, text) in [
            ("capacity_pools.local.max_in_flight", "3"),
            ("capacity_pools.local.foreground_reserved_slots", "2"),
        ] {
            setup
                .fields
                .iter_mut()
                .find(|field| field.descriptor.key == key)
                .unwrap()
                .text = text.into();
        }
        let edits = setup.edits().unwrap();
        assert_eq!(
            edits
                .iter()
                .filter(|edit| matches!(edit, Edit::AddPool { name } if name == "local"))
                .count(),
            1
        );
        assert!(
            !edits
                .iter()
                .any(|edit| matches!(edit, Edit::AddModel { .. }))
        );
        assert!(edits.contains(&Edit::Set {
            key: "capacity_pools.local.max_in_flight".into(),
            value: SettingValue::Integer(3),
        }));
        assert!(edits.contains(&Edit::Set {
            key: "capacity_pools.local.foreground_reserved_slots".into(),
            value: SettingValue::Integer(2),
        }));
    }

    #[test]
    fn review_regression_summary_represents_all_setting_types() {
        let values = BTreeMap::from([
            (
                "context.compaction_threshold".into(),
                SettingValue::Fraction(0.8),
            ),
            (
                "context.safety_reserve_fraction".into(),
                SettingValue::Fraction(0.05),
            ),
            (
                "fixture.list".into(),
                SettingValue::TextList(vec!["first".into(), "second".into()]),
            ),
            (
                "fixture.map".into(),
                SettingValue::TextMap(BTreeMap::from([("key".into(), "value".into())])),
            ),
            ("fixture.enabled".into(), SettingValue::Boolean(false)),
            ("fixture.count".into(), SettingValue::Integer(3)),
            (
                "fixture.text".into(),
                SettingValue::Text("[redacted]".into()),
            ),
        ]);
        assert_eq!(value_text(&SettingValue::Fraction(0.8)), "0.8");
        assert_eq!(value_text(&SettingValue::Fraction(0.05)), "0.05");
        assert_eq!(value_text(&SettingValue::TextList(vec![])), "[]");
        assert_eq!(value_text(&SettingValue::TextMap(BTreeMap::new())), "{}");
        assert_eq!(value_text(&SettingValue::Unset), "");
        let mut port = port(Discovery::Missing);
        port.snapshot.setup = Some(SetupPreview {
            replacing: false,
            values,
        });
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        setup.poll(&mut port);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| setup.render(frame, false, true))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "context.compaction_threshold = 0.8",
            "context.safety_reserve_fraction = 0.05",
            "first",
            "second",
            "key",
            "value",
            "fixture.enabled = false",
            "fixture.count = 3",
            "[redacted]",
        ] {
            assert!(text.contains(expected), "missing summary value: {expected}");
        }
    }

    #[test]
    fn review_regression_return_from_scrolled_summary_restores_form() {
        for reload in [false, true] {
            let mut port = port(Discovery::Missing);
            let mut setup = Setup::new("/fixture/.fluzo".into(), false);
            setup.welcome = false;
            setup.selected = 8;
            setup.fields[8].text = "3".into();
            port.snapshot.setup = Some(SetupPreview {
                replacing: false,
                values: BTreeMap::new(),
            });
            setup.poll(&mut port);
            for _ in 0..10 {
                setup.key(
                    KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                    &mut port,
                );
            }
            assert!(setup.scroll > 0);
            let key = if reload {
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
            } else {
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            };
            setup.key(key, &mut port);
            assert_eq!(port.sent.len(), 1);
            assert!(
                !port.sent.iter().any(|request| matches!(
                    request.action,
                    ConfigurationAction::ConfirmSetup { .. }
                ))
            );
            port.snapshot.setup = None;
            port.snapshot.version.revision += 1;
            setup.poll(&mut port);
            assert_eq!(setup.scroll, 0);
            assert_eq!(setup.selected, 8);
            assert_eq!(setup.fields[8].text, "3");
            assert!(!setup.quit);
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal
                .draw(|frame| setup.render(frame, false, true))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Target: /fixture/.fluzo"));
            assert!(text.contains("> capacity_pools.local.max_in_flight"));
            let buffer = terminal.backend().buffer();
            let row = buffer
                .content
                .chunks(120)
                .find(|cells| {
                    cells
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                        .contains("> capacity_pools.local.max_in_flight")
                })
                .unwrap();
            assert_eq!(row[62].symbol(), "3");
        }
    }

    #[test]
    fn setup_open_paste_cancel_and_valid_file_do_not_write() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        setup.poll(&mut port);
        setup.paste("injected\n");
        assert!(port.sent.is_empty());
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        setup.paste("e\u{301}\n\u{1b}[2J");
        assert!(!setup.fields[0].text.contains('\u{1b}'));
        setup.key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &mut port,
        );
        assert!(setup.fields[0].text.is_empty());
        setup.paste(&"x".repeat(3000));
        assert_eq!(setup.fields[0].text.len(), 2048);
        setup.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        setup.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        assert!(setup.quit && port.sent.is_empty());
        let mut setup = Setup::new("fixture".into(), false);
        port.snapshot.discovery = Discovery::Valid;
        setup.poll(&mut port);
        setup.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut port,
        );
        assert!(port.sent.is_empty());
        assert!(setup.status.contains("Setup skipped"));
    }
    #[test]
    fn welcome_dialog_hides_registry_keys_and_requires_separate_confirmation() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        setup.poll(&mut port);
        let shell = crate::shell::Shell::configuration_shell().unwrap();
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| setup.render_dialog(frame, &shell, false))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Welcome to Fluzo"));
            assert!(text.contains("Esc Not now"));
            assert!(!text.contains("models.coder") && !text.contains("tui."));
        }
        setup.paste("\n");
        assert!(port.sent.is_empty());
        setup.dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        assert_eq!(port.sent.len(), 1);
        assert!(matches!(
            port.sent[0].action,
            ConfigurationAction::PrepareSetup { .. }
        ));
        setup.dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        assert_eq!(port.sent.len(), 1);
        let mut cancelled = Setup::new("/fixture/.fluzo".into(), false);
        cancelled.poll(&mut port);
        cancelled.dialog_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        assert!(cancelled.quit);
        assert_eq!(port.sent.len(), 1);
    }

    #[test]
    fn setup_prepare_is_not_confirmation_and_defaults_use_registry() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("fixture".into(), false);
        setup.poll(&mut port);
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        setup.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut port,
        );
        assert!(matches!(
            port.sent[0].action,
            ConfigurationAction::PrepareSetup { .. }
        ));
        setup.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut port,
        );
        assert_eq!(port.sent.len(), 1);
        for field in &setup.fields {
            assert_eq!(field.text, value_text(&field.descriptor.default));
            assert!(!field.descriptor.key.starts_with("tui."));
        }
        assert!(
            setup
                .edits()
                .unwrap()
                .iter()
                .all(|edit| { !matches!(edit, Edit::Set { key, .. } if key.starts_with("tui.")) })
        );
    }
    #[test]
    fn setup_enumerated_choices_confirm_or_cancel_without_typing() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        setup.poll(&mut port);
        setup.welcome = false;
        setup.advanced = true;
        for index in 0..setup.fields.len() {
            let SettingKind::Choice(options) = setup.fields[index].descriptor.kind.clone() else {
                continue;
            };
            if options.len() < 2 {
                continue;
            }
            setup.selected = index;
            let original = setup.fields[index].text.clone();
            setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
            setup.paste("invalid");
            for _ in 0..options.len() {
                setup.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &mut port);
            }
            assert_eq!(setup.fields[index].text, original);
            for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| setup.render(frame, false, true))
                    .unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains(&format!("[x {}]", options.last().unwrap())));
            }
            setup.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
            assert_eq!(setup.fields[index].text, original);
            assert!(!setup.editing);
            setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
            for _ in 0..options.len() {
                setup.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &mut port);
            }
            setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
            assert_eq!(&setup.fields[index].text, options.last().unwrap());
            assert!(!setup.editing && setup.enum_choice.is_none());
        }
        assert!(port.sent.is_empty());
    }

    #[test]
    fn setup_inline_values_cancel_replace_and_restore_defaults() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        setup.poll(&mut port);
        setup.welcome = false;
        setup.selected = 8;
        setup.fields[8].text = "3".into();
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        setup.paste("7");
        assert_eq!(setup.fields[8].text, "7");
        setup.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
        assert_eq!(setup.fields[8].text, "3");
        assert!(!setup.editing && !setup.quit);
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        setup.paste("9");
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        assert_eq!(setup.fields[8].text, "9");
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        setup.key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &mut port,
        );
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
        assert_eq!(
            setup.fields[8].text,
            value_text(&setup.fields[8].descriptor.default)
        );
        assert!(port.sent.is_empty());
    }

    #[test]
    fn setup_booleans_use_choices_without_typing_or_implicit_save() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        setup.poll(&mut port);
        setup.welcome = false;
        setup.advanced = true;
        for index in 0..setup.fields.len() {
            if setup.fields[index].descriptor.kind != SettingKind::Boolean {
                continue;
            }
            setup.selected = index;
            let original = setup.fields[index].text.clone();
            let initial = original == "true";
            let arrow = if initial {
                KeyCode::Right
            } else {
                KeyCode::Left
            };
            setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
            setup.paste("true\nfalse");
            setup.key(
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                &mut port,
            );
            setup.key(KeyEvent::new(arrow, KeyModifiers::NONE), &mut port);
            setup.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut port);
            assert_eq!(setup.selected, index);
            assert_eq!(setup.fields[index].text, original);
            assert_eq!(setup.boolean_choice, Some(!initial));
            for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| setup.render(frame, false, true))
                    .unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(
                    text.contains(crate::configuration::boolean_control("", !initial).trim_start())
                );
            }
            setup.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut port);
            assert!(!setup.editing && !setup.quit);
            assert_eq!(setup.fields[index].text, original);
            setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
            setup.key(KeyEvent::new(arrow, KeyModifiers::NONE), &mut port);
            setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut port);
            assert_eq!(setup.fields[index].text, (!initial).to_string());
            assert!(setup.boolean_choice.is_none());
        }
        assert!(port.sent.is_empty());
    }

    #[test]
    fn advanced_setup_dialog_preserves_navigation_at_supported_sizes() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("/fixture/.fluzo".into(), false);
        setup.poll(&mut port);
        setup.welcome = false;
        setup.selected = 8;
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            for (color, ascii) in [(true, false), (false, false), (false, true)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| setup.render_with_capabilities(frame, color, ascii, color))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let left = (width - (width - 2).min(100)) / 2;
                let top = (height - (height - 2).min(34)) / 2;
                assert_eq!(buffer[(left, top)].symbol(), if ascii { "+" } else { "╭" });
                let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                assert!(text.contains("Configuration setup / Advanced"));
                assert!(text.contains("> capacity_pools.local.max"));
                assert!(text.contains("F4 back"));
                assert!(text.contains("Esc exit"));
            }
        }
        assert!(port.sent.is_empty());
    }

    #[test]
    fn setup_uses_shared_relief_identity_and_preserves_rgb_shadows() {
        for (color, ascii, rgb) in [
            (true, false, true),
            (true, false, false),
            (false, true, false),
        ] {
            let setup = Setup::new("/fixture/.fluzo".into(), false);
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|frame| setup.render_with_capabilities(frame, color, ascii, rgb))
                .unwrap();
            let mut expected = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 30, 3));
            use ratatui::widgets::Widget;
            Paragraph::new(crate::identity::wordmark(
                std::time::Duration::ZERO,
                color,
                rgb,
                ascii,
            ))
            .render(Rect::new(0, 0, 30, 3), &mut expected);
            for row in 0..if ascii { 1 } else { 3 } {
                for column in 0..30 {
                    assert_eq!(
                        terminal.backend().buffer()[(column + 3, row + 3)].symbol(),
                        expected[(column, row)].symbol()
                    );
                }
            }
            if color && rgb {
                assert!(terminal.backend().buffer().content.iter().any(|cell| {
                    matches!(
                        (cell.fg, cell.bg),
                        (
                            ratatui::style::Color::Rgb(..),
                            ratatui::style::Color::Rgb(..)
                        )
                    ) && cell.fg != cell.bg
                }));
            }
        }
    }

    #[test]
    fn setup_buffers_resize_redact_and_preserve_fields() {
        let mut port = port(Discovery::Missing);
        let mut setup = Setup::new("/fixture/\u{1b}[2J\u{202e}.fluzo".into(), false);
        setup.poll(&mut port);
        setup.welcome = false;
        setup.fields[0].text = "synthetic-private-endpoint".into();
        for (width, height) in [(40, 10), (60, 16), (80, 24), (120, 40), (160, 50)] {
            for color in [false, true] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| setup.render(frame, color, !color))
                    .unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(!text.contains("synthetic-private-endpoint"));
                assert!(!text.contains('\u{1b}') && !text.contains('\u{202e}'));
                assert!(text.contains(if width < 60 {
                    "Resize"
                } else {
                    "Configuration setup"
                }));
            }
        }
        assert_eq!(setup.fields[0].text, "synthetic-private-endpoint");
    }
}
