use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::configuration::*;
use fluzo_core::settings::{Privacy, SettingDescriptor, SettingKind, SettingValue, Settings};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
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
    selected: usize,
    editing: bool,
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
            selected: 0,
            editing: false,
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
                            if self.explicit { "Configuration saved offline. Agent runtime is not implemented; no execution started." } else { "Configuration saved offline. F2 opens settings; Esc exits. Agent runtime is not implemented." }.into()
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
        Ok(edits)
    }

    pub fn paste(&mut self, text: &str) {
        if !self.editing {
            return;
        }
        let field = &mut self.fields[self.selected];
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
        if self.editing {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.editing = false,
                KeyCode::Backspace => {
                    let field = &mut self.fields[self.selected];
                    if let Some((offset, _)) = field.text.grapheme_indices(true).next_back() {
                        field.text.truncate(offset);
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.fields[self.selected].text.clear()
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
                self.editing = true;
                let descriptor = &self.fields[self.selected].descriptor;
                self.status = match &descriptor.kind {
                    SettingKind::Choice(options) => format!("Options: {}. Ctrl+U clears; Enter finishes.", options.join(" / ")),
                    SettingKind::Boolean => "Enter true or false. Ctrl+U clears; Enter finishes.".into(),
                    SettingKind::Integer => format!("Nonnegative integer, maximum {:?}. Ctrl+U clears; Enter finishes.", descriptor.integer_maximum),
                    _ => "Enter a value (credential fields accept reference names only). Ctrl+U clears; Enter finishes.".into(),
                };
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, color: bool, ascii: bool) {
        self.render_with_capabilities(frame, color, ascii, false);
    }

    pub fn render_with_capabilities(&self, frame: &mut Frame, color: bool, ascii: bool, rgb: bool) {
        let area = frame.area();
        if area.width < 60 || area.height < 16 {
            frame.render_widget(
                Paragraph::new("Setup needs at least 60x16. Resize or Esc to cancel.")
                    .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        let settings = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_ui.clone())
            .unwrap_or_default();
        let theme = crate::visual::Theme::new(&settings, color);
        let accent = theme.accent;
        frame.render_widget(Block::default().style(theme.base), area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(accent)
            .title(" Configuration setup ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
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
        let mut lines = vec![
            Line::from(format!("Target: {}", self.target)),
            Line::from("Connection tests: unavailable. No provider or credential access."),
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
            let rows = body.height.saturating_sub(4).max(1) as usize;
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
                let value = if field.descriptor.privacy != Privacy::Public && !field.text.is_empty()
                {
                    "[private value entered]".into()
                } else {
                    safe_text(&field.text)
                };
                let line = format!(
                    "{} {} = {} {}",
                    if selected { ">" } else { " " },
                    field.descriptor.key,
                    value,
                    field.descriptor.unit
                );
                lines.push(Line::styled(
                    line,
                    if selected { accent } else { Style::default() },
                ));
            }
            let field = &self.fields[self.selected];
            lines.push(Line::from(field.descriptor.description.clone()));
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
                Span::raw(" edit Tab next ^A more ^S save Esc exit PgDn scroll"),
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
            assert!(text.contains("> capacity_pools.local.max_in_flight = 3"));
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
                        terminal.backend().buffer()[(column + 2, row + 1)].symbol(),
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
