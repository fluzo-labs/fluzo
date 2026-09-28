use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fluzo_core::application::{PROTOCOL_VERSION, Snapshot};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const DRAFT_LIMIT: usize = 8192;
const HISTORY_LIMIT: usize = 256;
const LINE_LIMIT: usize = 512;
const ACTIONS: [&str; 5] = [
    "Play synthetic stream",
    "Stop preview",
    "Follow output",
    "Help",
    "Quit demo",
];

#[derive(Default)]
pub struct Sanitizer {
    escape: u8,
}

impl Sanitizer {
    pub fn push(&mut self, text: &str) -> String {
        let mut output = String::new();
        for character in text.chars() {
            match self.escape {
                1 => {
                    self.escape = match character {
                        '[' => 2,
                        ']' | 'P' | '^' | '_' => 3,
                        _ => 0,
                    }
                }
                2 => {
                    if ('\u{40}'..='\u{7e}').contains(&character) {
                        self.escape = 0;
                    }
                }
                3 => {
                    if character == '\u{7}' || character == '\u{9c}' {
                        self.escape = 0;
                    } else if character == '\u{1b}' {
                        self.escape = 4;
                    }
                }
                4 => {
                    self.escape = if character == '\\' { 0 } else { 3 };
                }
                _ => match character {
                    '\u{1b}' => self.escape = 1,
                    '\u{9b}' => self.escape = 2,
                    '\u{90}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => self.escape = 3,
                    '\n' => output.push('\n'),
                    '\t' => output.push_str("    "),
                    '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}' => {
                        output.push_str(&format!("[U+{:04X}]", character as u32))
                    }
                    control if control.is_control() => {}
                    visible => output.push(visible),
                },
            }
        }
        output
    }
}

#[derive(Default)]
struct Editor {
    text: String,
    cursor: usize,
}

impl Editor {
    fn insert(&mut self, text: &str) -> bool {
        if self.text.len().saturating_add(text.len()) > DRAFT_LIMIT {
            return false;
        }
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        true
    }

    fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(offset, _)| offset);
    }

    fn right(&mut self) {
        if let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() {
            self.cursor += grapheme.len();
        }
    }

    fn key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => {
                self.cursor = self.text[..self.cursor]
                    .rfind('\n')
                    .map_or(0, |offset| offset + 1)
            }
            KeyCode::End => {
                self.cursor += self.text[self.cursor..]
                    .find('\n')
                    .unwrap_or(self.text.len() - self.cursor)
            }
            KeyCode::Backspace => {
                let end = self.cursor;
                self.left();
                self.text.drain(self.cursor..end);
            }
            KeyCode::Delete => {
                let start = self.cursor;
                self.right();
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Up | KeyCode::Down => {
                let start = self.text[..self.cursor]
                    .rfind('\n')
                    .map_or(0, |offset| offset + 1);
                let column = self.text[start..self.cursor].graphemes(true).count();
                let target = if code == KeyCode::Up {
                    if start == 0 {
                        return;
                    }
                    self.text[..start - 1]
                        .rfind('\n')
                        .map_or(0, |offset| offset + 1)
                } else {
                    let Some(end) = self.text[self.cursor..].find('\n') else {
                        return;
                    };
                    self.cursor + end + 1
                };
                self.cursor = target
                    + self.text[target..]
                        .split('\n')
                        .next()
                        .unwrap_or("")
                        .graphemes(true)
                        .take(column)
                        .map(str::len)
                        .sum::<usize>();
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Composer,
    Conversation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    Palette,
    Help,
    Quit,
}

pub struct Shell {
    draft: Editor,
    pub focus: Focus,
    overlay: Option<Overlay>,
    filter: String,
    selection: usize,
    lines: VecDeque<String>,
    first_line: usize,
    next_line: usize,
    anchor: Option<usize>,
    horizontal: usize,
    pub unread: usize,
    pub playing: bool,
    pub quit: bool,
    pub status: String,
    sanitizer: Sanitizer,
    snapshot: Snapshot,
    dropped: bool,
}

impl Shell {
    pub fn new(snapshot: Snapshot) -> Result<Self, &'static str> {
        if snapshot.protocol_version != PROTOCOL_VERSION
            || !snapshot.demo
            || snapshot.tasks.len() > 64
            || snapshot.tasks.iter().any(|task| task.validate().is_err())
        {
            return Err("Interactive preview requires a valid synthetic snapshot.");
        }
        let mut state = Self {
            draft: Editor::default(),
            focus: Focus::Composer,
            overlay: None,
            filter: String::new(),
            selection: 0,
            lines: VecDeque::new(),
            first_line: 0,
            next_line: 0,
            anchor: None,
            horizontal: 0,
            unread: 0,
            playing: false,
            quit: false,
            status: "Ready. No agent, tools or providers are connected.".into(),
            sanitizer: Sanitizer::default(),
            snapshot,
            dropped: false,
        };
        state.append(
            "System",
            "Synthetic workspace. Write in any language; Enter adds a line.",
        );
        state.append(
            "System",
            "Ctrl+P opens actions. Ctrl+S previews your draft; nothing is executed.",
        );
        Ok(state)
    }

    fn append(&mut self, origin: &str, text: &str) {
        for line in text.split('\n') {
            let mut chunk = format!("{origin} | ");
            for grapheme in line.graphemes(true) {
                if chunk.len() + grapheme.len() > LINE_LIMIT {
                    self.add_line(std::mem::take(&mut chunk));
                    chunk.push_str("       | ");
                }
                if grapheme.len() <= LINE_LIMIT - 10 {
                    chunk.push_str(grapheme);
                } else {
                    chunk.push_str("[oversized glyph]");
                }
            }
            self.add_line(chunk);
        }
    }

    fn add_line(&mut self, line: String) {
        self.lines.push_back(line);
        self.next_line += 1;
        if self.lines.len() > HISTORY_LIMIT {
            self.lines.pop_front();
            self.first_line += 1;
            self.dropped = true;
        }
        if let Some(anchor) = self.anchor.as_mut() {
            *anchor = (*anchor).max(self.first_line);
            self.unread = self.unread.saturating_add(1);
        }
    }

    pub fn stream(&mut self, text: &str) {
        let safe = self
            .sanitizer
            .push(&text.chars().take(2048).collect::<String>());
        if !safe.is_empty() {
            self.append("Preview", &safe);
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.overlay.is_some() || self.focus != Focus::Composer {
            return;
        }
        if text.len() > DRAFT_LIMIT {
            self.status = "Paste rejected: maximum draft is 8192 bytes.".into();
            return;
        }
        let safe = Sanitizer::default().push(&text.replace("\r\n", "\n").replace('\r', "\n"));
        if !self.draft.insert(&safe) {
            self.status = "Draft limit reached; existing input preserved.".into();
        }
    }

    fn actions(&self) -> Vec<usize> {
        ACTIONS
            .iter()
            .enumerate()
            .filter_map(|(index, action)| {
                action
                    .to_lowercase()
                    .contains(&self.filter.to_lowercase())
                    .then_some(index)
            })
            .collect()
    }

    fn follow(&mut self) {
        self.anchor = None;
        self.unread = 0;
    }

    fn action(&mut self, index: usize) {
        self.overlay = None;
        match index {
            0 => {
                self.playing = true;
                self.status = "Synthetic playback; no task is running.".into();
            }
            1 => {
                self.playing = false;
                self.status = "Preview stopped; draft and partial output preserved.".into();
            }
            2 => self.follow(),
            3 => self.overlay = Some(Overlay::Help),
            4 => self.request_quit(),
            _ => {}
        }
    }

    fn request_quit(&mut self) {
        if self.draft.text.is_empty() {
            self.quit = true;
        } else {
            self.overlay = Some(Overlay::Quit);
            self.selection = 0;
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if control && key.code == KeyCode::Char('c') {
            self.action(1);
            return;
        }
        if key.kind == KeyEventKind::Repeat && (control || self.overlay.is_some()) {
            return;
        }
        if control && key.code == KeyCode::Char('q') {
            self.request_quit();
            return;
        }
        if key.code == KeyCode::Esc {
            self.overlay = None;
            return;
        }
        if self.overlay == Some(Overlay::Quit) {
            match key.code {
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    self.selection = 1 - self.selection
                }
                KeyCode::Enter if self.selection == 1 => self.quit = true,
                KeyCode::Enter => self.overlay = None,
                _ => {}
            }
            return;
        }
        if self.overlay == Some(Overlay::Help) {
            return;
        }
        if self.overlay == Some(Overlay::Palette) {
            match key.code {
                KeyCode::Char(character)
                    if !control
                        && !key.modifiers.contains(KeyModifiers::ALT)
                        && !character.is_control()
                        && self.filter.len() < 64 =>
                {
                    self.filter.push(character);
                    self.selection = 0;
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.selection = 0;
                }
                KeyCode::Down => {
                    self.selection =
                        (self.selection + 1).min(self.actions().len().saturating_sub(1))
                }
                KeyCode::Up => self.selection = self.selection.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(index) = self.actions().get(self.selection).copied() {
                        self.action(index);
                    }
                }
                _ => {}
            }
            return;
        }
        if control && key.code == KeyCode::Char('p') {
            self.overlay = Some(Overlay::Palette);
            self.filter.clear();
            self.selection = 0;
            return;
        }
        if key.code == KeyCode::F(1) {
            self.overlay = Some(Overlay::Help);
            return;
        }
        if key.code == KeyCode::Tab {
            self.focus = if self.focus == Focus::Composer {
                Focus::Conversation
            } else {
                Focus::Composer
            };
            return;
        }
        match key.code {
            KeyCode::PageUp => {
                self.anchor = Some(
                    self.anchor
                        .unwrap_or(self.next_line.saturating_sub(8))
                        .saturating_sub(8)
                        .max(self.first_line),
                )
            }
            KeyCode::PageDown => {
                if let Some(anchor) = self.anchor.as_mut() {
                    *anchor = (*anchor + 8).min(self.next_line.saturating_sub(1));
                }
            }
            KeyCode::End if self.focus == Focus::Conversation => self.follow(),
            KeyCode::Left if self.focus == Focus::Conversation => {
                self.horizontal = self.horizontal.saturating_sub(16)
            }
            KeyCode::Right if self.focus == Focus::Conversation => {
                self.horizontal = (self.horizontal + 16).min(LINE_LIMIT)
            }
            KeyCode::Char('s') if control && self.focus == Focus::Composer => {
                if !self.draft.text.trim().is_empty() {
                    self.append("User preview", &self.draft.text.clone());
                    self.status =
                        "Draft previewed only; retained for editing. No command dispatched.".into();
                }
            }
            KeyCode::Enter if self.focus == Focus::Composer => {
                if !self.draft.insert("\n") {
                    self.status = "Draft limit reached.".into();
                }
            }
            KeyCode::Char(character)
                if self.focus == Focus::Composer
                    && !control
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.paste(&character.to_string())
            }
            code if self.focus == Focus::Composer => self.draft.key(code),
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut Frame, color: bool) {
        let area = frame.area();
        let accent = if color {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        if area.width < 60 || area.height < 16 {
            frame.render_widget(Paragraph::new("Fluzo demo: resize to at least 60x16.\nCtrl+Q exits; drafts stay in memory.\nIf a draft exists: Tab then Enter discards and exits."), area);
            return;
        }
        let layout = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(6),
            Constraint::Length(2),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "FLUZO / INTERACTIVE DEMO / NORMAL (no grants)",
                    accent,
                )),
                Line::from(format!(
                    "Synthetic session | {} fixture task(s) | no workspace execution",
                    self.snapshot.tasks.len()
                )),
            ]),
            layout[0],
        );
        let title = format!(
            "{} Conversation | {}{}",
            if self.focus == Focus::Conversation {
                ">"
            } else {
                " "
            },
            if self.anchor.is_none() {
                "following"
            } else {
                "paused"
            },
            if self.unread > 0 {
                format!(" | {} new", self.unread)
            } else {
                String::new()
            }
        );
        let count = usize::from(layout[1].height.saturating_sub(2));
        let start = self
            .anchor
            .map_or(self.lines.len().saturating_sub(count), |anchor| {
                anchor.saturating_sub(self.first_line)
            });
        let lines: Vec<Line> = self
            .lines
            .iter()
            .skip(start)
            .take(count)
            .map(|line| {
                let mut cells = 0;
                let text: String = line
                    .graphemes(true)
                    .filter(|glyph| {
                        let offset = cells;
                        cells += glyph.width();
                        offset >= self.horizontal
                    })
                    .collect();
                Line::from(text)
            })
            .collect();
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::TOP)
                    .title(title)
                    .border_style(accent),
            ),
            layout[1],
        );
        let draft_lines: Vec<_> = self.draft.text.split('\n').collect();
        let row = self.draft.text[..self.draft.cursor].matches('\n').count();
        let column = self.draft.text[..self.draft.cursor]
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .width();
        let width = usize::from(layout[2].width.saturating_sub(2));
        let horizontal = column.saturating_sub(width.saturating_sub(1));
        let top = row.saturating_sub(3);
        let visible: Vec<Line> = draft_lines
            .iter()
            .skip(top)
            .take(4)
            .map(|line| {
                let mut offset = 0;
                let text: String = line
                    .graphemes(true)
                    .filter(|glyph| {
                        let start = offset;
                        offset += glyph.width();
                        start >= horizontal
                    })
                    .collect();
                Line::from(text)
            })
            .collect();
        frame.render_widget(
            Paragraph::new(visible).block(
                Block::bordered()
                    .title(if self.focus == Focus::Composer {
                        "> Composer | Enter newline | Ctrl+S preview"
                    } else {
                        "Composer (draft preserved)"
                    })
                    .border_style(accent),
            ),
            layout[2],
        );
        if self.focus == Focus::Composer && self.overlay.is_none() {
            frame.set_cursor_position((
                layout[2].x + 1 + (column - horizontal) as u16,
                layout[2].y + 1 + (row - top) as u16,
            ));
        }
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(self.status.as_str()),
                Line::from(if self.dropped {
                    "History bounded/truncated | Ctrl+P actions | F1 help | Ctrl+Q exit"
                } else {
                    "Ctrl+P actions | Tab focus | PgUp/PgDn browse | F1 help | Ctrl+Q exit"
                }),
            ]),
            layout[3],
        );
        if let Some(overlay) = self.overlay {
            let popup = ratatui::layout::Rect::new(
                area.x + 2,
                area.y + 2,
                area.width - 4,
                area.height.saturating_sub(4).min(10),
            );
            let (title, lines) = match overlay {
                Overlay::Palette => {
                    let mut lines = vec![Line::from(format!(
                        "Search: {}",
                        Sanitizer::default().push(&self.filter)
                    ))];
                    let actions = self.actions();
                    if actions.is_empty() {
                        lines.push(Line::from("No matching actions"));
                    }
                    for (position, index) in actions.iter().enumerate() {
                        lines.push(Line::from(format!(
                            "{} {}",
                            if position == self.selection { ">" } else { " " },
                            ACTIONS[*index]
                        )));
                    }
                    ("Command palette | Esc closes", lines)
                }
                Overlay::Help => (
                    "Help | Esc returns",
                    vec![
                        Line::from("Enter: newline   Ctrl+S: preview (draft retained)"),
                        Line::from("Arrows/Home/End: edit   Paste: insert, never submit"),
                        Line::from("PgUp/PgDn: browse   Palette: Follow output"),
                        Line::from("Conversation focus: Left/Right pans long lines"),
                        Line::from("Ctrl+C: stop synthetic playback, not exit"),
                        Line::from("Ctrl+Q: exit   No real approvals or tool execution"),
                    ],
                ),
                Overlay::Quit => (
                    "Discard unsaved demo draft?",
                    vec![
                        Line::from(if self.selection == 0 {
                            "> Keep editing       Discard and exit"
                        } else {
                            "  Keep editing     > Discard and exit"
                        }),
                        Line::from("Tab changes selection; Enter confirms; Esc returns"),
                    ],
                ),
            };
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(lines).block(Block::bordered().title(title).border_style(accent)),
                popup,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluzo_core::application::Cursor;
    use ratatui::{Terminal, backend::TestBackend};

    fn shell() -> Shell {
        Shell::new(Snapshot {
            protocol_version: 1,
            demo: true,
            cursor: Cursor {
                epoch: 1,
                sequence: 0,
            },
            tasks: vec![],
            next_page: None,
        })
        .unwrap()
    }
    fn key(state: &mut Shell, code: KeyCode) {
        state.key(KeyEvent::new(code, KeyModifiers::NONE));
    }
    fn control(state: &mut Shell, character: char) {
        state.key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::CONTROL,
        ));
    }

    #[test]
    fn paste_never_submits_and_overlays_preserve_draft_focus_and_scroll() {
        let mut state = shell();
        state.paste("hola\n世界\u{1b}[31m!");
        assert_eq!(state.draft.text, "hola\n世界!");
        key(&mut state, KeyCode::PageUp);
        let anchor = state.anchor;
        control(&mut state, 'p');
        state.paste("Quit demo\n");
        state.stream("output while palette is open");
        key(&mut state, KeyCode::Esc);
        assert_eq!(state.focus, Focus::Composer);
        assert_eq!(state.anchor, anchor);
        assert_eq!(state.unread, 1);
        assert_eq!(state.draft.text, "hola\n世界!");
        assert!(!state.quit);
        control(&mut state, 'q');
        key(&mut state, KeyCode::Enter);
        assert!(!state.quit);
        control(&mut state, 'q');
        key(&mut state, KeyCode::Tab);
        key(&mut state, KeyCode::Enter);
        assert!(state.quit);
    }

    #[test]
    fn sanitizer_handles_split_sequences_and_bidi_without_buffering() {
        let mut sanitizer = Sanitizer::default();
        assert_eq!(sanitizer.push("safe\u{1b}]52;c;"), "safe");
        assert_eq!(sanitizer.push("secret\u{1b}"), "");
        assert_eq!(sanitizer.push("\\text\r\u{1b}[2J\u{202e}"), "text[U+202E]");
        assert_eq!(sanitizer.push("\u{1b}P"), "");
        for _ in 0..100 {
            assert!(sanitizer.push(&"x".repeat(8192)).is_empty());
        }
        assert_eq!(sanitizer.push("\u{1b}\\done"), "done");
    }

    #[test]
    fn grapheme_editing_and_limits_preserve_valid_drafts() {
        let mut state = shell();
        state.paste("e\u{301}世界");
        key(&mut state, KeyCode::Backspace);
        key(&mut state, KeyCode::Left);
        key(&mut state, KeyCode::Backspace);
        assert_eq!(state.draft.text, "世");
        state.paste(&"x".repeat(DRAFT_LIMIT));
        assert_eq!(state.draft.text, "世");
        for _ in 0..300 {
            state.stream("synthetic");
        }
        assert_eq!(state.lines.len(), HISTORY_LIMIT);
        assert!(state.dropped);
    }

    #[test]
    fn palette_search_actions_and_cancel_never_discard_input() {
        let mut state = shell();
        state.paste("keep");
        control(&mut state, 'p');
        for character in "play".chars() {
            key(&mut state, KeyCode::Char(character));
        }
        key(&mut state, KeyCode::Enter);
        assert!(state.playing);
        control(&mut state, 'c');
        assert!(!state.playing);
        assert_eq!(state.draft.text, "keep");
        assert!(!state.quit);
    }

    #[test]
    fn non_demo_snapshots_and_incompatible_versions_are_rejected() {
        let mut snapshot = shell().snapshot;
        snapshot.demo = false;
        assert!(Shell::new(snapshot.clone()).is_err());
        snapshot.demo = true;
        snapshot.protocol_version = 0;
        assert!(Shell::new(snapshot).is_err());
    }

    #[test]
    fn minimum_palette_keeps_all_actions_visible_and_empty_search_is_safe() {
        let mut state = shell();
        control(&mut state, 'p');
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
        terminal.draw(|frame| state.render(frame, false)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..16)
            .map(|row| {
                (0..60)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect()
            })
            .collect();
        assert!(rows[3].contains("Search:"));
        for (position, action) in ACTIONS.iter().enumerate() {
            assert!(rows[4 + position].contains(action));
        }
        for character in "missing action".chars() {
            key(&mut state, KeyCode::Char(character));
        }
        key(&mut state, KeyCode::Enter);
        assert_eq!(state.overlay, Some(Overlay::Palette));
        assert!(!state.playing);
        assert!(!state.quit);
    }

    #[test]
    fn renders_supported_viewports_and_small_notice_without_controls() {
        for (width, height) in [(80, 24), (120, 40), (160, 50), (60, 16), (30, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut state = shell();
            state.paste("Unicode 世界 e\u{301}");
            state.stream("\u{1b}]52;c;hidden\u{7}safe");
            terminal.draw(|frame| state.render(frame, false)).unwrap();
            let content: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(!content.contains('\u{1b}'));
            assert!(!content.contains("hidden"));
            if width >= 60 {
                assert!(content.contains("Composer"));
                assert!(content.contains("NORMAL"));
            } else {
                assert!(content.contains("resize"));
            }
        }
    }
}
