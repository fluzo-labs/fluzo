use crate::{setup::safe_text, shell::Shell, visual::Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::{
    configuration::Edit,
    model_discovery::{Model, ModelDiscoveryPort, PROTOCOL, Request, Status},
    settings::{SettingValue, valid_identifier},
};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Kind,
    Provider,
    Authorization,
    Header,
    Endpoint,
    Loading,
    Models,
    Alias,
    Review,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub alias: String,
    pub endpoint: String,
    pub model: Model,
    pub authorization_env: Option<String>,
}

pub struct ModelWizard {
    pub open: bool,
    step: Step,
    choice: usize,
    provider: usize,
    authorized: bool,
    header_env: String,
    endpoint: String,
    alias: String,
    query: String,
    models: Vec<Model>,
    checked: BTreeSet<String>,
    selected: usize,
    next_id: u64,
    pending: Option<u64>,
    status: String,
    pub completed: Option<Vec<Selection>>,
}
impl Default for ModelWizard {
    fn default() -> Self {
        Self {
            open: false,
            step: Step::Kind,
            choice: 0,
            provider: 0,
            authorized: false,
            header_env: String::new(),
            endpoint: String::new(),
            alias: String::new(),
            query: String::new(),
            models: vec![],
            checked: BTreeSet::new(),
            selected: 0,
            next_id: 1,
            pending: None,
            status: String::new(),
            completed: None,
        }
    }
}
impl ModelWizard {
    pub fn show(&mut self) {
        if self.pending.is_some() {
            return;
        }
        self.open = true;
        self.step = Step::Kind;
        self.choice = 0;
        self.status = "Choose where your models run.".into();
    }
    pub fn close(&mut self, port: &mut dyn ModelDiscoveryPort) {
        if let Some(id) = self.pending.take() {
            port.cancel(id);
        }
        self.open = false;
        self.completed = None;
    }
    pub fn poll(&mut self, port: &mut dyn ModelDiscoveryPort) -> bool {
        let Some(id) = self.pending else {
            return false;
        };
        match port.status(id) {
            Status::Pending => false,
            Status::Complete(Ok(models)) => {
                self.pending = None;
                self.models = models;
                self.checked.clear();
                self.selected = 0;
                self.query.clear();
                self.step = Step::Models;
                self.status = if self.models.is_empty() {
                    "No models returned. Esc edits the URL."
                } else {
                    "Space selects models. Enter continues. Unknown values are not inferred."
                }
                .into();
                true
            }
            status => {
                self.pending = None;
                self.step = Step::Endpoint;
                self.status =
                    format!("{status:?}. Correct URL/header and Tab to retry. No automatic retry.");
                true
            }
        }
    }
    fn visible(&self) -> Vec<&Model> {
        self.models
            .iter()
            .filter(|model| model.id.to_lowercase().contains(&self.query.to_lowercase()))
            .collect()
    }
    fn input(&mut self) -> Option<&mut String> {
        match self.step {
            Step::Header => Some(&mut self.header_env),
            Step::Endpoint => Some(&mut self.endpoint),
            Step::Alias => Some(&mut self.alias),
            Step::Models => Some(&mut self.query),
            _ => None,
        }
    }
    pub fn paste(&mut self, text: &str) {
        if let Some(target) = self.input() {
            for character in text.chars().filter(|character| !character.is_control()) {
                if target.len() + character.len_utf8() <= 2048 {
                    target.push(character);
                }
            }
        }
    }
    fn query(&mut self, port: &mut dyn ModelDiscoveryPort) {
        if self.endpoint.trim().is_empty() {
            self.status = "Enter a URL first.".into();
            return;
        }
        self.endpoint = self.endpoint.trim().trim_end_matches('/').to_owned();
        if !self.endpoint.ends_with("/v1") {
            self.endpoint.push_str("/v1");
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        match port.submit(Request {
            protocol: PROTOCOL,
            id,
            endpoint: self.endpoint.clone(),
            authorization_env: self.authorized.then(|| self.header_env.clone()),
        }) {
            Ok(()) => {
                self.pending = Some(id);
                self.step = Step::Loading;
                self.status = "Discovering models... Esc cancels. Maximum 5 seconds.".into();
            }
            Err(error) => {
                self.status =
                    format!("{error:?}. Check HTTP URL and Authorization environment reference.")
            }
        }
    }
    pub fn key(&mut self, key: KeyEvent, port: &mut dyn ModelDiscoveryPort) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.code == KeyCode::Esc {
            self.step = match self.step {
                Step::Kind => {
                    self.close(port);
                    return;
                }
                Step::Provider => Step::Kind,
                Step::Authorization => Step::Provider,
                Step::Header => Step::Authorization,
                Step::Endpoint => {
                    if self.authorized {
                        Step::Header
                    } else {
                        Step::Authorization
                    }
                }
                Step::Loading => {
                    if let Some(id) = self.pending.take() {
                        port.cancel(id);
                    }
                    Step::Endpoint
                }
                Step::Models => Step::Endpoint,
                Step::Alias => Step::Models,
                Step::Review => Step::Alias,
            };
            self.choice = 0;
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('u')
                && let Some(input) = self.input()
            {
                input.clear();
                self.selected = 0;
            }
            return;
        }
        match (self.step, key.code) {
            (Step::Kind | Step::Authorization, KeyCode::Up | KeyCode::Down | KeyCode::Tab) => {
                self.choice = (self.choice + 1) % 2
            }
            (Step::Provider, KeyCode::Down | KeyCode::Tab) => self.choice = (self.choice + 1) % 3,
            (Step::Provider, KeyCode::Up) => self.choice = (self.choice + 2) % 3,
            (Step::Kind, KeyCode::Enter) => {
                if self.choice == 1 {
                    self.status = "Frontier providers are planned for a separate story. Choose Local to continue.".into();
                } else {
                    self.step = Step::Provider;
                    self.choice = self.provider;
                    self.status = "Choose your local server.".into();
                }
            }
            (Step::Provider, KeyCode::Enter) => {
                self.provider = self.choice;
                self.choice = 0;
                self.step = Step::Authorization;
                self.status = "Does this provider require an Authorization header?".into();
            }
            (Step::Authorization, KeyCode::Enter) => {
                self.authorized = self.choice == 1;
                self.step = if self.authorized {
                    Step::Header
                } else {
                    Step::Endpoint
                };
                if !self.authorized {
                    self.header_env.clear();
                }
                self.status = "Tab or Enter leaves the URL field and fetches models. Esc goes back without querying.".into();
            }
            (Step::Header, KeyCode::Enter | KeyCode::Tab) => {
                if self.header_env.is_empty()
                    || !self.header_env.bytes().enumerate().all(|(index, byte)| {
                        byte == b'_'
                            || byte.is_ascii_alphabetic()
                            || (index > 0 && byte.is_ascii_digit())
                    })
                    || self.header_env.len() > 128
                {
                    self.status =
                        "Enter an environment variable name, not the secret value.".into();
                } else {
                    self.step = Step::Endpoint;
                    self.status = "Leaving URL with Tab/Enter sends this Authorization header to that endpoint. HTTP is unencrypted.".into();
                }
            }
            (Step::Endpoint, KeyCode::Enter | KeyCode::Tab) => self.query(port),
            (Step::Models, KeyCode::Down | KeyCode::Tab) => {
                self.selected = (self.selected + 1).min(self.visible().len().saturating_sub(1))
            }
            (Step::Models, KeyCode::Up) => self.selected = self.selected.saturating_sub(1),
            (Step::Models, KeyCode::Char(' ')) => {
                if let Some(model) = self.visible().get(self.selected) {
                    let id = model.id.clone();
                    if !self.checked.remove(&id) {
                        self.checked.insert(id);
                    }
                }
            }
            (Step::Models, KeyCode::Enter) => {
                if self.checked.is_empty()
                    && let Some(model) = self.visible().get(self.selected)
                {
                    self.checked.insert(model.id.clone());
                }
                if !self.checked.is_empty() {
                    self.step = Step::Alias;
                    self.status =
                        "Choose an alias prefix. Multiple models receive numbered aliases.".into();
                }
            }
            (Step::Alias, KeyCode::Enter | KeyCode::Tab) => {
                if valid_identifier(&self.alias) && self.alias.len() <= 48 {
                    self.step = Step::Review;
                    self.status =
                        "Confirm to stage all selected models atomically. Save remains separate."
                            .into();
                } else {
                    self.status = "Choose a valid alias, at most 48 bytes.".into();
                }
            }
            (Step::Review, KeyCode::Enter) => {
                self.completed = Some(self.selections());
            }
            (_, KeyCode::Backspace) => {
                if let Some(input) = self.input() {
                    input.pop();
                }
                self.selected = 0;
            }
            (_, KeyCode::Char(character))
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.paste(&character.to_string());
                self.selected = 0;
            }
            _ => {}
        }
    }
    fn selections(&self) -> Vec<Selection> {
        self.models
            .iter()
            .filter(|model| self.checked.contains(&model.id))
            .enumerate()
            .map(|(index, model)| Selection {
                alias: if self.checked.len() == 1 {
                    self.alias.clone()
                } else {
                    format!("{}_{}", self.alias, index + 1)
                },
                endpoint: self.endpoint.clone(),
                model: model.clone(),
                authorization_env: self.authorized.then(|| self.header_env.clone()),
            })
            .collect()
    }
    pub fn mouse(
        &mut self,
        event: crossterm::event::MouseEvent,
        area: Rect,
        port: &mut dyn ModelDiscoveryPort,
    ) {
        if self.step != Step::Endpoint
            || area.width < 60
            || area.height < 16
            || event.kind
                != crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
        {
            return;
        }
        let width = area.width.saturating_sub(4).min(88);
        let height = area.height.saturating_sub(2).min(24);
        let left = area.x + (area.width - width) / 2;
        let top = area.y + (area.height - height) / 2;
        let input = Rect::new(left + 2, top + 4, width - 4, 1);
        if !input.contains((event.column, event.row).into()) {
            self.query(port);
        }
    }
    pub fn reject(&mut self, message: &str) {
        self.status = message.into();
    }
    pub fn render(&self, frame: &mut Frame, shell: &Shell, color: bool) {
        let area = frame.area();
        if area.width < 60 || area.height < 16 {
            frame.render_widget(
                Paragraph::new("Resize to 60x16. Esc back/cancel. Ctrl+C exit."),
                area,
            );
            return;
        }
        let width = area.width.saturating_sub(4).min(88);
        let height = area.height.saturating_sub(2).min(24);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let theme = Theme::new(shell.preferences.effective(), color);
        shell.render_dialog_frame(
            frame,
            popup,
            Theme::new(shell.preferences.effective(), color),
        );
        let title = match self.step {
            Step::Kind => "1 / Model location",
            Step::Provider => "2 / Local provider",
            Step::Authorization | Step::Header => "3 / Authorization",
            Step::Endpoint | Step::Loading => "4 / Server URL",
            Step::Models => "5 / Select models",
            Step::Alias | Step::Review => "6 / Review models",
        };
        shell.render_dialog_title(
            frame,
            Rect::new(popup.x + 2, popup.y + 1, width - 4, 1),
            title,
            color,
        );
        let body = Rect::new(
            popup.x + 2,
            popup.y + 3,
            width - 4,
            height.saturating_sub(8),
        );
        let mut lines = Vec::new();
        match self.step {
            Step::Kind | Step::Provider | Step::Authorization => {
                let choices: &[&str] = match self.step {
                    Step::Kind => &["Local model", "Frontier model (coming later)"],
                    Step::Provider => &["LM Studio", "Ollama", "OpenAI Compatible"],
                    _ => &["No Authorization header", "Yes, add Authorization header"],
                };
                for (index, choice) in choices.iter().enumerate() {
                    lines.push(Line::styled(
                        format!("{} {choice}", if index == self.choice { ">" } else { " " }),
                        if index == self.choice {
                            theme.accent
                        } else {
                            theme.base
                        },
                    ));
                    lines.push(Line::default());
                }
            }
            Step::Header => {
                lines.push(Line::from(
                    "Authorization: value from an environment variable",
                ));
                lines.push(Line::from(format!("> {}", safe_text(&self.header_env))));
                lines.push(Line::from(
                    "Its value must contain the full header, such as Bearer <token>.",
                ));
                lines.push(Line::from(
                    "Only the reference is saved, never the token. HTTP is unencrypted.",
                ));
            }
            Step::Endpoint => {
                lines.push(Line::from(
                    "Server URL (HTTP, localhost, private IP or DNS name)",
                ));
                lines.push(Line::styled(
                    format!("> {}", safe_text(&self.endpoint)),
                    theme.accent,
                ));
                lines.push(Line::from(
                    "Tab/Enter fetches the catalog. No typing, paste or window-focus query.",
                ));
            }
            Step::Loading => lines.push(Line::from("Discovering models... Esc cancels.")),
            Step::Models => {
                lines.push(Line::from(format!(
                    "Search: {} | Selected: {}",
                    safe_text(&self.query),
                    self.checked.len()
                )));
                lines.push(Line::styled(
                    "Model | Max context | Output tokens | Slots",
                    theme.accent,
                ));
                let rows = usize::from(body.height.saturating_sub(2)).max(1);
                let first = self.selected.saturating_sub(rows - 1);
                for (index, model) in self.visible().iter().enumerate().skip(first).take(rows) {
                    let shown =
                        |value: Option<u64>| value.map_or("?".into(), |value| value.to_string());
                    let max_id = usize::from(body.width.saturating_sub(32));
                    let id: String = safe_text(&model.id).chars().take(max_id).collect();
                    lines.push(Line::styled(
                        format!(
                            "{} [{}] {} | {} | {} | {}",
                            if index == self.selected { ">" } else { " " },
                            if self.checked.contains(&model.id) {
                                "x"
                            } else {
                                " "
                            },
                            id,
                            shown(model.context_window),
                            shown(model.max_output_tokens),
                            shown(model.slots.map(u64::from))
                        ),
                        if index == self.selected {
                            theme.accent
                        } else {
                            theme.base
                        },
                    ));
                }
            }
            Step::Alias => {
                lines.push(Line::from(format!(
                    "{} models selected",
                    self.checked.len()
                )));
                lines.push(Line::styled(
                    format!("Alias prefix: {}", safe_text(&self.alias)),
                    theme.accent,
                ));
            }
            Step::Review => {
                lines.push(Line::from(format!(
                    "{} models, endpoint {}",
                    self.checked.len(),
                    safe_text(&self.endpoint)
                )));
                lines.push(Line::from(
                    "Reported limits are hints, not verified inference capabilities.",
                ));
                lines.push(Line::from(
                    "Unknown limits use registry suggestions; slots stay at 1.",
                ));
                for item in self
                    .selections()
                    .iter()
                    .take(usize::from(body.height.saturating_sub(3)))
                {
                    lines.push(Line::from(format!(
                        "{}: {}",
                        item.alias,
                        safe_text(&item.model.id)
                    )));
                }
            }
        }
        frame.render_widget(Paragraph::new(lines).style(theme.base), body);
        frame.render_widget(
            Paragraph::new(self.status.as_str())
                .wrap(Wrap { trim: false })
                .style(theme.muted),
            Rect::new(popup.x + 2, popup.bottom() - 4, width - 4, 2),
        );
        frame.render_widget(
            Paragraph::new("Enter/Tab next | Space select | Esc back/cancel").style(theme.accent),
            Rect::new(popup.x + 2, popup.bottom() - 2, width - 4, 1),
        );
    }
}
pub fn model_edits(alias: &str, endpoint: &str, model: &str) -> Vec<Edit> {
    let mut edits = vec![
        Edit::AddPool { name: alias.into() },
        Edit::AddModel { name: alias.into() },
    ];
    for (suffix, value) in [
        ("base_url", endpoint),
        ("model", model),
        ("capacity_id", alias),
        ("pool", alias),
    ] {
        edits.push(Edit::Set {
            key: format!("models.{alias}.{suffix}"),
            value: SettingValue::Text(value.into()),
        });
    }
    edits
}
pub fn selection_edits(selection: &Selection) -> Vec<Edit> {
    let mut edits = model_edits(&selection.alias, &selection.endpoint, &selection.model.id);
    if let Some(name) = &selection.authorization_env {
        edits.push(Edit::Set {
            key: format!("models.{}.auth", selection.alias),
            value: SettingValue::Text("env".into()),
        });
        edits.push(Edit::Set {
            key: format!("models.{}.api_key_env", selection.alias),
            value: SettingValue::Text(name.clone()),
        });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Port {
        requests: Vec<Request>,
        cancelled: Vec<u64>,
    }
    impl ModelDiscoveryPort for Port {
        fn submit(&mut self, request: Request) -> Result<(), fluzo_core::model_discovery::Error> {
            self.requests.push(request);
            Ok(())
        }
        fn status(&mut self, _: u64) -> Status {
            Status::Complete(Ok(vec!["fixture".into(), "second".into()]))
        }
        fn cancel(&mut self, id: u64) {
            self.cancelled.push(id);
        }
    }
    fn press(wizard: &mut ModelWizard, port: &mut Port, code: KeyCode) {
        wizard.key(KeyEvent::new(code, KeyModifiers::NONE), port);
    }
    #[test]
    fn url_blur_queries_once_and_cancel_fences_late_results() {
        let mut wizard = ModelWizard::default();
        let mut port = Port::default();
        wizard.show();
        for _ in 0..3 {
            press(&mut wizard, &mut port, KeyCode::Enter);
        }
        wizard.paste("http://127.0.0.1:1234");
        assert!(port.requests.is_empty());
        press(&mut wizard, &mut port, KeyCode::Tab);
        assert_eq!(port.requests.len(), 1);
        press(&mut wizard, &mut port, KeyCode::Tab);
        assert_eq!(port.requests.len(), 1);
        press(&mut wizard, &mut port, KeyCode::Esc);
        assert_eq!(port.cancelled, vec![1]);
        assert!(!wizard.poll(&mut port));
        assert!(wizard.completed.is_none());
        assert!(wizard.models.is_empty());
    }
    #[test]
    fn frontier_is_unavailable_and_multiselect_survives_filtering() {
        let mut wizard = ModelWizard::default();
        let mut port = Port::default();
        wizard.show();
        press(&mut wizard, &mut port, KeyCode::Down);
        press(&mut wizard, &mut port, KeyCode::Enter);
        assert!(wizard.step == Step::Kind);
        assert!(port.requests.is_empty());
        wizard.step = Step::Models;
        wizard.models = vec!["fixture".into(), "second".into()];
        press(&mut wizard, &mut port, KeyCode::Char(' '));
        wizard.paste("second");
        press(&mut wizard, &mut port, KeyCode::Char(' '));
        assert_eq!(wizard.checked.len(), 2);
        press(&mut wizard, &mut port, KeyCode::Enter);
        wizard.paste("alias");
        press(&mut wizard, &mut port, KeyCode::Enter);
        assert!(wizard.completed.is_none());
        press(&mut wizard, &mut port, KeyCode::Enter);
        let selections = wizard.completed.unwrap();
        assert_eq!(selections.len(), 2);
        assert_eq!(selections[0].alias, "alias_1");
        assert_eq!(selections[1].alias, "alias_2");
    }
    #[test]
    fn shared_dialogs_keep_background_and_controls_at_all_sizes() {
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            let shell = Shell::configuration_shell().unwrap();
            let mut wizard = ModelWizard::default();
            wizard.show();
            for step in [
                Step::Kind,
                Step::Provider,
                Step::Authorization,
                Step::Header,
                Step::Endpoint,
                Step::Loading,
                Step::Models,
                Step::Alias,
                Step::Review,
            ] {
                wizard.step = step;
                terminal
                    .draw(|frame| {
                        frame.render_widget(Paragraph::new("retained background"), frame.area());
                        wizard.render(frame, &shell, false);
                    })
                    .unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("retained background"));
                assert!(text.contains("Esc back/cancel"));
                assert!(text.contains('╭'));
            }
        }
    }
}
