use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::configuration::ConfigurationPort;
use ratatui::{Frame, layout::Rect, text::Line, widgets::Paragraph};

use crate::{
    configuration::ConfigurationView,
    shell::Shell,
    visual::{Preferences, Theme, VisualOptions},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Menu {
    #[default]
    Root,
    Configuration,
    Models,
    Developer,
    Plugins,
    Providers,
}

const ROOT_COMMANDS: [(&str, &str); 6] = [
    ("Models", "@models"),
    ("Developer Menu", "@developer"),
    ("Quit", "@quit"),
    ("Plugins", "@plugins"),
    ("Providers", "@providers"),
    ("Configuration", "@configuration"),
];

const COMMANDS: [(&str, &str); 10] = [
    ("Models", "models."),
    ("Limits", "harness."),
    ("Pools", "capacity_pools."),
    ("Storage", "storage."),
    ("Telemetry", "telemetry."),
    ("Theme", "tui.theme"),
    ("Notifications", "tui.notifications."),
    ("Developer preferences", "tui.dev_menu"),
    ("Presentation", "tui."),
    ("All / advanced", ""),
];

pub struct Workspace {
    pub shell: Shell,
    pub configuration: ConfigurationView,
    pub model_wizard: crate::model_wizard::ModelWizard,
    palette: bool,
    menu: Menu,
    parent: Option<(String, usize)>,
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
            model_wizard: Default::default(),
            palette: false,
            menu: Menu::Root,
            parent: None,
            query: String::new(),
            selection: 0,
            ascii,
        })
    }

    fn actions(&self) -> Vec<(&'static str, &'static str)> {
        let commands: &[(&str, &str)] = match self.menu {
            Menu::Root => &ROOT_COMMANDS,
            Menu::Configuration => &COMMANDS,
            Menu::Models => &[
                ("Configured models", "models."),
                ("Add model > Local / LAN", "@add-model"),
            ],
            Menu::Developer => &[("Presentation settings", "tui.")],
            Menu::Plugins | Menu::Providers => &[],
        };
        commands
            .iter()
            .copied()
            .filter(|(name, _)| {
                *name != "Developer Menu" || self.shell.preferences.effective().dev_menu
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

    pub fn open_configuration_menu(&mut self) {
        self.palette = true;
        self.menu = Menu::Configuration;
        self.parent = None;
        self.query.clear();
        self.selection = 0;
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
            if !self.configuration.open {
                self.palette = true;
            }
            return;
        }
        if self.palette {
            match key.code {
                KeyCode::Esc => {
                    if let Some((query, selection)) = self.parent.take() {
                        self.menu = Menu::Root;
                        self.query = query;
                        self.selection = selection;
                    } else {
                        self.palette = false;
                    }
                }
                KeyCode::Up => self.selection = self.selection.saturating_sub(1),
                KeyCode::Down => {
                    self.selection =
                        (self.selection + 1).min(self.actions().len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Some((_, filter)) = self.actions().get(self.selection).copied() {
                        let submenu = match filter {
                            "@configuration" => Some(Menu::Configuration),
                            "@models" => Some(Menu::Models),
                            "@developer" => Some(Menu::Developer),
                            "@plugins" => Some(Menu::Plugins),
                            "@providers" => Some(Menu::Providers),
                            _ => None,
                        };
                        if let Some(menu) = submenu {
                            self.parent = Some((std::mem::take(&mut self.query), self.selection));
                            self.menu = menu;
                            self.selection = 0;
                        } else {
                            match filter {
                                "@quit" => self.shell.quit = true,
                                "@add-model" => self.model_wizard.show(),
                                _ => self.configuration.show(filter),
                            }
                            self.palette = false;
                        }
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
            self.menu = Menu::Root;
            self.parent = None;
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
        self.shell.preferences.cancel();
        if let Some(theme) = self.configuration.preview_theme()
            && theme != self.shell.preferences.applied().theme
        {
            self.shell.preferences.begin();
            let _ = self.shell.preferences.change("tui.theme", 1);
        }
        self.shell.render(frame, color);
        if self.model_wizard.open {
            self.model_wizard.render(frame, &self.shell, color);
        } else if self.configuration.open {
            self.configuration.render_dialog(frame, &self.shell, color);
        } else if self.palette {
            let area = frame.area();
            if area.width < 60 || area.height < 16 {
                return;
            }
            let width = area.width.saturating_sub(4).min(70);
            let height = area.height.saturating_sub(4).min(20);
            let popup = Rect::new(
                area.x + (area.width - width) / 2,
                area.y + (area.height - height) / 2,
                width,
                height,
            );
            let theme = Theme::new(self.shell.preferences.effective(), color);
            self.shell.render_dialog_frame(
                frame,
                popup,
                Theme::new(self.shell.preferences.effective(), color),
            );
            self.shell.render_dialog_title(
                frame,
                Rect::new(popup.x + 2, popup.y + 1, width - 4, 1),
                match self.menu {
                    Menu::Root => "Commands",
                    Menu::Configuration => "Commands / Configuration",
                    Menu::Models => "Commands / Models",
                    Menu::Developer => "Commands / Developer Menu",
                    Menu::Plugins => "Commands / Plugins",
                    Menu::Providers => "Commands / Providers",
                },
                color,
            );
            let roomy = height >= 16;
            let search_y = popup.y + if roomy { 3 } else { 2 };
            let body_y = search_y + if roomy { 2 } else { 1 };
            let footer_y = popup.bottom() - 2;
            let rows = usize::from(footer_y.saturating_sub(body_y));
            let query = crate::setup::safe_text(&self.query);
            let visible_query: String = query
                .chars()
                .rev()
                .take(usize::from(width - 13))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            frame.render_widget(
                Paragraph::new(format!("Search: {visible_query}")).style(theme.base),
                Rect::new(popup.x + 2, search_y, width - 4, 1),
            );
            let first = self.selection.saturating_sub(rows.saturating_sub(1));
            let actions = self.actions();
            let mut lines = Vec::new();
            if actions.is_empty() {
                lines.push(Line::from(match self.menu {
                    Menu::Plugins => "Plugins are not implemented. No plugins are loaded.",
                    Menu::Providers => "Provider management is not implemented.",
                    _ => "No matching actions",
                }));
            }
            for (index, (name, _)) in actions.iter().enumerate().skip(first).take(rows) {
                let selected = index == self.selection;
                let style = if selected && color {
                    theme
                        .base
                        .bg(theme.accent.fg.unwrap_or(ratatui::style::Color::Magenta))
                        .fg(
                            if self.shell.preferences.effective().theme == "high-contrast" {
                                ratatui::style::Color::Black
                            } else {
                                ratatui::style::Color::Rgb(255, 250, 241)
                            },
                        )
                } else if selected {
                    theme.accent
                } else {
                    theme.base
                };
                lines.push(Line::styled(
                    format!(
                        "{} {:<padding$}",
                        if selected && !color { ">" } else { " " },
                        name,
                        padding = usize::from(width - 4)
                    ),
                    style,
                ));
            }
            frame.render_widget(
                Paragraph::new(lines).style(theme.base),
                Rect::new(popup.x + 1, body_y, width - 2, rows as u16),
            );
            frame.render_widget(
                Paragraph::new("Enter opens | Esc returns | Ctrl+C exits").style(theme.muted),
                Rect::new(popup.x + 2, footer_y, width - 4, 1),
            );
            frame.set_cursor_position((
                popup.x + 10 + visible_query.chars().count() as u16,
                search_y,
            ));
            crate::visual::terminal_colors(frame.buffer_mut(), color, self.shell.truecolor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn palette_preserves_shared_dialog_design_and_selected_row_after_resize() {
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            for ascii in [false, true] {
                let mut workspace = Workspace::new("/fixture", ascii).unwrap();
                workspace.shell.preferences.ascii = ascii;
                workspace.shell.truecolor = true;
                workspace.palette = true;
                workspace.selection = workspace.actions().len() - 1;
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| workspace.render(frame, true))
                    .unwrap();
                let popup_width = (width - 4).min(70);
                let popup_height = (height - 4).min(20);
                let left = (width - popup_width) / 2;
                let top = (height - popup_height) / 2;
                let buffer = terminal.backend().buffer();
                assert_eq!(buffer[(left, top)].symbol(), if ascii { "+" } else { "╭" });
                let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                assert!(text.contains("Configuration"));
                assert!(text.contains("Models"));
                assert!(!text.contains("Limits"));
                assert!(text.contains("Esc returns"));
                assert!(text.contains(if ascii { "/" } else { "╱" }));
                let theme = Theme::new(workspace.shell.preferences.effective(), true);
                assert!(
                    buffer
                        .content
                        .iter()
                        .any(|cell| cell.bg == theme.accent.fg.unwrap())
                );
                assert!(!text.contains("Play synthetic"));
            }
        }
    }

    #[test]
    fn open_configuration_menu_lands_on_configuration_commands() {
        for ascii in [false, true] {
            let mut workspace = Workspace::new("/fixture", ascii).unwrap();
            workspace.query = "zzz".into();
            workspace.selection = 5;
            workspace.open_configuration_menu();
            assert!(workspace.query.is_empty());
            assert_eq!(workspace.selection, 0);
            let filters: Vec<&str> = workspace
                .actions()
                .iter()
                .map(|(_, filter)| *filter)
                .collect();
            assert!(filters.contains(&"harness."), "{filters:?}");
            assert!(filters.contains(&"tui.theme"), "{filters:?}");
            assert!(!filters.contains(&"@models"), "{filters:?}");
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal
                .draw(|frame| workspace.render(frame, true))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Limits"), "{text}");
            assert!(text.contains("Pools"), "{text}");
        }
    }
}
