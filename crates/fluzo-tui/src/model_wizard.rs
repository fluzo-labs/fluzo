use crate::{setup::safe_text, shell::Shell, visual::Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::{
    configuration::Edit,
    model_discovery::{
        AddressClass, Model, ModelDiscoveryPort, Notice, PROTOCOL, Probe, Request, Status,
    },
    settings::{SettingValue, valid_identifier},
};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Kind,
    Provider,
    Authorization,
    Header,
    Endpoint,
    Notice,
    Loading,
    Models,
    Alias,
    Review,
}

/// Which submitted job the view is still waiting on. A probe never connects, so a
/// pending probe and a pending discovery request must be cancellable distinctly.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Probe(u64),
    Discover(u64),
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
    pending: Option<Pending>,
    notice: Option<Notice>,
    accepted: Option<(String, AddressClass)>,
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
            notice: None,
            accepted: None,
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
        self.cancel_pending(port);
        self.open = false;
        self.completed = None;
    }
    fn cancel_pending(&mut self, port: &mut dyn ModelDiscoveryPort) {
        if let Some(pending) = self.pending.take() {
            port.cancel(match pending {
                Pending::Probe(id) | Pending::Discover(id) => id,
            });
        }
    }
    pub fn poll(&mut self, port: &mut dyn ModelDiscoveryPort) -> bool {
        let Some(pending) = self.pending else {
            return false;
        };
        let id = match pending {
            Pending::Probe(id) | Pending::Discover(id) => id,
        };
        match port.status(id) {
            Status::Pending => false,
            Status::Notice(notice) => {
                self.pending = None;
                if notice.class == AddressClass::Loopback || self.accepts(&notice) {
                    self.notice = None;
                    self.submit_discovery(port);
                } else {
                    self.notice = Some(notice);
                    self.step = Step::Notice;
                    self.status =
                        "Enter accepts cleartext HTTP for this endpoint only. Esc sends nothing."
                            .into();
                }
                true
            }
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
        self.invalidate_acceptance();
        if let Some(target) = self.input() {
            for character in text.chars().filter(|character| !character.is_control()) {
                if target.len() + character.len_utf8() <= 2048 {
                    target.push(character);
                }
            }
        }
    }
    fn accepts(&self, notice: &Notice) -> bool {
        self.accepted
            .as_ref()
            .is_some_and(|(endpoint, class)| *class == notice.class && *endpoint == self.endpoint)
    }
    fn invalidate_acceptance(&mut self) {
        if self.step == Step::Endpoint {
            self.accepted = None;
        }
    }
    fn accept_notice(&mut self, port: &mut dyn ModelDiscoveryPort) {
        let Some(notice) = self.notice.clone() else {
            self.step = Step::Endpoint;
            return;
        };
        match port.accept(&notice.endpoint, notice.class) {
            Ok(()) => {
                self.accepted = Some((notice.endpoint, notice.class));
                self.notice = None;
                self.submit_discovery(port);
            }
            Err(error) => {
                self.status = format!("{error:?}. Enter accepts again. Esc sends nothing.")
            }
        }
    }
    fn decline_notice(&mut self, port: &mut dyn ModelDiscoveryPort) {
        if let Some(notice) = self.notice.take() {
            // Records a redacted declined diagnostic; sends nothing either way.
            let _ = port.decline(&notice.endpoint);
        }
        self.step = Step::Endpoint;
        self.status = "Discovery declined. No request was sent.".into();
    }
    /// Resolve and classify before asking. A probe never opens a socket, so the
    /// warning is shown before any request can leave the process.
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
        match port.probe(Probe {
            protocol: PROTOCOL,
            id,
            endpoint: self.endpoint.clone(),
            authorization_env: self.authorized.then(|| self.header_env.clone()),
        }) {
            Ok(()) => {
                self.pending = Some(Pending::Probe(id));
                self.step = Step::Loading;
                self.status =
                    "Resolving the endpoint before connecting... Esc cancels. Maximum 5 seconds."
                        .into();
            }
            Err(error) => {
                self.status =
                    format!("{error:?}. Check HTTP URL and Authorization environment reference.")
            }
        }
    }
    fn submit_discovery(&mut self, port: &mut dyn ModelDiscoveryPort) {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        match port.submit(Request {
            protocol: PROTOCOL,
            id,
            endpoint: self.endpoint.clone(),
            authorization_env: self.authorized.then(|| self.header_env.clone()),
        }) {
            Ok(()) => {
                self.pending = Some(Pending::Discover(id));
                self.step = Step::Loading;
                self.status = "Discovering models... Esc cancels. Maximum 5 seconds.".into();
            }
            Err(error) => {
                self.step = Step::Endpoint;
                self.status =
                    format!("{error:?}. Correct URL/header and Tab to retry. No automatic retry.")
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
                    self.cancel_pending(port);
                    Step::Endpoint
                }
                Step::Notice => {
                    self.decline_notice(port);
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
            if key.code == KeyCode::Char('u') {
                self.invalidate_acceptance();
                if let Some(input) = self.input() {
                    input.clear();
                    self.selected = 0;
                }
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
            (Step::Notice, KeyCode::Enter | KeyCode::Tab) => self.accept_notice(port),
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
                self.invalidate_acceptance();
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
            Step::Notice => "4 / Cleartext warning",
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
                    "Tab/Enter resolves first; a non-local server shows a warning.",
                ));
            }
            Step::Notice => {
                let fit = |text: String| -> String {
                    text.chars().take(usize::from(body.width)).collect()
                };
                let notice = self.notice.clone().unwrap_or(Notice {
                    endpoint: self.endpoint.clone(),
                    resolved: vec![],
                    class: AddressClass::Unresolved,
                    credential_attached: self.authorized && !self.header_env.is_empty(),
                });
                let budget = usize::from(body.width.saturating_sub(10)).max(8);
                let resolved: Vec<String> = notice
                    .resolved
                    .iter()
                    .map(|address| safe_text(address))
                    .collect();
                let credential = if notice.credential_attached {
                    format!("attached from {}", safe_text(&self.header_env))
                } else {
                    "not attached".into()
                };
                lines.push(Line::styled(
                    "Cleartext HTTP: HTTPS is unsupported for discovery",
                    theme.accent,
                ));
                lines.push(Line::from(fit(format!(
                    "Endpoint: {}",
                    safe_text(&notice.endpoint)
                ))));
                lines.push(Line::from(fit(format!(
                    "Resolves: {}",
                    resolved_text(&resolved, budget)
                ))));
                lines.push(Line::from(fit(format!(
                    "Class: {} | Credential: {}",
                    class_text(notice.class),
                    credential
                ))));
                lines.push(Line::from(
                    "Accepting authorizes this endpoint only, for this run.",
                ));
            }
            Step::Loading => lines.push(Line::from(
                if matches!(self.pending, Some(Pending::Probe(_))) {
                    "Resolving the endpoint. Nothing sent yet. Esc cancels."
                } else {
                    "Discovering models... Esc cancels."
                },
            )),
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
fn class_text(class: AddressClass) -> &'static str {
    match class {
        AddressClass::Loopback => "loopback",
        AddressClass::Private => "private network",
        AddressClass::Public => "public internet",
        AddressClass::Unresolved => "unresolved",
    }
}

