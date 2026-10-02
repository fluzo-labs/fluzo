use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::configuration::ConfigurationPort;
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{
    configuration::ConfigurationView,
    shell::Shell,
    visual::{Preferences, Theme, VisualOptions},
};

const COMMANDS: [(&str, &str); 10] = [
    ("Configuration > Models", "models."),
    ("Configuration > Limits", "harness."),
    ("Configuration > Pools", "capacity_pools."),
    ("Configuration > Storage", "storage."),
    ("Configuration > Telemetry", "telemetry."),
    ("Configuration > Theme", "tui.theme"),
    ("Configuration > Notifications", "tui.notifications."),
    ("Configuration > Developer preferences", "tui.dev_menu"),
    ("Configuration > All / advanced", ""),
    ("Developer menu > Presentation settings", "tui."),
];

pub struct Workspace {
    pub shell: Shell,
    pub configuration: ConfigurationView,
    palette: bool,
    query: String,
    selection: usize,
    ascii: bool,
}

impl Workspace {
    pub fn new(repository: &str, ascii: bool) -> Result<Self, &'static str> {
        let mut shell = Shell::configuration_shell()?;
        shell.set_repository(repository);
        let mut configuration = ConfigurationView::default();
        configuration.target = repository.into();
        Ok(Self {
            shell,
            configuration,
            palette: false,
            query: String::new(),
            selection: 0,
            ascii,
        })
    }

    fn actions(&self) -> Vec<(&'static str, &'static str)> {
        COMMANDS
            .iter()
            .copied()
            .filter(|(name, _)| {
                !name.starts_with("Developer menu") || self.shell.preferences.effective().dev_menu
            })
            .filter(|(name, _)| name.to_lowercase().contains(&self.query.to_lowercase()))
            .collect()
    }

    pub fn poll(&mut self, port: &mut dyn ConfigurationPort) -> bool {
        let changed = self.configuration.poll(port);
        if let Some(snapshot) = &self.configuration.snapshot
            && self.shell.preferences.applied() != &snapshot.effective_ui
        {
            let version = self.shell.preferences.version.saturating_add(1);
            if let Ok(mut preferences) = Preferences::new(VisualOptions {
                settings: snapshot.effective_ui.clone(),
                ascii: self.ascii,
                ..Default::default()
            }) {
                preferences.version = version;
                self.shell.preferences = preferences;
            }
        }
        self.shell.preferences.ascii = self.ascii;
        changed
    }

    pub fn key(&mut self, key: KeyEvent, port: &mut dyn ConfigurationPort) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'q'))
        {
            self.shell.quit = true;
            return;
        }
        if self.configuration.open {
            self.configuration.key(key, port);
            return;
        }
        if self.palette {
            match key.code {
                KeyCode::Esc => self.palette = false,
                KeyCode::Up => self.selection = self.selection.saturating_sub(1),
                KeyCode::Down => {
                    self.selection =
                        (self.selection + 1).min(self.actions().len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Some((_, filter)) = self.actions().get(self.selection) {
                        self.configuration.show(filter);
                        self.palette = false;
                    }
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.selection = 0;
                }
                KeyCode::Char(character)
                    if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                        && self.query.len() < 128 =>
                {
                    self.query.push(character);
                    self.selection = 0;
                }
                _ => {}
            }
        } else if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('p' | 'g' | 'l'))
        {
            self.palette = true;
            self.query.clear();
            self.selection = 0;
        } else {
            self.shell.configuration_input(key);
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.configuration.open {
            self.configuration.paste(text);
        } else if !self.palette {
            self.shell.paste(text);
        }
    }

    pub fn render(&mut self, frame: &mut Frame, color: bool) {
        self.shell.resize(frame.area());
        self.shell.render(frame, color);
        if self.configuration.open {
            self.configuration.render(frame, color, self.ascii);
        } else if self.palette {
            let area = frame.area();
            if area.width < 60 || area.height < 16 {
                return;
            }
            let popup = Rect::new(area.x + 2, area.y + 2, area.width - 4, area.height - 4);
            let theme = Theme::new(self.shell.preferences.effective(), color);
            let mut lines = vec![Line::from(format!(
                "Search: {}",
                crate::setup::safe_text(&self.query)
            ))];
            lines.extend(self.actions().iter().enumerate().map(|(index, (name, _))| {
                Line::from(format!(
                    "{} {name}",
                    if index == self.selection { ">" } else { " " }
                ))
            }));
            lines.push(Line::from("Enter opens | Esc returns | Ctrl+C exits"));
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Commands / configuration only")
                        .style(theme.base),
                ),
                popup,
            );
        }
    }
}