/// Fit the resolved address list to the available columns, keeping as many
/// addresses as fit and counting the rest instead of dropping them silently.
fn resolved_text(resolved: &[String], budget: usize) -> String {
    if resolved.is_empty() {
        return "(none)".into();
    }
    let mut text = String::new();
    let mut shown = 0usize;
    for address in resolved {
        let candidate = if shown == 0 {
            address.clone()
        } else {
            format!("{text}, {address}")
        };
        if candidate.chars().count() > budget {
            break;
        }
        text = candidate;
        shown += 1;
    }
    if shown == resolved.len() {
        return text;
    }
    let suffix = format!(" +{} more", resolved.len() - shown.max(1));
    let keep = budget.saturating_sub(suffix.chars().count());
    let head: String = if shown == 0 {
        resolved[0].chars().take(keep).collect()
    } else {
        text.chars().take(keep).collect()
    };
    format!("{head}{suffix}")
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
    use fluzo_core::model_discovery::Error;
    use std::collections::BTreeMap;

    /// Mirrors the runtime's consent gate: a probe never connects and always
    /// answers with a notice, and a discovery request for a non-loopback endpoint
    /// with no matching acceptance is refused with a notice instead of models.
    #[derive(Default)]
    struct Port {
        class: Option<AddressClass>,
        probes: Vec<Probe>,
        accepted: Vec<(String, AddressClass)>,
        declined: Vec<String>,
        requests: Vec<Request>,
        cancelled: Vec<u64>,
        statuses: BTreeMap<u64, Status>,
    }
    impl Port {
        fn new(class: AddressClass) -> Self {
            Self {
                class: Some(class),
                ..Self::default()
            }
        }
        fn class(&self) -> AddressClass {
            self.class.unwrap_or(AddressClass::Loopback)
        }
        fn notice(&self, endpoint: &str, credential_attached: bool) -> Notice {
            Notice {
                endpoint: endpoint.to_owned(),
                resolved: match self.class() {
                    AddressClass::Loopback => vec!["127.0.0.1".into()],
                    AddressClass::Private => {
                        vec!["192.168.1.20".into(), "192.168.1.21".into()]
                    }
                    AddressClass::Public => vec!["203.0.113.7".into()],
                    AddressClass::Unresolved => vec![],
                },
                class: self.class(),
                credential_attached,
            }
        }
    }
    impl ModelDiscoveryPort for Port {
        fn probe(&mut self, probe: Probe) -> Result<(), Error> {
            let notice = self.notice(&probe.endpoint, probe.authorization_env.is_some());
            self.statuses.insert(probe.id, Status::Notice(notice));
            self.probes.push(probe);
            Ok(())
        }
        fn accept(&mut self, endpoint: &str, class: AddressClass) -> Result<(), Error> {
            self.accepted.push((endpoint.to_owned(), class));
            Ok(())
        }
        fn decline(&mut self, endpoint: &str) -> Result<(), Error> {
            self.declined.push(endpoint.to_owned());
            Ok(())
        }
        fn submit(&mut self, request: Request) -> Result<(), Error> {
            let class = self.class();
            let status = if class != AddressClass::Loopback
                && !self.accepted.contains(&(request.endpoint.clone(), class))
            {
                Status::Notice(self.notice(&request.endpoint, request.authorization_env.is_some()))
            } else {
                Status::Complete(Ok(vec!["fixture".into(), "second".into()]))
            };
            self.statuses.insert(request.id, status);
            self.requests.push(request);
            Ok(())
        }
        fn status(&mut self, id: u64) -> Status {
            self.statuses.get(&id).cloned().unwrap_or(Status::Unknown)
        }
        fn cancel(&mut self, id: u64) {
            self.cancelled.push(id);
        }
    }
    fn press(wizard: &mut ModelWizard, port: &mut Port, code: KeyCode) {
        wizard.key(KeyEvent::new(code, KeyModifiers::NONE), port);
    }
    fn ctrl(wizard: &mut ModelWizard, port: &mut Port, code: KeyCode) {
        wizard.key(KeyEvent::new(code, KeyModifiers::CONTROL), port);
    }
    fn at_url(class: AddressClass) -> (ModelWizard, Port) {
        let mut wizard = ModelWizard::default();
        let mut port = Port::new(class);
        wizard.show();
        for _ in 0..3 {
            press(&mut wizard, &mut port, KeyCode::Enter);
        }
        assert_eq!(wizard.step, Step::Endpoint);
        (wizard, port)
    }
    /// Type a URL, leave the field and let the probe answer.
    fn warn(wizard: &mut ModelWizard, port: &mut Port, url: &str) {
        wizard.paste(url);
        assert!(port.probes.is_empty(), "typing or paste must not probe");
        press(wizard, port, KeyCode::Tab);
        assert!(wizard.poll(port));
        assert_eq!(wizard.step, Step::Notice);
        assert!(
            port.requests.is_empty(),
            "the notice must precede any request"
        );
    }
    fn draw(wizard: &ModelWizard, width: u16, height: u16, color: bool) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        let shell = Shell::configuration_shell().unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("retained background"), frame.area());
                wizard.render(frame, &shell, color);
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
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
    fn loopback_blur_probes_then_discovers_without_a_notice() {
        let (mut wizard, mut port) = at_url(AddressClass::Loopback);
        wizard.paste("http://127.0.0.1:1234");
        assert!(port.probes.is_empty());
        assert!(port.requests.is_empty());
        press(&mut wizard, &mut port, KeyCode::Tab);
        assert_eq!(port.probes.len(), 1);
        assert_eq!(port.probes[0].endpoint, "http://127.0.0.1:1234/v1");
        assert_eq!(wizard.step, Step::Loading);
        assert!(wizard.poll(&mut port));
        assert_ne!(wizard.step, Step::Notice);
        assert_eq!(port.requests.len(), 1);
        assert!(port.accepted.is_empty(), "loopback needs no acceptance");
        assert!(wizard.poll(&mut port));
        assert_eq!(wizard.step, Step::Models);
        assert_eq!(wizard.models.len(), 2);
        press(&mut wizard, &mut port, KeyCode::Tab);
        assert_eq!(port.requests.len(), 1);
    }
    #[test]
    fn cancel_fences_a_pending_probe_and_a_pending_discovery() {
        let (mut wizard, mut port) = at_url(AddressClass::Loopback);
        wizard.paste("http://127.0.0.1:1234");
        press(&mut wizard, &mut port, KeyCode::Tab);
        press(&mut wizard, &mut port, KeyCode::Esc);
        assert_eq!(port.cancelled, vec![1]);
        assert!(!wizard.poll(&mut port));
        assert!(wizard.models.is_empty());
        assert!(wizard.completed.is_none());
        press(&mut wizard, &mut port, KeyCode::Tab);
        assert!(wizard.poll(&mut port));
        assert_eq!(wizard.step, Step::Loading);
        press(&mut wizard, &mut port, KeyCode::Esc);
        assert_eq!(port.cancelled, vec![1, 3]);
        assert!(!wizard.poll(&mut port));
        assert!(wizard.models.is_empty());
        assert_eq!(port.requests.len(), 1);
    }
    #[test]
    fn non_loopback_warns_before_any_request_then_accept_submits_that_endpoint() {
        let (mut wizard, mut port) = at_url(AddressClass::Private);
        warn(&mut wizard, &mut port, "http://model.internal:1234");
        let notice = wizard.notice.clone().unwrap();
        assert_eq!(notice.endpoint, "http://model.internal:1234/v1");
        assert_eq!(notice.class, AddressClass::Private);
        assert!(!notice.credential_attached);
        press(&mut wizard, &mut port, KeyCode::Enter);
        assert_eq!(
            port.accepted,
            vec![(
                "http://model.internal:1234/v1".to_owned(),
                AddressClass::Private
            )]
        );
        assert_eq!(port.requests.len(), 1);
        assert_eq!(port.requests[0].endpoint, "http://model.internal:1234/v1");
        assert_eq!(wizard.step, Step::Loading);
        assert!(wizard.poll(&mut port));
        assert_eq!(wizard.step, Step::Models);
    }
    #[test]
    fn declining_the_notice_sends_nothing_and_stages_no_state() {
        let (mut wizard, mut port) = at_url(AddressClass::Public);
        warn(&mut wizard, &mut port, "http://models.example.com");
        press(&mut wizard, &mut port, KeyCode::Esc);
        assert_eq!(wizard.step, Step::Endpoint);
        assert!(wizard.notice.is_none());
        assert!(wizard.accepted.is_none());
        assert!(wizard.pending.is_none());
        assert!(wizard.models.is_empty());
        assert!(wizard.completed.is_none());
        assert!(port.accepted.is_empty());
        assert!(port.requests.is_empty());
        assert_eq!(port.declined, vec!["http://models.example.com/v1"]);
        assert!(!wizard.poll(&mut port));
    }
    #[test]
    fn editing_the_endpoint_after_acceptance_invalidates_it() {
        let (mut wizard, mut port) = at_url(AddressClass::Private);
        warn(&mut wizard, &mut port, "http://model.internal:1234");
        press(&mut wizard, &mut port, KeyCode::Enter);
        assert!(wizard.poll(&mut port));
        assert_eq!(wizard.step, Step::Models);
        press(&mut wizard, &mut port, KeyCode::Esc);
        assert_eq!(wizard.step, Step::Endpoint);
        assert!(wizard.accepted.is_some());
        ctrl(&mut wizard, &mut port, KeyCode::Char('u'));
        assert!(wizard.accepted.is_none());
        assert!(wizard.endpoint.is_empty());
        wizard.paste("http://other.internal:1234");
        press(&mut wizard, &mut port, KeyCode::Tab);
        assert!(wizard.poll(&mut port));
        assert_eq!(wizard.step, Step::Notice);
        assert_eq!(
            wizard.notice.clone().unwrap().endpoint,
            "http://other.internal:1234/v1"
        );
        assert_eq!(
            port.requests.len(),
            1,
            "only the accepted endpoint was requested"
        );
        press(&mut wizard, &mut port, KeyCode::Esc);
        wizard.paste("http://model.internal:1234");
        press(&mut wizard, &mut port, KeyCode::Tab);
        assert!(wizard.poll(&mut port));
        assert_eq!(
            wizard.step,
            Step::Notice,
            "returning to an accepted URL after an edit must re-warn"
        );
        assert_eq!(port.requests.len(), 1);
    }
    #[test]
    fn a_class_change_at_connect_time_rewarns_instead_of_connecting() {
        let (mut wizard, mut port) = at_url(AddressClass::Private);
        warn(&mut wizard, &mut port, "http://model.internal:1234");
        press(&mut wizard, &mut port, KeyCode::Enter);
        assert_eq!(wizard.step, Step::Loading);
        // The runtime re-resolves at connect and finds a different class than the
        // accepted one, so it answers the request with a notice instead of models.
        let request = port.requests.last().unwrap().id;
        port.class = Some(AddressClass::Public);
        port.accepted.clear();
        let late = port.notice(&wizard.endpoint, false);
        port.statuses.insert(request, Status::Notice(late));
        assert!(wizard.poll(&mut port));
        assert_eq!(wizard.step, Step::Notice);
        assert_eq!(wizard.notice.unwrap().class, AddressClass::Public);
        assert_eq!(
            port.requests.len(),
            1,
            "the refused request is the only one"
        );
    }
    #[test]
    fn the_notice_is_inert_except_accept_and_decline() {
        let (mut wizard, mut port) = at_url(AddressClass::Public);
        warn(&mut wizard, &mut port, "http://models.example.com");
        let probes = port.probes.len();
        for code in [
            KeyCode::Char('x'),
            KeyCode::Backspace,
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Char(' '),
            KeyCode::Left,
            KeyCode::Right,
        ] {
            press(&mut wizard, &mut port, code);
            wizard.paste("http://evil.example.com");
        }
        ctrl(&mut wizard, &mut port, KeyCode::Char('u'));
        assert_eq!(port.probes.len(), probes);
        assert!(port.requests.is_empty());
        assert_eq!(wizard.step, Step::Notice);
        assert!(!wizard.poll(&mut port));
        let area = Rect::new(0, 0, 120, 40);
        wizard.mouse(
            crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                modifiers: crossterm::event::KeyModifiers::NONE,
                column: 4,
                row: 4,
            },
            area,
            &mut port,
        );
        assert_eq!(port.probes.len(), probes);
        assert!(port.requests.is_empty());
    }
    #[test]
    fn notice_names_endpoint_addresses_transport_and_credential() {
        for (width, height) in [(80, 24), (120, 40), (160, 50)] {
            for color in [false, true] {
                let wizard = ModelWizard {
                    authorized: true,
                    header_env: "FLUZO_KEY".into(),
                    notice: Some(Notice {
                        endpoint: "http://model.internal:1234/v1".into(),
                        resolved: vec!["192.168.1.20".into(), "192.168.1.21".into()],
                        class: AddressClass::Private,
                        credential_attached: true,
                    }),
                    step: Step::Notice,
                    ..ModelWizard::default()
                };
                let text = draw(&wizard, width, height, color);
                assert!(text.contains("4 / Cleartext warning"), "{width}x{height}");
                assert!(
                    text.contains("Cleartext HTTP: HTTPS is unsupported for discovery"),
                    "{width}x{height}"
                );
                assert!(
                    text.contains("Endpoint: http://model.internal:1234/v1"),
                    "{width}x{height}"
                );
                assert!(
                    text.contains("192.168.1.20, 192.168.1.21"),
                    "{width}x{height}"
                );
                assert!(text.contains("private network"), "{width}x{height}");
                assert!(
                    text.contains("Credential: attached from FLUZO_KEY"),
                    "{width}x{height}"
                );
                assert!(text.contains("retained background"), "{width}x{height}");
                assert!(text.contains("Esc back/cancel"), "{width}x{height}");
            }
        }
    }
    #[test]
    fn notice_sanitizes_control_and_bidi_text() {
        let wizard = ModelWizard {
            notice: Some(Notice {
                endpoint: "http://evil\u{202e}gpoc.example/v1".into(),
                resolved: vec!["\u{1b}[31m10.0.0.1".into()],
                class: AddressClass::Private,
                credential_attached: false,
            }),
            step: Step::Notice,
            ..ModelWizard::default()
        };
        let text = draw(&wizard, 120, 40, false);
        assert!(!text.contains('\u{202e}'));
        assert!(!text.contains('\u{1b}'));
        assert!(text.contains("\\u{202e}"));
        assert!(text.contains("\\u{1b}"));
        assert!(text.contains("Credential: not attached"));
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
                Step::Notice,
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
