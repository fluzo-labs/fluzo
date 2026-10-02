use std::collections::VecDeque;
use std::time::Duration;

use crate::visual::{Preferences, Theme, VisualOptions, display_value};

use crate::notification_stack::{Lifetime, NotificationStack, Severity};
use crate::presentation;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use fluzo_core::application::{PROTOCOL_VERSION, Snapshot, TaskState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table};
use unicode_segmentation::UnicodeSegmentation;

const DRAFT_LIMIT: usize = 8192;
const HISTORY_LIMIT: usize = 256;
const LINE_LIMIT: usize = 512;
const ACTIONS: [&str; 7] = [
    "Play synthetic stream",
    "Stop preview",
    "Follow output",
    "Help",
    "Quit demo",
    "Theme preview",
    "Developer menu",
];

const HELP_ROWS: &[(&str, &str, &str)] = &[
    ("Global", "Ctrl+P", "Commands"),
    ("Global", "Ctrl+G", "Toggle help"),
    ("Global", "Ctrl+C", "Quit safely"),
    ("Global", "Esc / Alt+Esc", "Close / cancel"),
    ("Global", "Tab", "Editor / chat focus"),
    ("Global", "Ctrl+B", "Toggle sidebar"),
    ("Global", "Ctrl+D", "Session details"),
    ("Global", "Ctrl+L / Ctrl+M", "Model information"),
    ("Global", "Ctrl+S", "Session information"),
    ("Global", "Ctrl+End", "Follow output"),
    ("Editor", "Enter", "Send demo preview"),
    ("Editor", "Shift+Enter", "Insert newline"),
    ("Editor", "/ (empty draft)", "Commands"),
    ("Editor", "Arrows / Home / End", "Move cursor"),
    ("Editor", "Backspace / Delete", "Delete text"),
    ("Editor", "Ctrl+A / Ctrl+E", "Line start / end"),
    ("Editor", "Ctrl+H", "Backspace"),
    ("Editor", "Ctrl+U / Ctrl+K", "Delete before / after"),
    ("Editor", "Up / Down", "History (one line)"),
    ("Editor", "Paste", "Insert, never submit"),
    ("Chat", "Up / Down / k / j", "Scroll one row"),
    ("Chat", "Ctrl+K / Ctrl+J", "Scroll one row"),
    ("Chat", "Shift+Up/Down / K/J", "Previous / next item"),
    ("Chat", "PgUp/PgDn / b/f", "Scroll one page"),
    ("Chat", "u / d", "Scroll half page"),
    ("Chat", "Home / End / g / G", "Top / bottom"),
    ("Chat", "Space", "Expand / collapse"),
    ("Chat", "Mouse wheel", "Scroll three rows"),
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
pub(crate) struct Editor {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

impl Editor {
    pub(crate) fn insert(&mut self, text: &str) -> bool {
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

    pub(crate) fn key(&mut self, code: KeyCode) {
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
    Theme,
    Developer,
    Models,
    Sessions,
    Details,
}

struct Fragment {
    start: usize,
    end: usize,
    complete: bool,
    expanded: bool,
}

struct DisplayRow {
    source: usize,
    part: usize,
    line: Line<'static>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct RenderGeometry {
    area: Rect,
    loading: Option<Rect>,
}

pub struct Shell {
    #[cfg(test)]
    formatting_passes: std::cell::Cell<usize>,
    draft: Editor,
    pub focus: Focus,
    overlay: Option<Overlay>,
    filter: String,
    selection: usize,
    lines: VecDeque<String>,
    first_line: usize,
    next_line: usize,
    anchor: Option<usize>,
    anchor_part: usize,
    viewport: Rect,
    pub unread: usize,
    pub playing: bool,
    pub quit: bool,
    pub status: String,
    sanitizer: Sanitizer,
    snapshot: Snapshot,
    dropped: bool,
    pub preferences: Preferences,
    pub next_scene: bool,
    pub notification_test: bool,
    notification_editor: Editor,
    editing_notification: bool,
    notification_preview: bool,
    notification_sequence: u64,
    notification_stack: NotificationStack,
    pub notification_transport_supported: bool,
    pub elapsed: Duration,
    changed_at: Duration,
    pub truecolor: bool,
    pub redraws: u64,
    pub skipped: u64,
    pub frame_micros: u128,
    preview_snapshot: Option<Snapshot>,
    repository: String,
    fragments: VecDeque<Fragment>,
    messages: VecDeque<(usize, usize, bool)>,
    footers: VecDeque<(usize, presentation::AssistantFooter)>,
    selected_fragment: Option<usize>,
    active_fragment: Option<usize>,
    playback_started: Duration,
    hide_sidebar: bool,
    scrollbar_until: Duration,
    help_page: usize,
    input_history: VecDeque<String>,
    history_position: Option<usize>,
    history_draft: String,
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
        let mut state = Self::empty(snapshot)?;
        state.append(
            "System",
            "Synthetic workspace. Enter sends a preview; Shift+Enter adds a line.",
        );
        state.append(
            "System",
            "Ctrl+P opens commands. Esc stops playback; Ctrl+C exits. No execution.",
        );
        Ok(state)
    }

    fn empty(snapshot: Snapshot) -> Result<Self, &'static str> {
        Ok(Self {
            #[cfg(test)]
            formatting_passes: std::cell::Cell::new(0),
            draft: Editor::default(),
            focus: Focus::Composer,
            overlay: None,
            filter: String::new(),
            selection: 0,
            lines: VecDeque::new(),
            first_line: 0,
            next_line: 0,
            anchor: None,
            anchor_part: 0,
            viewport: Rect::new(0, 0, 80, 24),
            unread: 0,
            playing: false,
            quit: false,
            status: "Ready. No agent, tools or providers are connected.".into(),
            sanitizer: Sanitizer::default(),
            snapshot,
            dropped: false,
            preferences: Preferences::new(VisualOptions::default())?,
            next_scene: false,
            notification_test: false,
            notification_editor: Editor {
                text: "Synthetic completion test. No agent work was executed.".into(),
                cursor: 0,
            },
            editing_notification: false,
            notification_preview: false,
            notification_sequence: 0,
            notification_stack: NotificationStack::new(Default::default(), 0)
                .map_err(|_| "Invalid default notification settings.")?,
            notification_transport_supported: false,
            elapsed: Duration::ZERO,
            changed_at: Duration::ZERO,
            truecolor: false,
            redraws: 0,
            skipped: 0,
            frame_micros: 0,
            preview_snapshot: None,
            repository: "Not supplied".into(),
            fragments: VecDeque::new(),
            messages: VecDeque::new(),
            footers: VecDeque::new(),
            selected_fragment: None,
            active_fragment: None,
            playback_started: Duration::ZERO,
            hide_sidebar: false,
            scrollbar_until: Duration::ZERO,
            help_page: 0,
            input_history: VecDeque::new(),
            history_position: None,
            history_draft: String::new(),
        })
    }

    pub fn configuration_shell() -> Result<Self, &'static str> {
        let mut shell = Self::empty(Snapshot {
            protocol_version: PROTOCOL_VERSION,
            demo: false,
            cursor: fluzo_core::application::Cursor {
                epoch: 0,
                sequence: 0,
            },
            tasks: vec![],
            next_page: None,
        })?;
        shell.status =
            "Agent runtime unavailable. Ctrl+P opens configuration; no execution.".into();
        shell.append(
            "System",
            "Configuration workspace. No agent runtime, providers or tools are connected.",
        );
        Ok(shell)
    }

    pub(crate) fn configuration_input(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('b') if control => self.hide_sidebar = !self.hide_sidebar,
            KeyCode::Tab => {
                self.focus = if self.focus == Focus::Composer {
                    Focus::Conversation
                } else {
                    Focus::Composer
                }
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.status = "Execution unavailable; composer draft retained.".into()
            }
            KeyCode::Enter if self.focus == Focus::Composer => {
                self.draft.insert("\n");
            }
            KeyCode::Char(character)
                if !control
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.focus == Focus::Composer =>
            {
                self.draft.insert(&character.to_string());
            }
            code if self.focus == Focus::Composer => self.draft.key(code),
            KeyCode::Up => {
                self.scroll_rows(-1);
            }
            KeyCode::Down => {
                self.scroll_rows(1);
            }
            _ => {}
        }
    }

    pub fn set_repository(&mut self, repository: &str) {
        let bounded: String = repository.chars().take(1024).collect();
        self.repository = Sanitizer::default()
            .push(&bounded)
            .replace('\n', "[newline]");
        if repository.chars().count() > 1024 {
            self.repository.push_str(" [truncated]");
        }
        if self.repository.is_empty() {
            self.repository = "Not supplied".into();
        }
    }

    fn compact(&self, area: Rect) -> bool {
        self.hide_sidebar || presentation::compact(area)
    }

    fn body_area(&self, area: Rect) -> Rect {
        presentation::body_mode(area, self.compact(area))
    }

    fn body_layout(&self, area: Rect) -> [Rect; 4] {
        let width = usize::from(self.body_area(area).width.saturating_sub(4));
        let (rows, _, _) = presentation::editor(&self.draft.text, self.draft.cursor, width);
        presentation::layout_mode(area, rows.len(), self.compact(area))
    }

    pub fn resize(&mut self, area: Rect) {
        self.viewport = area;
    }

    pub fn mouse(&mut self, event: MouseEvent, area: Rect) -> bool {
        self.resize(area);
        if self.overlay.is_some() || area.width < 60 || area.height < 16 {
            return false;
        }
        let notice_area = self.notification_area(area);
        let hit = self
            .notification_stack
            .visible(usize::from(notice_area.height / 5))
            .enumerate()
            .find_map(|(index, notice)| {
                let rect = Rect::new(
                    notice_area.x,
                    notice_area.y + index as u16 * 5,
                    notice_area.width,
                    5,
                );
                rect.contains((event.column, event.row).into())
                    .then_some((notice.cursor(), rect))
            });
        if let Some((cursor, rect)) = hit {
            return event.kind == MouseEventKind::Down(MouseButton::Left)
                && Self::notification_close_area(rect).contains((event.column, event.row).into())
                && self.notification_stack.dismiss(cursor);
        }
        let layout = self.body_layout(area);
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && layout[2].contains((event.column, event.row).into())
        {
            let changed = self.focus != Focus::Composer;
            self.focus = Focus::Composer;
            return changed;
        }
        let conversation = layout[1];
        if !conversation.contains((event.column, event.row).into()) {
            return false;
        }
        if event.kind == MouseEventKind::Down(MouseButton::Left) {
            let rows = self.display_rows(conversation.width, false);
            let position = self.display_start(&rows, conversation.height)
                + usize::from(event.row - conversation.y);
            let selected = rows.get(position).map(|row| {
                self.fragments
                    .iter()
                    .find(|fragment| row.source >= fragment.start && row.source < fragment.end)
                    .map_or_else(
                        || {
                            self.messages
                                .iter()
                                .find(|(start, end, _)| row.source >= *start && row.source < *end)
                                .map_or(row.source, |(start, _, _)| *start)
                        },
                        |fragment| fragment.start,
                    )
            });
            let changed = self.selected_fragment != selected || self.focus != Focus::Conversation;
            self.focus = Focus::Conversation;
            self.selected_fragment = selected;
            if !changed
                && let Some(fragment) = self
                    .fragments
                    .iter_mut()
                    .find(|fragment| Some(fragment.start) == selected)
            {
                fragment.expanded = !fragment.expanded;
                return true;
            }
            return changed;
        }
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll_rows(-3),
            MouseEventKind::ScrollDown => self.scroll_rows(3),
            _ => false,
        }
    }

    fn display_start(&self, rows: &[DisplayRow], height: u16) -> usize {
        self.anchor
            .map_or(rows.len().saturating_sub(usize::from(height)), |source| {
                if let Some(first) = rows.iter().position(|row| row.source == source) {
                    let count = rows[first..]
                        .iter()
                        .take_while(|row| row.source == source)
                        .count();
                    first + self.anchor_part.min(count.saturating_sub(1))
                } else {
                    rows.iter()
                        .position(|row| row.source > source)
                        .unwrap_or(rows.len().saturating_sub(1))
                }
            })
    }

    pub(crate) fn scrollbar_visible(&self) -> bool {
        self.elapsed < self.scrollbar_until
    }

    fn scroll_rows(&mut self, delta: isize) -> bool {
        let area = self.body_layout(self.viewport)[1];
        let rows = self.display_rows(area.width, false);
        let bottom = rows.len().saturating_sub(usize::from(area.height));
        if bottom == 0 {
            let changed = self.anchor.is_some() || self.anchor_part != 0;
            self.anchor = None;
            self.anchor_part = 0;
            self.unread = 0;
            return changed;
        }
        let current = self.display_start(&rows, area.height).min(bottom);
        let target = current.saturating_add_signed(delta).min(bottom);
        let previous = (self.anchor, self.anchor_part);
        let was_visible = self.scrollbar_visible();
        if bottom > 0 {
            self.scrollbar_until = self.elapsed.saturating_add(Duration::from_secs(2));
        }
        if target == bottom {
            self.follow();
        } else if let Some(row) = rows.get(target) {
            self.anchor = Some(row.source);
            self.anchor_part = row.part;
        }
        previous != (self.anchor, self.anchor_part) || was_visible != self.scrollbar_visible()
    }

    fn display_rows(&self, width: u16, color: bool) -> Vec<DisplayRow> {
        #[cfg(test)]
        self.formatting_passes.set(self.formatting_passes.get() + 1);
        let theme = Theme::new(self.preferences.effective(), color);
        let content_width = usize::from(width.saturating_sub(2)).clamp(1, 120);
        let mut rows = Vec::new();
        let mut fenced = false;
        for (offset, text) in self.lines.iter().enumerate() {
            let source = self.first_line + offset;
            if let Some((_, footer)) = self.footers.iter().find(|(line, _)| *line == source) {
                rows.push(DisplayRow {
                    source,
                    part: 0,
                    line: footer.render(width, theme, self.preferences.ascii),
                });
                continue;
            }
            let fragment = self
                .fragments
                .iter()
                .find(|fragment| source >= fragment.start && source < fragment.end);
            let message = self
                .messages
                .iter()
                .find(|(start, end, _)| source >= *start && source < *end);
            let identity = fragment.map_or_else(
                || message.map_or(source, |(start, _, _)| *start),
                |fragment| fragment.start,
            );
            if source == identity {
                fenced = false;
            }
            let selected =
                self.focus == Focus::Conversation && self.selected_fragment == Some(identity);
            let user = message.is_some_and(|(_, _, user)| *user);
            let prefix = if selected {
                if self.preferences.ascii { "| " } else { "▌ " }
            } else if user {
                if self.preferences.ascii { "| " } else { "│ " }
            } else {
                "  "
            };
            let mut background = theme.base;
            let spans = if let Some(fragment) = fragment {
                let relative = source - fragment.start;
                let body_end = fragment.end;
                if !fragment.expanded && relative > 10 && source < body_end {
                    if relative != 11 {
                        continue;
                    }
                    vec![Span::styled(
                        format!("  … {} more lines · Space to expand", body_end - source),
                        theme.muted,
                    )]
                } else if relative == 0 {
                    let icon = if fragment.complete {
                        if self.preferences.ascii {
                            "[v] "
                        } else {
                            "✓ "
                        }
                    } else if text.contains("Demo stopped") {
                        if self.preferences.ascii {
                            "[x] "
                        } else {
                            "× "
                        }
                    } else if self.preferences.ascii {
                        "[.] "
                    } else {
                        "● "
                    };
                    vec![
                        Span::styled(
                            icon,
                            (if fragment.complete {
                                theme.success
                            } else {
                                theme.muted
                            })
                            .remove_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        Span::styled(
                            text.clone(),
                            if color && self.preferences.effective().theme == "high-contrast" {
                                theme.accent
                            } else if color {
                                theme.base.fg(ratatui::style::Color::Rgb(0, 164, 255))
                            } else {
                                theme.base
                            },
                        ),
                    ]
                } else {
                    background = if source < body_end {
                        presentation::panel(theme, color)
                    } else {
                        theme.base
                    };
                    vec![Span::styled(
                        text.clone(),
                        if source < body_end {
                            background
                        } else {
                            theme.muted
                        },
                    )]
                }
            } else {
                let text = text
                    .split_once(" | ")
                    .map_or(text.as_str(), |(_, content)| content);
                presentation::markdown(text, &mut fenced, theme)
            };
            let spans = if self.preferences.ascii {
                spans
                    .into_iter()
                    .map(|mut span| {
                        span.content = span
                            .content
                            .replace('•', "-")
                            .replace('│', "|")
                            .replace('…', "...")
                            .replace('·', "|")
                            .into();
                        span
                    })
                    .collect()
            } else {
                spans
            };
            for (part, mut line) in presentation::wrap(spans, content_width)
                .into_iter()
                .enumerate()
            {
                let mut spans = vec![Span::styled(
                    prefix,
                    if user { theme.accent } else { theme.success },
                )];
                spans.append(&mut line.spans);
                rows.push(DisplayRow {
                    source,
                    part,
                    line: Line::from(spans).style(background),
                });
            }
        }
        if self.active_fragment.is_some() {
            rows.push(DisplayRow {
                source: self.next_line,
                part: 0,
                line: Line::default(),
            });
            rows.push(DisplayRow {
                source: self.next_line,
                part: 1,
                line: self.message_loading(color),
            });
        }
        rows
    }

    fn message_loading(&self, color: bool) -> Line<'static> {
        let settings = self.preferences.effective();
        let theme = Theme::new(settings, color);
        let moving =
            self.overlay.is_none() && settings.animation_fps > 0 && !settings.reduced_motion;
        let mut spans = vec![Span::styled("  ", theme.base)];
        spans.extend(
            crate::identity::message_wave(
                self.elapsed,
                moving,
                color,
                self.truecolor,
                self.preferences.ascii,
            )
            .spans,
        );
        spans.push(crate::identity::working_label(
            self.elapsed,
            moving,
            color,
            self.truecolor,
        ));
        if color && settings.theme == "high-contrast" {
            let mut wave = Line::from(spans.split_off(1));
            crate::identity::fire_colors(
                std::slice::from_mut(&mut wave),
                if moving { self.elapsed } else { Duration::ZERO },
                1.2,
                self.truecolor,
            );
            spans.extend(wave.spans);
        }
        let seconds = self.elapsed.saturating_sub(self.playback_started).as_secs();
        let timer = if seconds >= 60 {
            format!(" {}m{:02}s", seconds / 60, seconds % 60)
        } else {
            format!(" {seconds}s")
        };
        spans.push(Span::styled(timer, theme.muted));
        Line::from(spans).style(theme.base)
    }

    pub(crate) fn loading_timer_second(&self) -> Option<u64> {
        self.active_fragment
            .map(|_| self.elapsed.saturating_sub(self.playback_started).as_secs())
    }

    fn render_message_loading(&self, frame: &mut Frame, color: bool, loading: Option<Rect>) {
        if self.active_fragment.is_none() || self.overlay.is_some() {
            return;
        }
        if let Some(area) = loading.filter(|area| frame.area().intersection(*area) == *area) {
            let row = area.y;
            let theme = Theme::new(self.preferences.effective(), color);
            for column in area.x..area.right() {
                frame.buffer_mut()[(column, row)].reset();
                frame.buffer_mut()[(column, row)].set_style(theme.base);
            }
            frame.render_widget(
                Paragraph::new(self.message_loading(color)),
                Rect::new(area.x, row, area.width, 1),
            );
        }
    }

    pub fn task_state(&self) -> TaskState {
        self.snapshot
            .tasks
            .first()
            .map_or(TaskState::Pending, |task| task.state)
    }

    pub fn state_age(&self) -> Duration {
        self.elapsed.saturating_sub(self.changed_at)
    }

    pub fn set_snapshot(&mut self, snapshot: Snapshot) -> Result<(), &'static str> {
        Self::new(snapshot.clone())?;
        self.snapshot = snapshot;
        self.changed_at = self.elapsed;
        Ok(())
    }

    fn close_preview(&mut self) {
        self.preferences.cancel();
        if let Some(snapshot) = self.preview_snapshot.take() {
            self.snapshot = snapshot;
            self.changed_at = self.elapsed;
        }
        self.next_scene = false;
        self.editing_notification = false;
        self.notification_preview = false;
        let cursors: Vec<_> = self
            .notification_stack
            .retained()
            .iter()
            .map(|notice| notice.cursor())
            .collect();
        for cursor in cursors {
            self.notification_stack.dismiss(cursor);
        }
        self.overlay = None;
    }

    fn visual_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .preferences
            .entries()
            .into_iter()
            .filter(|(descriptor, _)| {
                self.overlay != Some(Overlay::Theme) || descriptor.key == "tui.theme"
            })
            .filter(|(descriptor, _)| {
                descriptor.key.contains(&self.filter.to_lowercase())
                    || descriptor
                        .description
                        .to_lowercase()
                        .contains(&self.filter.to_lowercase())
            })
            .map(|(descriptor, _)| descriptor.key)
            .collect();
        if self.overlay == Some(Overlay::Developer)
            && "notification test message".contains(&self.filter.to_lowercase())
        {
            keys.push("Notification test message".into());
        }
        if self.overlay == Some(Overlay::Developer)
            && "notification preview".contains(&self.filter.to_lowercase())
        {
            keys.push("Notification preview".into());
        }
        keys
    }

    pub(crate) fn advance_notifications(&mut self) -> bool {
        match self.notification_stack.advance(self.elapsed) {
            Ok(expired) => expired > 0,
            Err(_) => false,
        }
    }

    fn preview_notice(&mut self, repeat: bool) {
        if !repeat || self.notification_sequence == 0 {
            self.notification_sequence = self.notification_sequence.saturating_add(1);
        }
        let severity = match self.notification_sequence % 5 {
            0 => Severity::Approval,
            1 => Severity::Information,
            2 => Severity::Success,
            3 => Severity::Warning,
            _ => Severity::Error,
        };
        let cursor = fluzo_core::application::Cursor {
            epoch: 0,
            sequence: self.notification_sequence,
        };
        if self
            .notification_stack
            .receive(
                cursor,
                severity,
                Lifetime::Transient,
                "Synthetic notice. No agent work was executed.",
                self.elapsed,
            )
            .is_err()
        {
            self.status = "Notification preview unavailable; invalid clock or settings.".into();
        }
    }

    fn notification_area(&self, area: Rect) -> Rect {
        if !self.notification_preview
            || self.overlay.is_some()
            || area.width < 60
            || area.height < 16
        {
            return Rect::default();
        }
        let conversation = self.body_layout(area)[1];
        let top = conversation.y.max(area.y + 3);
        let width = conversation.width.min(48);
        Rect::new(
            conversation.right().saturating_sub(width),
            top,
            width,
            conversation.bottom().saturating_sub(top),
        )
    }

    fn notification_close_area(rect: Rect) -> Rect {
        Rect::new(rect.right().saturating_sub(4), rect.y, 3, 1)
    }

    fn render_notifications(&self, frame: &mut Frame, color: bool) {
        let area = self.notification_area(frame.area());
        let theme = Theme::new(self.preferences.effective(), color);
        for (index, notice) in self
            .notification_stack
            .visible(usize::from(area.height / 5))
            .enumerate()
        {
            let rect = Rect::new(area.x, area.y + index as u16 * 5, area.width, 5);
            let (label, style) = match notice.severity() {
                Severity::Information => ("INFO", theme.accent),
                Severity::Success => ("SUCCESS", theme.success),
                Severity::Warning => ("WARNING", theme.warning),
                Severity::Error => ("ERROR", theme.error),
                Severity::Approval => ("APPROVAL", theme.warning),
            };
            let border_style = match notice.severity() {
                Severity::Error => theme.error,
                Severity::Warning => theme.warning,
                _ => theme.accent,
            };
            self.render_dialog_frame(
                frame,
                rect,
                Theme {
                    accent: border_style,
                    ..theme
                },
            );
            frame.render_widget(
                Paragraph::new(" x ").style(border_style),
                Self::notification_close_area(rect),
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(format!("[DEMO {label}]"), style)),
                    Line::from(notice.text()),
                    Line::from(Span::styled("Alt+D dismiss oldest", theme.muted)),
                ])
                .style(theme.base),
                Rect::new(rect.x + 2, rect.y + 1, rect.width.saturating_sub(4), 3),
            );
        }
    }

    pub(crate) fn notification_text(&self) -> &str {
        &self.notification_editor.text
    }

    fn append(&mut self, origin: &str, text: &str) {
        let start = self.next_line;
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
        self.messages.push_back((
            start.max(self.first_line),
            self.next_line,
            origin == "User preview",
        ));
    }

    fn add_line(&mut self, line: String) {
        self.lines.push_back(line);
        self.next_line += 1;
        if self.lines.len() > HISTORY_LIMIT {
            self.lines.pop_front();
            self.first_line += 1;
            self.dropped = true;
        }
        while self
            .footers
            .front()
            .is_some_and(|(line, _)| *line < self.first_line)
        {
            self.footers.pop_front();
        }
        while self
            .fragments
            .front()
            .is_some_and(|fragment| fragment.end <= self.first_line)
        {
            self.fragments.pop_front();
        }
        while self
            .messages
            .front()
            .is_some_and(|(_, end, _)| *end <= self.first_line)
        {
            self.messages.pop_front();
        }
        if self
            .selected_fragment
            .is_some_and(|selected| selected < self.first_line)
        {
            self.selected_fragment = None;
        }
        if let Some(anchor) = self.anchor.as_mut() {
            *anchor = (*anchor).max(self.first_line);
            self.unread = self.unread.saturating_add(1);
        }
    }

    pub(crate) fn finish_playback(&mut self) {
        if self.active_fragment.is_some() {
            self.synthetic_fragment_tick(0);
        }
        self.playing = false;
        self.status = "Synthetic playback ended; no runtime completion implied.".into();
    }

    pub fn synthetic_fragment_tick(&mut self, index: usize) {
        if let Some(start) = self.active_fragment.take() {
            if let Some(fragment) = self
                .fragments
                .iter_mut()
                .find(|fragment| fragment.start == start)
            {
                fragment.complete = true;
                fragment.end = self.next_line;
            }
            if let Some(title) = start
                .checked_sub(self.first_line)
                .and_then(|offset| self.lines.get_mut(offset))
            {
                *title = title.replace("[Demo running]", "[Demo complete]");
            }
            self.add_line(String::new());
            self.footers.push_back((
                self.next_line,
                presentation::AssistantFooter {
                    model: "GTP-6 Astra".into(),
                    provider: "Github Copilot".into(),
                    duration: "1m21s".into(),
                },
            ));
            self.add_line(String::new());
            self.add_line(String::new());
            return;
        }
        let actions = [
            (
                "Inspect project structure",
                "Read source layout and entry points.",
                "Identify files relevant to the request.",
            ),
            (
                "Review the proposed change",
                "Compare behavior with the contract.",
                "Keep edits focused on the requested task.",
            ),
            (
                "Verify the result",
                "Check the intended assertions.",
                "Report results without hiding failures.",
            ),
        ];
        let (title, first, second) = actions[(index / 2) % actions.len()];
        let detailed = index / 2 % actions.len() == 1;
        if detailed {
            self.append("Preview", "\n## Proposed change\nKeep **ownership** explicit and preserve the user's draft during resize.\n\n- Reflow messages to the available width.\n- Keep `Escape` and cancellation responsive.\n\n```diff\n- let width = 80;\n+ let width = viewport.width;\n```\n\nThis is a visual example, not an executed edit.\n");
        }
        let start = self.next_line;
        self.fragments.push_back(Fragment {
            start,
            end: start + if detailed { 18 } else { 4 },
            complete: false,
            expanded: false,
        });
        self.active_fragment = Some(start);
        self.add_line(format!("{title} [Demo running]"));
        self.add_line(format!("  Synthetic fragment {}", index / 2 + 1));
        self.add_line(format!("  {first}"));
        self.add_line(format!("  {second}"));
        if detailed {
            for number in 1..=14 {
                self.add_line(format!(
                    "  {number:>2}  synthetic output: bounded presentation, no tool execution"
                ));
            }
        }
    }

    fn stop_fragment(&mut self) {
        if let Some(start) = self.active_fragment.take()
            && let Some(title) = start
                .checked_sub(self.first_line)
                .and_then(|offset| self.lines.get_mut(offset))
        {
            *title = title.replace("[Demo running]", "[Demo stopped]");
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
        if self.editing_notification {
            let safe = crate::notification::safe_test_text(text);
            if text.len() <= 4096 && self.notification_editor.text.len() + safe.len() <= 256 {
                self.notification_editor.insert(&safe);
            } else {
                self.status = "Notification text limit: 256 bytes; input preserved.".into();
            }
            return;
        }
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
                if index == 6 && !self.preferences.effective().dev_menu {
                    return None;
                }
                action
                    .to_lowercase()
                    .contains(&self.filter.to_lowercase())
                    .then_some(index)
            })
            .collect()
    }

    fn follow(&mut self) {
        self.scrollbar_until = self.elapsed.saturating_add(Duration::from_secs(2));
        self.anchor = None;
        self.anchor_part = 0;
        self.unread = 0;
    }

    fn action(&mut self, index: usize) {
        self.close_preview();
        match index {
            0 => {
                if !self.playing {
                    self.playback_started = self.elapsed;
                }
                self.playing = true;
                self.status = "Synthetic playback; no task is running.".into();
            }
            1 => {
                self.stop_fragment();
                self.playing = false;
                self.status = "Preview stopped; draft and partial output preserved.".into();
            }
            2 => self.follow(),
            3 => {
                self.help_page = 0;
                self.overlay = Some(Overlay::Help);
            }
            4 => self.request_quit(),
            5 | 6 => {
                if index == 6 && !self.preferences.effective().dev_menu {
                    return;
                }
                self.preferences.begin();
                self.preview_snapshot = Some(self.snapshot.clone());
                self.filter.clear();
                self.selection = 0;
                self.overlay = Some(if index == 5 {
                    Overlay::Theme
                } else {
                    Overlay::Developer
                });
            }
            _ => {}
        }
    }

    fn request_quit(&mut self) {
        self.close_preview();
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
        let menu_navigation = matches!(self.overlay, Some(Overlay::Theme | Overlay::Developer))
            && matches!(
                key.code,
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
            );
        if key.kind == KeyEventKind::Repeat
            && (control
                || (self.overlay.is_some() && !menu_navigation)
                || key.code == KeyCode::Enter)
        {
            return;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
            self.request_quit();
            return;
        }
        if self.notification_preview && self.overlay.is_none() {
            if key.code == KeyCode::Esc {
                self.close_preview();
                self.status = "Notification preview closed; unapplied preferences reverted.".into();
                return;
            }
            if key.modifiers == KeyModifiers::ALT {
                match key.code {
                    KeyCode::Char('n') => self.preview_notice(false),
                    KeyCode::Char('b') => {
                        for _ in 0..40 {
                            self.preview_notice(false);
                        }
                    }
                    KeyCode::Char('r') => self.preview_notice(true),
                    KeyCode::Char('d') => {
                        if let Some(cursor) = self
                            .notification_stack
                            .retained()
                            .first()
                            .map(|notice| notice.cursor())
                        {
                            self.notification_stack.dismiss(cursor);
                        }
                    }
                    _ => {}
                }
                return;
            }
            if control && key.code == KeyCode::Char('a') {
                self.preferences.apply();
                self.close_preview();
                self.status =
                    "Notification preview applied for session only; no file saved.".into();
                return;
            }
            if control && key.code == KeyCode::Char('r') {
                self.preferences.reset();
                return;
            }
        }
        if self.editing_notification {
            match key.code {
                KeyCode::Esc => self.editing_notification = false,
                KeyCode::Enter if !control => {
                    if !self.notification_editor.text.trim().is_empty() {
                        self.notification_test = true;
                        self.editing_notification = false;
                    }
                }
                KeyCode::Char('u') if control => self.notification_editor = Editor::default(),
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    self.paste(&character.to_string());
                }
                code if !control => self.notification_editor.key(code),
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Esc {
            if self.overlay.is_some() {
                self.close_preview();
            } else if self.playing || self.active_fragment.is_some() {
                self.action(1);
            } else {
                self.selected_fragment = None;
            }
            return;
        }
        if matches!(self.overlay, Some(Overlay::Theme | Overlay::Developer)) {
            if control {
                match key.code {
                    KeyCode::Char('a') => {
                        self.preferences.apply();
                        self.status = format!(
                            "UI settings v{} applied for session only; no file saved.",
                            self.preferences.version
                        );
                        self.close_preview();
                    }
                    KeyCode::Char('r') => {
                        if self.overlay == Some(Overlay::Theme) {
                            self.preferences.reset_theme();
                        } else {
                            self.preferences.reset();
                        }
                    }
                    KeyCode::Char('e') if self.overlay == Some(Overlay::Developer) => {
                        self.editing_notification = true;
                        self.notification_editor.cursor = self.notification_editor.text.len();
                    }
                    KeyCode::Char('u') => {
                        self.filter.clear();
                        self.selection = 0;
                    }
                    KeyCode::Char('n') if self.overlay == Some(Overlay::Developer) => {
                        self.next_scene = true
                    }
                    KeyCode::Char('t') if self.overlay == Some(Overlay::Developer) => {
                        self.notification_test = true;
                    }
                    _ => {}
                }
                return;
            }
            match key.code {
                KeyCode::Up | KeyCode::BackTab => self.selection = self.selection.saturating_sub(1),
                KeyCode::Home => self.selection = 0,
                KeyCode::End => self.selection = self.visual_keys().len().saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => {
                    self.selection =
                        (self.selection + 1).min(self.visual_keys().len().saturating_sub(1))
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
                    if let Some(selected) = self.visual_keys().get(self.selection) {
                        if selected == "Notification preview" {
                            if key.code == KeyCode::Enter
                                && self
                                    .notification_stack
                                    .configure(self.preferences.effective().notifications.clone())
                                    .is_ok()
                            {
                                self.notification_preview = true;
                                self.overlay = None;
                                self.status = "DEMO notices: Alt+N add | Alt+B burst | Alt+R replay | Alt+D dismiss".into();
                            }
                            return;
                        }
                        if selected == "tui.notifications.desktop_enabled"
                            && !self.notification_transport_supported
                            && self.preferences.source(selected)
                                != fluzo_core::settings::SettingOrigin::CommandLine
                        {
                            self.status =
                                "Notifications unavailable: this preview requires Ghostty.".into();
                            return;
                        }
                        if selected == "Notification test message" {
                            if key.code == KeyCode::Enter {
                                self.editing_notification = true;
                                self.notification_editor.cursor =
                                    self.notification_editor.text.len();
                            }
                            return;
                        }
                        if let Err(error) = self
                            .preferences
                            .change(selected, if key.code == KeyCode::Left { -1 } else { 1 })
                        {
                            if self.preferences.source(selected)
                                != fluzo_core::settings::SettingOrigin::CommandLine
                            {
                                self.status = error.into();
                            }
                        } else {
                            self.status =
                                "Temporary preview; Esc reverts, Ctrl+A applies for session."
                                    .into();
                        }
                    }
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.selection = 0;
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::ALT)
                        && !character.is_control()
                        && self.filter.len() < 64 =>
                {
                    self.filter.push(character);
                    self.selection = 0;
                }
                _ => {}
            }
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
            if control && key.code == KeyCode::Char('g') {
                self.close_preview();
            } else {
                let pages = HELP_ROWS.len().div_ceil(Self::help_capacity(self.viewport));
                match key.code {
                    KeyCode::Right | KeyCode::Down | KeyCode::PageDown | KeyCode::Tab => {
                        self.help_page = (self.help_page + 1).min(pages - 1);
                    }
                    KeyCode::Left | KeyCode::Up | KeyCode::PageUp | KeyCode::BackTab => {
                        self.help_page = self.help_page.saturating_sub(1);
                    }
                    KeyCode::Home => self.help_page = 0,
                    KeyCode::End => self.help_page = pages - 1,
                    _ => {}
                }
            }
            return;
        }
        if matches!(
            self.overlay,
            Some(Overlay::Models | Overlay::Sessions | Overlay::Details)
        ) {
            if self.overlay == Some(Overlay::Details) && control && key.code == KeyCode::Char('d') {
                self.close_preview();
            }
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
        if control && key.code == KeyCode::End {
            self.follow();
            return;
        }
        if control
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(
                key.code,
                KeyCode::Char('a' | 'A' | 'c' | 'C' | 'x' | 'X' | 'v' | 'V')
            )
        {
            self.status =
                "Text selection and clipboard shortcuts are unavailable; draft preserved.".into();
            return;
        }
        if control {
            match key.code {
                KeyCode::Char('l' | 'm') => {
                    self.overlay = Some(Overlay::Models);
                    return;
                }
                KeyCode::Char('s') => {
                    self.overlay = Some(Overlay::Sessions);
                    return;
                }
                KeyCode::Char('d') => {
                    self.overlay = Some(Overlay::Details);
                    return;
                }
                KeyCode::Char('b') => {
                    if !presentation::compact(self.viewport) {
                        self.hide_sidebar = !self.hide_sidebar;
                    }
                    return;
                }
                KeyCode::Char('n') => {
                    self.status =
                        "New sessions are unavailable in this read-only demo; draft preserved."
                            .into();
                    return;
                }
                KeyCode::Char('y') => {
                    self.status = "YOLO is unavailable; demo remains NORMAL.".into();
                    return;
                }
                KeyCode::Char('z') => {
                    self.status = "Suspend is unavailable; Ctrl+C exits safely.".into();
                    return;
                }
                KeyCode::Char('o') => {
                    self.status = "External editor is unavailable; draft preserved.".into();
                    return;
                }
                KeyCode::Char('f' | 'r' | 'v') => {
                    self.status =
                        "Attachments and clipboard are unavailable; use terminal paste.".into();
                    return;
                }
                KeyCode::Char('t' | ' ') | KeyCode::Null => {
                    self.status = "Task panel is unavailable; no runtime connected.".into();
                    return;
                }
                _ => {}
            }
        }
        if key.code == KeyCode::BackTab {
            self.status = "Plan/YOLO modes are unavailable; demo remains NORMAL.".into();
            return;
        }
        if (control && key.code == KeyCode::Char('p'))
            || (self.focus == Focus::Composer
                && self.draft.text.is_empty()
                && key.code == KeyCode::Char('/')
                && key.modifiers.is_empty())
        {
            self.overlay = Some(Overlay::Palette);
            self.filter.clear();
            self.selection = 0;
            return;
        }
        if control && key.code == KeyCode::Char('g') {
            self.help_page = 0;
            self.overlay = Some(Overlay::Help);
            return;
        }
        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
            if self.focus == Focus::Composer {
                self.focus = Focus::Conversation;
                self.selected_fragment = self
                    .display_rows(self.body_layout(self.viewport)[1].width, false)
                    .iter()
                    .rev()
                    .find_map(|row| {
                        self.fragments
                            .iter()
                            .find(|fragment| {
                                row.source >= fragment.start && row.source < fragment.end
                            })
                            .map(|fragment| fragment.start)
                            .or_else(|| {
                                self.messages
                                    .iter()
                                    .find(|(start, end, _)| {
                                        row.source >= *start && row.source < *end
                                    })
                                    .map(|(start, _, _)| *start)
                            })
                    });
                self.follow();
            } else {
                self.focus = Focus::Composer;
            }
            return;
        }
        if self.focus == Focus::Composer {
            if (control && key.code == KeyCode::Char('j'))
                || (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT))
            {
                self.paste("\n");
                return;
            }
            if control {
                match key.code {
                    KeyCode::Char('a') => self.draft.key(KeyCode::Home),
                    KeyCode::Char('e') => self.draft.key(KeyCode::End),
                    KeyCode::Char('h') => self.draft.key(KeyCode::Backspace),
                    KeyCode::Char('u') => {
                        let end = self.draft.cursor;
                        self.draft.key(KeyCode::Home);
                        self.draft.text.drain(self.draft.cursor..end);
                    }
                    KeyCode::Char('k') => {
                        let start = self.draft.cursor;
                        self.draft.key(KeyCode::End);
                        self.draft.text.drain(start..self.draft.cursor);
                        self.draft.cursor = start;
                    }
                    _ => {}
                }
                return;
            }
            if matches!(key.code, KeyCode::Up | KeyCode::Down)
                && !self.draft.text.contains('\n')
                && !self.input_history.is_empty()
            {
                let position = self.history_position.unwrap_or(self.input_history.len());
                if self.history_position.is_none() {
                    self.history_draft = self.draft.text.clone();
                }
                let target = if key.code == KeyCode::Up {
                    position.saturating_sub(1)
                } else {
                    (position + 1).min(self.input_history.len())
                };
                self.history_position = (target < self.input_history.len()).then_some(target);
                self.draft.text = self
                    .input_history
                    .get(target)
                    .unwrap_or(&self.history_draft)
                    .clone();
                self.draft.cursor = self.draft.text.len();
                return;
            }
        }
        if self.focus == Focus::Conversation {
            let height = isize::try_from(self.body_layout(self.viewport)[1].height)
                .unwrap_or(1)
                .max(1);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let delta = match key.code {
                KeyCode::Up if !shift => Some(-1),
                KeyCode::Down if !shift => Some(1),
                KeyCode::Char('j') => Some(1),
                KeyCode::Char('k') => Some(-1),
                KeyCode::PageUp | KeyCode::Char('b') if !control => Some(-height),
                KeyCode::PageDown | KeyCode::Char('f') if !control => Some(height),
                KeyCode::Char('u') if !control => Some(-height / 2),
                KeyCode::Char('d') if !control => Some(height / 2),
                _ => None,
            };
            if let Some(delta) = delta {
                self.scroll_rows(delta);
                return;
            }
            match key.code {
                KeyCode::Home | KeyCode::Char('g') => {
                    self.scrollbar_until = self.elapsed.saturating_add(Duration::from_secs(2));
                    self.anchor = Some(self.first_line);
                    self.anchor_part = 0;
                    return;
                }
                KeyCode::End | KeyCode::Char('G') => {
                    self.follow();
                    return;
                }
                KeyCode::Char('c' | 'y' | 'C' | 'Y') => {
                    self.status = "Clipboard copy is unavailable; use terminal selection.".into();
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Char('J' | 'K')
                if self.focus == Focus::Conversation =>
            {
                let identities = self
                    .fragments
                    .iter()
                    .map(|fragment| fragment.start)
                    .chain(self.messages.iter().map(|(start, _, _)| *start))
                    .filter(|start| *start >= self.first_line);
                let next = if matches!(key.code, KeyCode::Down | KeyCode::Char('J')) {
                    identities
                        .filter(|start| {
                            self.selected_fragment
                                .is_none_or(|selected| *start > selected)
                        })
                        .min()
                } else {
                    identities
                        .filter(|start| {
                            self.selected_fragment
                                .is_none_or(|selected| *start < selected)
                        })
                        .max()
                };
                if let Some(start) = next {
                    self.scrollbar_until = self.elapsed.saturating_add(Duration::from_secs(2));
                    self.selected_fragment = Some(start);
                    self.anchor = Some(start);
                    self.anchor_part = 0;
                }
            }
            KeyCode::PageUp => {
                self.scroll_rows(
                    -isize::try_from(self.body_layout(self.viewport)[1].height).unwrap_or(1),
                );
            }
            KeyCode::PageDown => {
                self.scroll_rows(
                    isize::try_from(self.body_layout(self.viewport)[1].height).unwrap_or(1),
                );
            }
            KeyCode::End if self.focus == Focus::Conversation => self.follow(),
            KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l' | 'H' | 'L')
                if self.focus == Focus::Conversation =>
            {
                self.status =
                    "Sidebar focus and horizontal scrolling are unavailable in this demo.".into();
            }
            KeyCode::Enter if self.focus == Focus::Composer => {
                if !self.draft.text.trim().is_empty() {
                    self.stop_fragment();
                    self.append("User preview", &self.draft.text.clone());
                    if self.input_history.back() != Some(&self.draft.text) {
                        self.input_history.push_back(self.draft.text.clone());
                        if self.input_history.len() > 32 {
                            self.input_history.pop_front();
                        }
                    }
                    self.history_position = None;
                    self.status =
                        "Draft previewed only; retained for editing. No command dispatched.".into();
                }
            }
            KeyCode::Char(' ') if self.focus == Focus::Conversation => {
                if let Some(fragment) = self
                    .fragments
                    .iter_mut()
                    .find(|fragment| Some(fragment.start) == self.selected_fragment)
                {
                    fragment.expanded = !fragment.expanded;
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
        self.render_frame(frame, color);
    }

    pub(crate) fn render_frame(&self, frame: &mut Frame, color: bool) -> RenderGeometry {
        let area = frame.area();
        let mut geometry = RenderGeometry {
            area,
            loading: None,
        };
        let theme = Theme::new(self.preferences.effective(), color);
        frame.render_widget(Block::default().style(theme.base), area);
        if area.width < 60 || area.height < 16 {
            frame.render_widget(Paragraph::new("Fluzo demo: resize required.\nMinimum: 60x16\nCtrl+C exits; draft kept.\nTab, Enter: discard and exit."), area);
            crate::visual::terminal_colors(frame.buffer_mut(), color, self.truecolor);
            return geometry;
        }
        let layout = self.body_layout(area);
        self.render_sidebar(frame, color);
        let rows = self.display_rows(layout[1].width, color);
        let start = self.display_start(&rows, layout[1].height);
        for (offset, row) in rows
            .iter()
            .skip(start)
            .take(usize::from(layout[1].height))
            .enumerate()
        {
            frame.render_widget(
                Paragraph::new(row.line.clone()),
                Rect::new(layout[1].x, layout[1].y + offset as u16, layout[1].width, 1),
            );
            if row.source == self.next_line && row.part == 1 && self.overlay.is_none() {
                geometry.loading = Some(Rect::new(
                    layout[1].x,
                    layout[1].y + offset as u16,
                    layout[1].width,
                    1,
                ));
            }
        }
        if self.scrollbar_visible() && rows.len() > usize::from(layout[1].height) {
            let visible = usize::from(layout[1].height);
            let maximum = rows.len() - visible;
            let length = (visible * visible / rows.len()).max(1);
            let offset = start.min(maximum) * (visible - length) / maximum;
            for row in 0..visible {
                let thumb = row >= offset && row < offset + length;
                let style = if theme.base.bg == Some(ratatui::style::Color::Rgb(32, 31, 38)) {
                    theme.base.fg(if thumb {
                        ratatui::style::Color::Rgb(255, 96, 255)
                    } else {
                        ratatui::style::Color::Rgb(58, 57, 67)
                    })
                } else if thumb {
                    theme.accent
                } else {
                    theme.muted
                };
                frame.buffer_mut()[(layout[1].right(), layout[1].y + row as u16)]
                    .set_symbol(match (thumb, self.preferences.ascii) {
                        (true, false) => "┃",
                        (false, false) => "│",
                        (true, true) => "#",
                        (false, true) => "|",
                    })
                    .set_style(style.remove_modifier(ratatui::style::Modifier::BOLD));
            }
        }
        let width = usize::from(layout[2].width.saturating_sub(4));
        let (draft_rows, row, _) = presentation::editor(&self.draft.text, self.draft.cursor, width);
        let visible = usize::from(layout[2].height.saturating_sub(2));
        let top = row.saturating_sub(visible.saturating_sub(1));
        for offset in 0..visible {
            let prompt = if offset + top == 0 && self.focus == Focus::Composer {
                "  > "
            } else {
                "::: "
            };
            let text = draft_rows.get(top + offset).cloned().unwrap_or_default();
            let text = if self.draft.text.is_empty() && offset == 0 {
                if self.snapshot.demo {
                    "Ask anything… (offline demo)"
                } else {
                    "Draft only; execution unavailable"
                }
                .into()
            } else {
                text
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        prompt,
                        if self.focus == Focus::Composer {
                            theme.success
                        } else {
                            theme.muted
                        },
                    ),
                    Span::styled(
                        text,
                        if self.draft.text.is_empty() || self.focus != Focus::Composer {
                            theme.muted
                        } else {
                            theme.base
                        },
                    ),
                ])),
                Rect::new(
                    layout[2].x,
                    layout[2].y + 1 + offset as u16,
                    layout[2].width,
                    1,
                ),
            );
        }
        self.render_cursor(frame);
        self.render_footer(frame, layout[3], theme);
        if self.status != "Ready. No agent, tools or providers are connected." {
            frame.render_widget(
                Paragraph::new(self.status.as_str()).style(theme.muted),
                Rect::new(layout[2].x, layout[2].y, layout[2].width, 1),
            );
        }
        self.render_identity(frame, color, geometry);
        self.render_overlay(frame, color);
        self.render_diagnostics(frame, color);
        crate::visual::terminal_colors(frame.buffer_mut(), color, self.truecolor);
        geometry
    }

    fn overlay_area(&self, area: Rect) -> Rect {
        if self.overlay == Some(Overlay::Details) && self.compact(area) {
            return Rect::new(
                area.x + 1,
                area.y + 2,
                area.width.saturating_sub(2),
                area.height.saturating_sub(4).min(20),
            );
        }
        let (width, height) = if self.overlay == Some(Overlay::Quit) {
            (
                50.min(area.width.saturating_sub(4)),
                10.min(area.height.saturating_sub(4)),
            )
        } else {
            (
                area.width.saturating_sub(4).min(70),
                area.height.saturating_sub(4).min(20),
            )
        };
        Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        )
    }

    fn render_overlay(&self, frame: &mut Frame, color: bool) {
        if let Some(overlay) = self.overlay {
            let theme = Theme::new(self.preferences.effective(), color);
            let popup = self.overlay_area(frame.area());
            let width = popup.width;
            if self.editing_notification {
                self.render_notification_editor(frame, popup, color);
                return;
            }
            if overlay == Overlay::Help {
                self.render_help(frame, popup, color);
                return;
            }
            if overlay == Overlay::Quit {
                self.render_quit(frame, popup, theme);
                return;
            }
            if overlay == Overlay::Details && self.compact(frame.area()) {
                self.render_dialog_frame(frame, popup, theme);
                let mut lines = vec![Line::from("Demo session"), Line::default()];
                lines.extend(presentation::demo_model_info(
                    popup.width - 4,
                    theme,
                    self.preferences.ascii,
                ));
                lines.extend([
                    Line::default(),
                    Line::from("No runtime connected. Ctrl+D / Esc closes."),
                ]);
                frame.render_widget(
                    Paragraph::new(lines).style(theme.base),
                    Rect::new(popup.x + 2, popup.y + 1, popup.width - 4, popup.height - 2),
                );
                return;
            }
            let searching = matches!(
                overlay,
                Overlay::Palette | Overlay::Theme | Overlay::Developer
            );
            let roomy = popup.height >= 16;
            let body_y = popup.y
                + if searching {
                    if roomy { 5 } else { 3 }
                } else {
                    3
                };
            let footer_rows = if overlay == Overlay::Developer { 4 } else { 1 };
            let footer_y = popup.bottom() - 1 - footer_rows;
            let body_height = footer_y.saturating_sub(body_y);
            let (title, lines) = match overlay {
                Overlay::Palette => {
                    let mut lines = Vec::new();
                    let actions = self.actions();
                    if actions.is_empty() {
                        lines.push(Line::from("No matching actions"));
                    }
                    for (position, index) in actions.iter().enumerate() {
                        let selected = position == self.selection;
                        let style = if selected && color {
                            theme
                                .base
                                .bg(theme.accent.fg.unwrap_or(ratatui::style::Color::Magenta))
                                .fg(if self.preferences.effective().theme == "high-contrast" {
                                    ratatui::style::Color::Black
                                } else {
                                    ratatui::style::Color::Rgb(255, 250, 241)
                                })
                        } else if selected {
                            theme.accent
                        } else {
                            theme.base
                        };
                        lines.push(Line::from(Span::styled(
                            format!(
                                "{} {:<padding$}",
                                if selected && !color { ">" } else { " " },
                                ACTIONS[*index],
                                padding = usize::from(width.saturating_sub(4))
                            ),
                            style,
                        )));
                    }
                    ("Commands", lines)
                }
                Overlay::Theme | Overlay::Developer => {
                    let keys = self.visual_keys();
                    let mut lines = Vec::new();
                    let entries = self.preferences.entries();
                    let visible = usize::from(body_height.saturating_sub(3))
                        .max(1)
                        .min(keys.len());
                    let first = self.selection.saturating_sub(visible.saturating_sub(1));
                    for (index, key) in keys.iter().enumerate().skip(first).take(visible) {
                        let value = entries
                            .iter()
                            .find(|(descriptor, _)| descriptor.key == *key)
                            .map_or_else(
                                || "Enter to edit".into(),
                                |(_, value)| display_value(value),
                            );
                        lines.push(Line::from(Span::styled(
                            format!(
                                "{} {}: {}",
                                if index == self.selection { ">" } else { " " },
                                key,
                                value
                            ),
                            if index == self.selection {
                                theme.accent
                            } else {
                                theme.muted
                            },
                        )));
                    }
                    if let Some(key) = keys.get(self.selection) {
                        if let Some((descriptor, value)) = self
                            .preferences
                            .entries()
                            .into_iter()
                            .find(|(descriptor, _)| &descriptor.key == key)
                        {
                            lines.push(Line::from(format!(
                                "{} | default {} | {:?}",
                                display_value(&value),
                                display_value(&descriptor.default),
                                self.preferences.source(key)
                            )));
                            lines.push(Line::from(Span::styled(
                                descriptor.description,
                                theme.muted,
                            )));
                            lines.push(Line::from(
                                if self.preferences.source(key)
                                    == fluzo_core::settings::SettingOrigin::CommandLine
                                {
                                    "Locked by command line; preview cannot override it."
                                } else if key == "tui.notifications.desktop_enabled"
                                    && !self.notification_transport_supported
                                {
                                    "Unavailable: Ghostty transport required"
                                } else if key == "tui.notifications.desktop_enabled" {
                                    "Session Apply | Left/Right edit | Up/Down/Tab select"
                                } else {
                                    "Live | Left/Right edit | Up/Down/Tab select"
                                },
                            ));
                        } else if key == "Notification preview" {
                            lines.push(Line::from(
                                "Enter opens an empty in-app preview; no external send.",
                            ));
                            lines.push(Line::from(
                                "Alt+N add | Alt+B burst | Alt+R replay | Alt+D dismiss",
                            ));
                            lines.push(Line::from("Ctrl+A apply | Ctrl+R reset | Esc revert"));
                        } else {
                            lines.push(Line::from("Synthetic text only; do not enter secrets."));
                            lines.push(Line::from("Enter edits | Ctrl+T sends after 3 seconds"));
                            lines.push(Line::from(
                                "Desktop opt-in required; blur window to receive.",
                            ));
                        }
                    } else {
                        lines.push(Line::from("No matching settings"));
                    }
                    lines.push(Line::from(
                        "Save .fluzo: unavailable in this read-only phase",
                    ));
                    if overlay == Overlay::Developer {
                        lines.push(Line::from("Laya: unavailable; no consent/capacity adapter"));
                    }
                    (
                        if overlay == Overlay::Theme {
                            "Theme preview"
                        } else {
                            "Developer menu"
                        },
                        lines,
                    )
                }
                Overlay::Models => (
                    "Models | offline demo",
                    vec![
                        Line::from("GTP-6 Astra (ExtraHigh)"),
                        Line::from("via Github Copilot"),
                        Line::from("Example only; model selection is not implemented."),
                        Line::from("Esc returns; no provider connection."),
                    ],
                ),
                Overlay::Sessions => (
                    "Sessions | offline demo",
                    vec![
                        Line::from("Demo session"),
                        Line::from("Persistent sessions are not implemented."),
                        Line::from("Esc returns; current draft is preserved."),
                    ],
                ),
                Overlay::Details => {
                    let mut lines =
                        presentation::demo_model_info(width - 4, theme, self.preferences.ascii);
                    lines.push(Line::from("No runtime connected. Ctrl+D / Esc closes."));
                    ("Session details | offline demo", lines)
                }
                Overlay::Help | Overlay::Quit => return,
            };
            self.render_dialog_frame(frame, popup, theme);
            let inner = Rect::new(popup.x + 2, popup.y + 1, width - 4, 1);
            self.render_dialog_title(frame, inner, title, color);
            if searching {
                let (text, _) = self.dialog_search(inner.width);
                let search = if self.filter.is_empty() {
                    Line::from(vec![
                        Span::styled("> ", theme.accent),
                        Span::styled(
                            if overlay == Overlay::Palette {
                                "Search commands..."
                            } else {
                                "Search settings..."
                            },
                            theme.muted,
                        ),
                    ])
                } else {
                    Line::from(vec![Span::styled("> ", theme.accent), Span::raw(text)])
                };
                frame.render_widget(
                    Paragraph::new(search).style(theme.base),
                    Rect::new(inner.x, popup.y + if roomy { 3 } else { 2 }, inner.width, 1),
                );
            }
            let body = if overlay == Overlay::Palette {
                Rect::new(popup.x + 1, body_y, width - 2, body_height)
            } else {
                Rect::new(inner.x, body_y, inner.width, body_height)
            };
            let start = if overlay == Overlay::Palette {
                self.selection
                    .saturating_sub(usize::from(body.height).saturating_sub(1))
            } else {
                0
            };
            frame.render_widget(
                Paragraph::new(lines.into_iter().skip(start).collect::<Vec<_>>()).style(theme.base),
                body,
            );
            let footer = match overlay {
                Overlay::Palette => "Up/Down select  Enter run  Esc close",
                Overlay::Theme | Overlay::Developer => {
                    "Ctrl+A Apply session | Ctrl+R Reset | Esc Revert"
                }
                _ => "Esc close",
            };
            frame.render_widget(
                Paragraph::new(footer).style(theme.muted),
                Rect::new(inner.x, footer_y, inner.width, 1),
            );
            if overlay == Overlay::Developer {
                frame.render_widget(
                    Paragraph::new("Ctrl+N State | Ctrl+T Notification test | Ctrl+E Text")
                        .style(theme.muted),
                    Rect::new(inner.x, footer_y + 1, inner.width, 1),
                );
                let feedback = if self.status.starts_with("Notification") {
                    self.status.as_str()
                } else {
                    "Desktop test: switch to another window before 3s; focused windows suppress delivery."
                };
                frame.render_widget(
                    Paragraph::new(feedback)
                        .style(theme.warning)
                        .wrap(ratatui::widgets::Wrap { trim: false }),
                    Rect::new(inner.x, footer_y + 2, inner.width, 2),
                );
            }
            self.render_cursor(frame);
        }
    }

    fn render_notification_editor(&self, frame: &mut Frame, popup: Rect, color: bool) {
        let theme = Theme::new(self.preferences.effective(), color);
        self.render_dialog_frame(frame, popup, theme);
        self.render_dialog_title(
            frame,
            Rect::new(popup.x + 2, popup.y + 1, popup.width - 4, 1),
            "Notification test text",
            color,
        );
        frame.render_widget(
            Paragraph::new("Synthetic text only. After Enter, switch windows.").style(theme.muted),
            Rect::new(popup.x + 2, popup.y + 2, popup.width - 4, 1),
        );
        let width = usize::from(popup.width - 4);
        let (rows, row, _) = presentation::editor(
            &self.notification_editor.text,
            self.notification_editor.cursor,
            width,
        );
        let height = usize::from(popup.height.saturating_sub(6));
        let top = row.saturating_sub(height.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(
                rows.into_iter()
                    .skip(top)
                    .map(Line::from)
                    .collect::<Vec<_>>(),
            )
            .style(theme.base),
            Rect::new(popup.x + 2, popup.y + 4, popup.width - 4, height as u16),
        );
        frame.render_widget(
            Paragraph::new("Enter send in 3s | Ctrl+U clear | Esc back").style(theme.muted),
            Rect::new(popup.x + 2, popup.bottom() - 2, popup.width - 4, 1),
        );
        self.render_cursor(frame);
    }

    fn render_dialog_title(&self, frame: &mut Frame, area: Rect, title: &str, color: bool) {
        let theme = Theme::new(self.preferences.effective(), color);
        let accent = theme.accent.remove_modifier(ratatui::style::Modifier::BOLD);
        let mut spans = vec![Span::styled(title.to_owned(), accent), Span::raw(" ")];
        let fill = usize::from(area.width).saturating_sub(Line::from(title).width() + 1);
        for index in 0..fill {
            let style =
                if color && self.truecolor && self.preferences.effective().theme == "default" {
                    let fraction = index as f32 / fill.saturating_sub(1).max(1) as f32;
                    theme.base.fg(ratatui::style::Color::Rgb(
                        (107.0 + 148.0 * fraction) as u8,
                        (80.0 + 16.0 * fraction) as u8,
                        255,
                    ))
                } else {
                    accent
                };
            spans.push(Span::styled(
                if self.preferences.ascii { "/" } else { "╱" },
                style,
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_dialog_frame(&self, frame: &mut Frame, popup: Rect, theme: Theme) {
        let block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .style(theme.base)
            .border_style(theme.accent.remove_modifier(ratatui::style::Modifier::BOLD));
        let block = if self.preferences.ascii {
            block.border_set(ratatui::symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            })
        } else {
            block
        };
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);
    }

    fn dialog_search(&self, width: u16) -> (String, u16) {
        let capacity = usize::from(width.saturating_sub(3));
        let sanitized = Sanitizer::default().push(&self.filter);
        let mut visible = Vec::new();
        let mut cells = 0;
        for grapheme in sanitized.graphemes(true).rev() {
            let size = unicode_width::UnicodeWidthStr::width(grapheme);
            if cells + size > capacity {
                break;
            }
            visible.push(grapheme);
            cells += size;
        }
        (visible.into_iter().rev().collect(), cells as u16)
    }

    fn render_quit(&self, frame: &mut Frame, popup: Rect, theme: Theme) {
        self.render_dialog_frame(frame, popup, theme);
        for (offset, text) in [
            (2, "Discard unsaved demo draft?"),
            (6, "Tab / Left / Right selects; Enter confirms"),
            (7, "Esc returns without losing your draft"),
        ] {
            frame.render_widget(
                Paragraph::new(text)
                    .style(if offset == 2 { theme.base } else { theme.muted })
                    .alignment(ratatui::layout::Alignment::Center),
                Rect::new(popup.x + 2, popup.y + offset, popup.width - 4, 1),
            );
        }
        let mut buttons = Vec::new();
        for (index, label) in ["Keep editing", "Discard and exit"].iter().enumerate() {
            if index > 0 {
                buttons.push(Span::raw(" "));
            }
            let selected = self.selection == index;
            let style = if selected {
                if let Some(background) = theme.accent.fg {
                    if self.preferences.effective().theme == "high-contrast" {
                        theme.base.bg(background).fg(ratatui::style::Color::Black)
                    } else {
                        theme
                            .base
                            .bg(ratatui::style::Color::Rgb(255, 96, 255))
                            .fg(ratatui::style::Color::Rgb(255, 250, 241))
                    }
                } else {
                    theme
                        .accent
                        .add_modifier(ratatui::style::Modifier::REVERSED)
                }
            } else if theme.base.bg == Some(ratatui::style::Color::Rgb(32, 31, 38)) {
                theme.base.bg(ratatui::style::Color::Rgb(58, 57, 67))
            } else {
                theme.base
            };
            buttons.push(Span::styled(
                format!(
                    " {} {label}   ",
                    if selected && theme.accent.fg.is_none() {
                        ">"
                    } else {
                        " "
                    }
                ),
                style,
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(buttons)).alignment(ratatui::layout::Alignment::Center),
            Rect::new(popup.x + 2, popup.y + 4, popup.width - 4, 1),
        );
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let description = if theme.base.bg == Some(ratatui::style::Color::Rgb(32, 31, 38)) {
            theme.base.fg(ratatui::style::Color::Rgb(96, 95, 107))
        } else {
            theme.muted
        };
        let mut hints = Vec::new();
        if self.overlay.is_some() {
            hints.push(("esc", "close"));
            if self.overlay == Some(Overlay::Help) {
                hints.push(("pgup/pgdn", "pages"));
            }
        } else {
            if self.playing || self.active_fragment.is_some() {
                hints.push(("esc", "cancel"));
            }
            hints.push((
                "tab",
                if self.focus == Focus::Composer {
                    "focus chat"
                } else {
                    "focus editor"
                },
            ));
            hints.push((
                if self.focus == Focus::Composer && self.draft.text.is_empty() {
                    "/ or ctrl+p"
                } else {
                    "ctrl+p"
                },
                "commands",
            ));
            hints.push(("ctrl+l", "models"));
            if !presentation::compact(frame.area()) {
                hints.push(("ctrl+b", "sidebar"));
            }
            if self.focus == Focus::Composer {
                hints.push(("shift+enter", "newline"));
            } else {
                hints.extend([
                    ("up/down", "scroll"),
                    ("shift+up/down", "select"),
                    ("space", "expand"),
                ]);
            }
        }
        hints.extend([("ctrl+c", "quit"), ("ctrl+g", "help")]);
        let state = format!(
            "{} {}{}{}",
            if self.snapshot.demo {
                "demo"
            } else {
                "offline"
            },
            if self.anchor.is_some() {
                "paused"
            } else {
                "following"
            },
            if self.unread > 0 {
                format!(" / {} new", self.unread)
            } else {
                String::new()
            },
            if self.dropped { " / truncated" } else { "" }
        );
        let state_width = state.len() as u16;
        let available = usize::from(area.width.saturating_sub(state_width + 4));
        let mut spans = Vec::new();
        let mut used = 0;
        for (key, action) in hints {
            let separator = if used == 0 {
                ""
            } else if self.preferences.ascii {
                " * "
            } else {
                " • "
            };
            let length = Line::from(separator).width() + key.len() + action.len() + 1;
            if used + length > available {
                if used + 4 <= available {
                    spans.push(Span::styled(" ...", description));
                }
                break;
            }
            spans.extend([
                Span::styled(separator, description),
                Span::styled(key, theme.muted),
                Span::styled(format!(" {action}"), description),
            ]);
            used += length;
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x + 1, area.y, available as u16, 1),
        );
        frame.render_widget(
            Paragraph::new(state).style(description),
            Rect::new(area.right() - state_width - 1, area.y, state_width, 1),
        );
    }

    fn help_capacity(area: Rect) -> usize {
        usize::from(area.height.saturating_sub(4).min(20).saturating_sub(7)).max(1)
    }

    fn render_help(&self, frame: &mut Frame, popup: Rect, color: bool) {
        let theme = Theme::new(self.preferences.effective(), color);
        let capacity = Self::help_capacity(frame.area());
        let pages = HELP_ROWS.len().div_ceil(capacity);
        let page = self.help_page.min(pages - 1);
        self.render_dialog_frame(frame, popup, theme);
        let inner = Rect::new(
            popup.x + 2,
            popup.y + 1,
            popup.width.saturating_sub(4),
            popup.height.saturating_sub(2),
        );
        self.render_dialog_title(
            frame,
            Rect::new(inner.x, inner.y, inner.width, 1),
            &format!("Help {}/{}", page + 1, pages),
            color,
        );
        let rows =
            HELP_ROWS
                .iter()
                .skip(page * capacity)
                .take(capacity)
                .map(|(scope, keys, action)| {
                    Row::new(vec![
                        Span::styled(*scope, theme.muted),
                        Span::styled(
                            *keys,
                            theme.accent.remove_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        Span::styled(*action, theme.base),
                    ])
                });
        let table = Table::new(
            rows,
            [
                ratatui::layout::Constraint::Length(7),
                ratatui::layout::Constraint::Length(20),
                ratatui::layout::Constraint::Min(1),
            ],
        )
        .column_spacing(1)
        .header(Row::new(["Context", "Shortcut", "Action"]).style(theme.muted));
        frame.render_widget(
            table,
            Rect::new(inner.x, inner.y + 2, inner.width, (capacity + 1) as u16),
        );
        frame.render_widget(
            Paragraph::new("PgUp/PgDn: pages   Ctrl+G / Esc: close").style(theme.muted),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }

    fn render_cursor(&self, frame: &mut Frame) {
        if self.editing_notification && frame.area().width >= 60 && frame.area().height >= 16 {
            let popup = self.overlay_area(frame.area());
            let (_, row, column) = presentation::editor(
                &self.notification_editor.text,
                self.notification_editor.cursor,
                usize::from(popup.width - 4),
            );
            let height = usize::from(popup.height.saturating_sub(6));
            let top = row.saturating_sub(height.saturating_sub(1));
            frame.set_cursor_position((
                popup.x + 2 + column as u16,
                popup.y + 4 + (row - top) as u16,
            ));
            return;
        }
        if frame.area().width >= 60
            && frame.area().height >= 16
            && matches!(
                self.overlay,
                Some(Overlay::Palette | Overlay::Theme | Overlay::Developer)
            )
        {
            let popup = self.overlay_area(frame.area());
            let (_, cells) = self.dialog_search(popup.width - 4);
            frame.set_cursor_position((
                popup.x + 4 + cells,
                popup.y + if popup.height >= 16 { 3 } else { 2 },
            ));
            return;
        }
        if self.focus != Focus::Composer
            || self.overlay.is_some()
            || frame.area().width < 60
            || frame.area().height < 16
        {
            return;
        }
        let area = self.body_layout(frame.area())[2];
        let (_, row, column) = presentation::editor(
            &self.draft.text,
            self.draft.cursor,
            usize::from(area.width.saturating_sub(4)),
        );
        let visible = usize::from(area.height.saturating_sub(2));
        let top = row.saturating_sub(visible.saturating_sub(1));
        frame.set_cursor_position((area.x + 4 + column as u16, area.y + 1 + (row - top) as u16));
    }

    fn identity_moving(&self) -> bool {
        let settings = self.preferences.effective();
        self.overlay.is_none()
            && settings.animation_fps > 0
            && !settings.reduced_motion
            && crate::visual::motion_active(self.presentation_state(), self.state_age())
    }

    pub(crate) fn presentation_state(&self) -> TaskState {
        if self.playing {
            TaskState::Running
        } else {
            self.task_state()
        }
    }

    fn color_identity(&self, lines: &mut [Line<'static>], color: bool) {
        if color && self.preferences.effective().theme == "high-contrast" {
            crate::identity::fire_colors(
                lines,
                if self.identity_moving() {
                    self.state_age()
                } else {
                    Duration::ZERO
                },
                8.0,
                self.truecolor,
            );
        }
        crate::identity::state_colors(
            lines,
            self.presentation_state(),
            self.state_age(),
            self.identity_moving(),
            color,
            self.truecolor,
        );
    }

    fn render_diagnostics(&self, frame: &mut Frame, color: bool) {
        let area = frame.area();
        if !self.preferences.effective().flags.render_diagnostics
            || area.width < 60
            || area.height < 16
        {
            return;
        }
        let settings = self.preferences.effective();
        let theme = Theme::new(settings, color);
        let row = Rect::new(area.x + 1, area.bottom() - 1, area.width - 2, 1);
        let cadence = self.redraws as f64 / self.elapsed.as_secs_f64().max(1.0);
        let text = format!(
            "UI v{} | target {} | redraw {cadence:.1}/s avg | {}us | skip {}",
            self.preferences.version, settings.animation_fps, self.frame_micros, self.skipped
        );
        frame.render_widget(Block::default().style(theme.base), row);
        frame.render_widget(Paragraph::new(text).style(theme.muted), row);
    }

    pub(crate) fn render_identity(&self, frame: &mut Frame, color: bool, geometry: RenderGeometry) {
        let area = frame.area();
        if geometry.area != area {
            self.render(frame, color);
            return;
        }
        if area.width < 60 || area.height < 16 {
            return;
        }
        let settings = self.preferences.effective();
        let theme = Theme::new(settings, color);
        let working = self.playing || self.task_state() == TaskState::Running;
        let moving = working && settings.animation_fps > 0 && !settings.reduced_motion;
        for column in area.x..area.right() {
            frame.buffer_mut()[(column, area.y)].reset();
            frame.buffer_mut()[(column, area.y)].set_style(theme.base);
        }
        let mut activity = crate::identity::activity(
            area.width,
            if moving { self.elapsed } else { Duration::ZERO },
            working,
            moving,
            color,
            self.truecolor,
            self.preferences.ascii,
        );
        if color && settings.theme == "high-contrast" && working {
            crate::identity::fire_colors(
                std::slice::from_mut(&mut activity),
                if moving { self.elapsed } else { Duration::ZERO },
                0.3,
                self.truecolor,
            );
        }
        frame.render_widget(
            Paragraph::new(activity),
            Rect::new(area.x, area.y, area.width, 1),
        );
        if self.compact(area) {
            let header = Rect::new(area.x + 1, area.y + 1, area.width - 2, 1);
            frame.render_widget(Block::default().style(theme.base), header);
            let mut wordmark = crate::identity::compact_wordmark(
                if self.identity_moving() {
                    self.state_age()
                } else {
                    Duration::ZERO
                },
                color,
                self.truecolor,
            );
            self.color_identity(std::slice::from_mut(&mut wordmark), color);
            let mut spans = wordmark.spans;
            spans.push(Span::raw(" "));
            let details = if !self.snapshot.demo {
                Line::from(Span::styled(
                    "Configuration / runtime unavailable",
                    theme.muted,
                ))
            } else if matches!(self.overlay, Some(Overlay::Developer | Overlay::Theme)) {
                Line::from(Span::styled(
                    format!("Demo state: {:?}", self.task_state()),
                    theme.muted,
                ))
            } else {
                presentation::compact_details(
                    &self.repository,
                    header.width.saturating_sub(10),
                    self.overlay == Some(Overlay::Details),
                    theme,
                    self.preferences.ascii,
                )
            };
            let diagonals = usize::from(header.width).saturating_sub(7 + details.width());
            spans.push(Span::styled(
                if self.preferences.ascii { "/" } else { "╱" }.repeat(diagonals),
                theme.accent.remove_modifier(ratatui::style::Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            spans.extend(details.spans);
            frame.render_widget(Paragraph::new(Line::from(spans)), header);
        }
        if (self.compact(area) || matches!(self.overlay, Some(Overlay::Developer | Overlay::Theme)))
            && (self.overlay.is_none() || self.overlay_area(area).y > area.y + 2)
        {
            frame.render_widget(
                Block::default().style(theme.base),
                Rect::new(area.x + 1, area.y + 2, self.body_area(area).width, 1),
            );
            frame.render_widget(
                Paragraph::new(if self.snapshot.demo {
                    format!("Demo state: {:?} · NORMAL · offline", self.task_state())
                } else {
                    "NORMAL / runtime unavailable / no execution".into()
                })
                .style(theme.muted),
                Rect::new(area.x + 1, area.y + 2, self.body_area(area).width, 1),
            );
        }
        if !self.compact(area) && self.overlay.is_none() {
            self.render_wordmark(frame, color);
        }
        self.render_message_loading(frame, color, geometry.loading);
        self.render_notifications(frame, color);
        self.render_diagnostics(frame, color);
        self.render_cursor(frame);
        crate::visual::terminal_colors(frame.buffer_mut(), color, self.truecolor);
    }

    fn render_wordmark(&self, frame: &mut Frame, color: bool) {
        let area = Rect::new(
            self.body_area(frame.area()).right() + 1,
            frame.area().y + 1,
            30,
            3,
        );
        let settings = self.preferences.effective();
        let theme = Theme::new(settings, color);
        let moving = self.identity_moving();
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                frame.buffer_mut()[(column, row)].reset();
                frame.buffer_mut()[(column, row)].set_style(theme.base);
            }
        }
        let mut lines = crate::identity::wordmark(
            if moving {
                self.state_age()
            } else {
                Duration::ZERO
            },
            color,
            self.truecolor,
            self.preferences.ascii,
        );
        self.color_identity(&mut lines, color);
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_sidebar(&self, frame: &mut Frame, color: bool) {
        let area = frame.area();
        if self.compact(area) {
            return;
        }
        let inner = Rect::new(
            self.body_area(area).right() + 1,
            area.y + 1,
            29,
            area.height - 3,
        );
        let theme = Theme::new(self.preferences.effective(), color);
        if !self.snapshot.demo {
            let lines = vec![
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::from("Configuration workspace"),
                Line::from(self.repository.clone()),
                Line::default(),
                Line::from("Agent runtime unavailable"),
                Line::from("No provider contacted"),
                Line::from("No task or session active"),
                Line::default(),
                Line::from("Ctrl+P Configuration"),
                Line::from("NORMAL / offline"),
            ];
            frame.render_widget(
                Paragraph::new(lines)
                    .style(theme.muted)
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                inner,
            );
            self.render_wordmark(frame, color);
            return;
        }
        let session = self
            .snapshot
            .tasks
            .first()
            .map_or("Demo session".into(), |task| {
                format!("Demo session {}", task.session.0)
            });
        let mut lines = vec![
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(session, theme.muted)),
            Line::from(""),
        ];
        let repository =
            presentation::wrap(vec![Span::styled(self.repository.clone(), theme.muted)], 29);
        lines.extend(repository.iter().take(2).cloned());
        if repository.len() > 2 {
            lines.push(Line::from(Span::styled("... path truncated", theme.muted)));
        }
        lines.push(Line::default());
        lines.extend(presentation::demo_model_info(
            inner.width,
            theme,
            self.preferences.ascii,
        ));
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("Modified files", theme.muted)),
            Line::from(Span::styled("No files changed", theme.muted)),
            Line::from(""),
            Line::from(Span::styled(
                format!("Demo state: {:?}", self.task_state()),
                theme.muted,
            )),
            Line::from(Span::styled("NORMAL / offline demo", theme.muted)),
            Line::from(Span::styled("No runtime connected", theme.muted)),
        ]);
        frame.render_widget(Paragraph::new(lines), inner);
        self.render_wordmark(frame, color);
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

    fn open_notification_preview(state: &mut Shell) {
        state.action(6);
        for character in "notification preview".chars() {
            key(state, KeyCode::Char(character));
        }
        key(state, KeyCode::Enter);
        assert!(state.notification_preview);
        assert!(state.overlay.is_none());
    }

    fn preview_key(state: &mut Shell, character: char) {
        state.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::ALT));
    }

    #[test]
    fn notification_preview_is_explicit_bounded_and_preserves_user_state() {
        let mut state = shell();
        let mut options = VisualOptions::default();
        options.settings.dev_menu = true;
        state.preferences = Preferences::new(options).unwrap();
        state.paste("retained draft");
        key(&mut state, KeyCode::Tab);
        key(&mut state, KeyCode::PageUp);
        let anchor = state.anchor;
        let snapshot = state.snapshot.clone();
        open_notification_preview(&mut state);
        assert!(state.notification_stack.retained().is_empty());
        assert!(!state.notification_test);
        preview_key(&mut state, 'n');
        let first = state.notification_stack.retained()[0].clone();
        preview_key(&mut state, 'r');
        assert_eq!(state.notification_stack.retained(), &[first]);
        preview_key(&mut state, 'b');
        assert_eq!(state.notification_stack.retained().len(), 32);
        assert_eq!(state.notification_stack.overflow_count(), 9);
        preview_key(&mut state, 'd');
        assert_eq!(state.notification_stack.retained().len(), 31);
        assert_eq!(state.anchor, anchor);
        assert_eq!(state.focus, Focus::Conversation);
        assert_eq!(state.draft.text, "retained draft");
        assert_eq!(state.snapshot, snapshot);
        assert!(!state.notification_test);
        state.elapsed = Duration::from_secs(5);
        assert!(state.advance_notifications());
        assert!(state.notification_stack.retained().is_empty());
        assert!(!state.advance_notifications());
        preview_key(&mut state, 'n');
        key(&mut state, KeyCode::Esc);
        assert!(!state.notification_preview);
        assert!(state.notification_stack.retained().is_empty());
        assert_eq!(state.draft.text, "retained draft");
    }

    #[test]
    fn notification_close_click_targets_one_notice_without_changing_focus() {
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut state = shell();
            let mut options = VisualOptions::default();
            options.settings.dev_menu = true;
            state.preferences = Preferences::new(options).unwrap();
            state.paste("retained draft");
            open_notification_preview(&mut state);
            preview_key(&mut state, 'n');
            preview_key(&mut state, 'n');
            let area = Rect::new(0, 0, width, height);
            state.resize(area);
            let bounds = state.notification_area(area);
            let visible = state
                .notification_stack
                .visible(usize::from(bounds.height / 5))
                .count();
            let index = visible.saturating_sub(1);
            let target = state.notification_stack.retained()[index].cursor();
            let rect = Rect::new(bounds.x, bounds.y + index as u16 * 5, bounds.width, 5);
            let close = Shell::notification_close_area(rect);
            let event = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: close.x + 1,
                row: close.y,
                modifiers: KeyModifiers::NONE,
            };
            let anchor = state.anchor;
            let selected = state.selected_fragment;
            let snapshot = state.snapshot.clone();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| state.render(frame, false)).unwrap();
            assert_eq!(
                terminal.backend().buffer()[(event.column, event.row)].symbol(),
                "x"
            );
            for kind in [
                MouseEventKind::Up(MouseButton::Left),
                MouseEventKind::Down(MouseButton::Right),
                MouseEventKind::ScrollDown,
            ] {
                assert!(!state.mouse(MouseEvent { kind, ..event }, area));
                assert_eq!(state.notification_stack.retained().len(), 2);
            }
            assert!(!state.mouse(
                MouseEvent {
                    column: rect.x + 2,
                    row: rect.y + 2,
                    ..event
                },
                area
            ));
            assert_eq!(state.focus, Focus::Composer);
            state.overlay = Some(Overlay::Help);
            assert!(!state.mouse(event, area));
            state.overlay = None;
            assert!(state.mouse(event, area));
            assert_eq!(state.notification_stack.retained().len(), 1);
            assert!(
                state
                    .notification_stack
                    .retained()
                    .iter()
                    .all(|notice| notice.cursor() != target)
            );
            assert_eq!(state.focus, Focus::Composer);
            assert_eq!(state.anchor, anchor);
            assert_eq!(state.selected_fragment, selected);
            assert_eq!(state.draft.text, "retained draft");
            assert_eq!(state.snapshot, snapshot);
            assert!(!state.notification_test);
        }
    }

    #[test]
    fn notification_borders_and_close_buttons_follow_error_and_warning_colors() {
        use ratatui::style::Color;
        for name in ["default", "high-contrast"] {
            for (color, rgb, ascii) in [
                (true, true, false),
                (true, false, true),
                (false, false, true),
            ] {
                for severity in [
                    Severity::Information,
                    Severity::Success,
                    Severity::Warning,
                    Severity::Error,
                    Severity::Approval,
                ] {
                    let mut state = shell();
                    let mut options = VisualOptions {
                        ascii,
                        ..VisualOptions::default()
                    };
                    options.settings.theme = name.into();
                    options.settings.dev_menu = true;
                    state.preferences = Preferences::new(options).unwrap();
                    state.truecolor = rgb;
                    open_notification_preview(&mut state);
                    state
                        .notification_stack
                        .receive(
                            Cursor {
                                epoch: 0,
                                sequence: 1,
                            },
                            severity,
                            Lifetime::Transient,
                            "Synthetic fixture",
                            Duration::ZERO,
                        )
                        .unwrap();
                    let area = Rect::new(0, 0, 80, 24);
                    let bounds = state.notification_area(area);
                    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
                    terminal.draw(|frame| state.render(frame, color)).unwrap();
                    let buffer = terminal.backend().buffer();
                    let expected = if !color {
                        Color::Reset
                    } else {
                        match severity {
                            Severity::Error => Color::Red,
                            Severity::Warning => Color::Yellow,
                            _ => {
                                let mut sample =
                                    ratatui::buffer::Buffer::empty(Rect::new(0, 0, 1, 1));
                                sample[(0, 0)].set_style(
                                    Theme::new(state.preferences.effective(), color).accent,
                                );
                                crate::visual::terminal_colors(&mut sample, color, rgb);
                                sample[(0, 0)].fg
                            }
                        }
                    };
                    for (column, row) in [
                        (bounds.x, bounds.y),
                        (bounds.x + 1, bounds.y),
                        (bounds.right() - 1, bounds.y + 2),
                        (bounds.x + 1, bounds.y + 4),
                        (bounds.right() - 3, bounds.y),
                    ] {
                        assert_eq!(buffer[(column, row)].fg, expected);
                    }
                    assert_eq!(buffer[(bounds.right() - 3, bounds.y)].symbol(), "x");
                    let text = buffer
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>();
                    let label = match severity {
                        Severity::Error => "[DEMO ERROR]",
                        Severity::Warning => "[DEMO WARNING]",
                        _ => "[DEMO",
                    };
                    assert!(text.contains(label));
                }
            }
        }
    }

    #[test]
    fn notification_preview_buffers_preserve_geometry_and_cached_frames() {
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            for (color, rgb, ascii) in [
                (true, true, false),
                (true, false, false),
                (false, false, true),
            ] {
                let mut state = shell();
                state.truecolor = rgb;
                let mut options = VisualOptions {
                    ascii,
                    ..VisualOptions::default()
                };
                options.settings.dev_menu = true;
                state.preferences = Preferences::new(options).unwrap();
                state.paste("keep composer");
                let area = Rect::new(0, 0, width, height);
                state.resize(area);
                let layout = state.body_layout(area);
                open_notification_preview(&mut state);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| state.render(frame, color)).unwrap();
                let before = terminal.backend().buffer().clone();
                preview_key(&mut state, 'b');
                let mut geometry = RenderGeometry::default();
                terminal
                    .draw(|frame| geometry = state.render_frame(frame, color))
                    .unwrap();
                let full = terminal.backend().buffer().clone();
                let bounds = state.notification_area(area);
                assert!(bounds.x >= layout[1].x);
                assert!(bounds.right() <= layout[1].right());
                assert!(bounds.bottom() <= layout[1].bottom());
                assert_eq!(state.body_layout(area), layout);
                let count = state
                    .notification_stack
                    .visible(usize::from(bounds.height / 5))
                    .count();
                assert!(count > 0);
                for index in 0..count {
                    let top = bounds.y + index as u16 * 5;
                    let right = bounds.right() - 1;
                    assert_eq!(
                        full[(bounds.x, top)].symbol(),
                        if ascii { "+" } else { "╭" }
                    );
                    assert_eq!(full[(right, top)].symbol(), if ascii { "+" } else { "╮" });
                    assert_eq!(
                        full[(bounds.x, top + 4)].symbol(),
                        if ascii { "+" } else { "╰" }
                    );
                    assert_eq!(
                        full[(right, top + 4)].symbol(),
                        if ascii { "+" } else { "╯" }
                    );
                    for row in top + 1..top + 4 {
                        assert_eq!(
                            full[(bounds.x, row)].symbol(),
                            if ascii { "|" } else { "│" }
                        );
                        assert_eq!(full[(right, row)].symbol(), if ascii { "|" } else { "│" });
                    }
                }
                assert!(
                    full.content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                        .contains("[DEMO")
                );
                for row in 0..height {
                    for column in 0..width {
                        if !bounds.contains((column, row).into()) {
                            assert_eq!(before[(column, row)], full[(column, row)]);
                        }
                    }
                }
                terminal
                    .draw(|frame| {
                        frame.buffer_mut().clone_from(&full);
                        state.render_identity(frame, color, geometry);
                    })
                    .unwrap();
                assert_eq!(terminal.backend().buffer(), &full);
                state.overlay = Some(Overlay::Quit);
                assert_eq!(state.notification_area(area), Rect::default());
                state.elapsed = Duration::from_secs(5);
                assert!(state.advance_notifications());
                state.overlay = None;
                terminal.draw(|frame| state.render(frame, color)).unwrap();
                assert!(
                    !terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                        .contains("[DEMO")
                );
                assert_eq!(
                    state.notification_area(Rect::new(0, 0, 30, 8)),
                    Rect::default()
                );
            }
        }
    }

    #[test]
    fn desktop_preference_metadata_distinguishes_apply_capability_and_cli_lock() {
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            for supported in [false, true] {
                for locked in [false, true] {
                    for enabled in [false, true] {
                        let mut state = shell();
                        state.notification_transport_supported = supported;
                        let mut options = VisualOptions::default();
                        options.settings.dev_menu = true;
                        options.settings.notifications.desktop_enabled = enabled;
                        if locked {
                            options
                                .locked
                                .insert("tui.notifications.desktop_enabled".into());
                        }
                        state.preferences = Preferences::new(options).unwrap();
                        state.action(6);
                        for character in "desktop_enabled".chars() {
                            key(&mut state, KeyCode::Char(character));
                        }
                        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                        for edited in [false, true] {
                            if edited {
                                key(&mut state, KeyCode::Right);
                            }
                            terminal.draw(|frame| state.render(frame, false)).unwrap();
                            let text: String = terminal
                                .backend()
                                .buffer()
                                .content
                                .iter()
                                .map(|cell| cell.symbol())
                                .collect();
                            let expected = if locked {
                                "Locked by command line"
                            } else if !supported {
                                "Unavailable: Ghostty transport required"
                            } else {
                                "Session Apply"
                            };
                            assert!(text.contains(expected), "{width}x{height}: {expected}");
                            assert!(!text.contains("Live |"));
                            assert_eq!(
                                state.preferences.applied().notifications.desktop_enabled,
                                enabled
                            );
                            assert_eq!(
                                state.preferences.effective().notifications.desktop_enabled,
                                if edited && supported && !locked {
                                    !enabled
                                } else {
                                    enabled
                                }
                            );
                            assert!(!state.notification_test);
                        }
                        control(&mut state, 'u');
                        key(&mut state, KeyCode::Home);
                        terminal.draw(|frame| state.render(frame, false)).unwrap();
                        let text: String = terminal
                            .backend()
                            .buffer()
                            .content
                            .iter()
                            .map(|cell| cell.symbol())
                            .collect();
                        assert!(text.contains("Live |"));
                        assert!(!text.contains("Session Apply"));
                    }
                }
            }
        }
    }

    #[test]
    fn desktop_preference_requires_supported_transport_and_explicit_apply() {
        for supported in [false, true] {
            let mut state = shell();
            state.notification_transport_supported = supported;
            let mut options = VisualOptions::default();
            options.settings.dev_menu = true;
            state.preferences = Preferences::new(options).unwrap();
            state.action(6);
            for character in "desktop_enabled".chars() {
                key(&mut state, KeyCode::Char(character));
            }
            key(&mut state, KeyCode::Right);
            assert_eq!(
                state.preferences.effective().notifications.desktop_enabled,
                supported
            );
            assert!(!state.preferences.applied().notifications.desktop_enabled);
            assert!(!state.notification_test);
            key(&mut state, KeyCode::Esc);
            assert!(!state.preferences.effective().notifications.desktop_enabled);
            state.action(6);
            for character in "desktop_enabled".chars() {
                key(&mut state, KeyCode::Char(character));
            }
            key(&mut state, KeyCode::Right);
            control(&mut state, 'a');
            assert_eq!(
                state.preferences.applied().notifications.desktop_enabled,
                supported
            );
            assert!(!state.notification_test);
            state.action(6);
            control(&mut state, 'r');
            assert!(!state.preferences.effective().notifications.desktop_enabled);
            key(&mut state, KeyCode::Esc);
            assert_eq!(
                state.preferences.applied().notifications.desktop_enabled,
                supported
            );
        }
    }

    #[test]
    fn developer_navigation_and_notification_editor_preserve_composer() {
        let mut state = shell();
        let mut options = VisualOptions::default();
        options.settings.dev_menu = true;
        state.preferences = Preferences::new(options).unwrap();
        state.paste("retained composer");
        state.action(6);
        let total = state.visual_keys().len();
        assert_eq!(total, 7);
        key(&mut state, KeyCode::Tab);
        assert_eq!(state.selection, 1);
        key(&mut state, KeyCode::BackTab);
        assert_eq!(state.selection, 0);
        key(&mut state, KeyCode::End);
        assert_eq!(state.selection, total - 1);
        key(&mut state, KeyCode::Up);
        key(&mut state, KeyCode::Enter);
        assert!(state.editing_notification);
        control(&mut state, 'u');
        state.paste("Hello 世界\u{1b}]52;c;hidden\u{7};test\n");
        assert_eq!(state.notification_text(), "Hello 世界 test ");
        assert!(!state.notification_test);
        let preserved = state.notification_text().to_owned();
        state.paste(&"x".repeat(257));
        assert_eq!(state.notification_text(), preserved);
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut geometry = RenderGeometry::default();
            terminal
                .draw(|frame| {
                    geometry = state.render_frame(frame, true);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("Notification test text"));
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < width && cursor.y < height);
            terminal
                .draw(|frame| {
                    frame.buffer_mut().clone_from(&buffer);
                    state.render_identity(frame, true, geometry);
                })
                .unwrap();
            assert_eq!(&buffer, terminal.backend().buffer());
            assert_eq!(cursor, terminal.get_cursor_position().unwrap());
        }
        key(&mut state, KeyCode::Esc);
        assert!(!state.notification_test);
        assert_eq!(state.overlay, Some(Overlay::Developer));
        control(&mut state, 'e');
        key(&mut state, KeyCode::Enter);
        assert!(state.notification_test);
        assert!(!state.editing_notification);
        assert_eq!(state.draft.text, "retained composer");
        control(&mut state, 'u');
        key(&mut state, KeyCode::Home);
        state.key(KeyEvent::new_with_kind(
            KeyCode::Down,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ));
        assert_eq!(state.selection, 1);
        key(&mut state, KeyCode::Esc);
        assert_eq!(state.draft.text, "retained composer");
    }

    #[test]
    fn visual_menu_does_not_overpaint_any_wordmark_row() {
        for (width, height) in [(120, 40), (160, 50), (196, 36)] {
            let mut state = shell();
            state.truecolor = true;
            let mut options = VisualOptions::default();
            options.settings.dev_menu = true;
            state.preferences = Preferences::new(options).unwrap();
            state.action(6);
            for task_state in [
                TaskState::Pending,
                TaskState::Running,
                TaskState::Waiting,
                TaskState::Blocked,
                TaskState::Completed,
                TaskState::Failed,
                TaskState::Cancelled,
            ] {
                if state.snapshot.tasks.is_empty() {
                    use fluzo_core::application::*;
                    state.snapshot.tasks.push(TaskSnapshot {
                        id: TaskId(1),
                        session: SessionId(1),
                        workspace: WorkspaceId(1),
                        version: 1,
                        state: task_state,
                        reason: None,
                        attempt: None,
                        operation: None,
                        verification: VerificationState::NotRun,
                        active_milliseconds: 0,
                        title: "Synthetic".into(),
                    });
                }
                state.snapshot.tasks[0].state = task_state;
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| state.render(frame, true)).unwrap();
                let rendered = terminal.backend().buffer().clone();
                terminal
                    .draw(|frame| {
                        frame.buffer_mut().clone_from(&rendered);
                        state.render_wordmark(frame, true);
                    })
                    .unwrap();
                assert_eq!(
                    &rendered,
                    terminal.backend().buffer(),
                    "{task_state:?} at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn high_contrast_fire_covers_identity_loaders_and_cached_frames() {
        for rgb in [false, true] {
            for color in [false, true] {
                for size in [(80, 24), (120, 40)] {
                    let mut state = shell();
                    state.truecolor = rgb;
                    let mut options = VisualOptions::default();
                    options.settings.theme = "high-contrast".into();
                    state.preferences = Preferences::new(options).unwrap();
                    state.playing = true;
                    state.synthetic_fragment_tick(0);
                    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                    let mut geometry = RenderGeometry::default();
                    terminal
                        .draw(|frame| {
                            geometry = state.render_frame(frame, color);
                        })
                        .unwrap();
                    let cached = terminal.backend().buffer().clone();
                    if color && rgb {
                        for cell in &cached.content {
                            if let ratatui::style::Color::Rgb(red, green, blue) = cell.fg
                                && !cell.symbol().trim().is_empty()
                            {
                                assert!(
                                    red >= green && green >= blue,
                                    "{}: {red},{green},{blue}",
                                    cell.symbol()
                                );
                            }
                        }
                        assert!(
                            cached.content.iter().any(|cell| matches!(
                                cell.fg,
                                ratatui::style::Color::Rgb(255, _, _)
                            ))
                        );
                    }
                    state.elapsed = Duration::from_millis(500);
                    terminal
                        .draw(|frame| {
                            frame.buffer_mut().clone_from(&cached);
                            state.render_identity(frame, color, geometry);
                        })
                        .unwrap();
                    let animated = terminal.backend().buffer().clone();
                    terminal.draw(|frame| state.render(frame, color)).unwrap();
                    assert_eq!(&animated, terminal.backend().buffer());
                    state.preferences.begin();
                    state.preferences.change("tui.reduced_motion", 1).unwrap();
                    let frozen = state.message_loading(color);
                    state.elapsed = Duration::from_millis(900);
                    assert_eq!(frozen, state.message_loading(color));
                }
            }
        }
    }

    #[test]
    fn notification_feedback_is_visible_inside_developer_menu() {
        for (width, height) in [(60, 16), (80, 24), (196, 36)] {
            let mut state = shell();
            state.overlay = Some(Overlay::Developer);
            for feedback in [
                "Notification test in 3 seconds; focus another window to receive it.",
                "Notification suppressed: terminal reports window focused.",
                "Notifications disabled; start with --desktop-notifications.",
                "Notification test sent to terminal; desktop delivery is not confirmed.",
            ] {
                state.status = feedback.into();
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| state.render(frame, false)).unwrap();
                let popup = state.overlay_area(terminal.backend().buffer().area);
                let text = (popup.bottom() - 3..popup.bottom() - 1)
                    .map(|row| {
                        (popup.x + 2..popup.right() - 2)
                            .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                assert_eq!(
                    text.split_whitespace().collect::<Vec<_>>(),
                    feedback.split_whitespace().collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn diagnostics_are_visible_reversible_and_leave_controls_uncovered() {
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut state = shell();
            state.paste("retained draft");
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let original = terminal.backend().buffer().clone();
            state.preferences.begin();
            state
                .preferences
                .change("tui.flags.render_diagnostics", 1)
                .unwrap();
            state.redraws = 20;
            state.elapsed = Duration::from_secs(2);
            state.frame_micros = 42;
            state.skipped = 3;
            for overlay in [
                None,
                Some(Overlay::Developer),
                Some(Overlay::Help),
                Some(Overlay::Quit),
            ] {
                state.overlay = overlay;
                terminal.draw(|frame| state.render(frame, true)).unwrap();
                let buffer = terminal.backend().buffer();
                let row: String = (0..width)
                    .map(|column| buffer[(column, height - 1)].symbol())
                    .collect();
                assert!(row.contains("UI v0 | target 60 | redraw 10.0/s avg | 42us | skip 3"));
                let footer: String = (0..width)
                    .map(|column| buffer[(column, height - 2)].symbol())
                    .collect();
                assert!(footer.contains("demo following"));
                assert_eq!(state.draft.text, "retained draft");
            }
            state.overlay = None;
            state.preferences.cancel();
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            assert_eq!(&original, terminal.backend().buffer());
        }
    }

    #[test]
    fn capability_fallback_covers_every_rendered_surface() {
        use ratatui::style::Color;
        for color in [false, true] {
            for theme in ["default", "high-contrast"] {
                for ascii in [false, true] {
                    let mut state = shell();
                    let mut options = VisualOptions::default();
                    options.settings.theme = theme.into();
                    options.settings.flags.render_diagnostics = true;
                    options.ascii = ascii;
                    state.preferences = Preferences::new(options).unwrap();
                    state.synthetic_fragment_tick(0);
                    for overlay in [
                        None,
                        Some(Overlay::Palette),
                        Some(Overlay::Theme),
                        Some(Overlay::Developer),
                        Some(Overlay::Models),
                        Some(Overlay::Details),
                        Some(Overlay::Help),
                        Some(Overlay::Quit),
                    ] {
                        state.overlay = overlay;
                        for (width, height) in [(30, 8), (60, 16), (120, 40)] {
                            let mut terminal =
                                Terminal::new(TestBackend::new(width, height)).unwrap();
                            terminal.draw(|frame| state.render(frame, color)).unwrap();
                            for cell in &terminal.backend().buffer().content {
                                assert!(!matches!(cell.fg, Color::Rgb(..) | Color::Indexed(_)));
                                assert!(!matches!(cell.bg, Color::Rgb(..) | Color::Indexed(_)));
                                if !color {
                                    assert_eq!(cell.fg, Color::Reset);
                                    assert_eq!(cell.bg, Color::Reset);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unread_activity_is_visible_until_follow_without_changing_focus() {
        for (width, height) in [(60, 16), (80, 24), (120, 40)] {
            let mut state = shell();
            state.resize(Rect::new(0, 0, width, height));
            state.paste("keep draft");
            for _ in 0..60 {
                state.stream("synthetic history");
            }
            state.scroll_rows(-10);
            let anchor = state.anchor;
            state.stream("new activity");
            state.stream("more activity");
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| state.render(frame, false)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("demo paused / 2 new"));
            assert_eq!(state.anchor, anchor);
            assert_eq!(state.focus, Focus::Composer);
            assert_eq!(state.draft.text, "keep draft");
            state.follow();
            terminal.draw(|frame| state.render(frame, false)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("demo following"));
            assert!(!text.contains("2 new"));
            assert_eq!(state.unread, 0);
        }
    }

    #[test]
    fn animation_geometry_refreshes_after_reflow_scroll_and_expansion() {
        let mut state = shell();
        state.truecolor = true;
        for index in 0..40 {
            state.stream(&format!("message {index}"));
        }
        state.synthetic_fragment_tick(0);
        for (width, height) in [(60, 16), (120, 40), (80, 24), (160, 50)] {
            state.resize(Rect::new(0, 0, width, height));
            for paused in [true, false] {
                if paused {
                    state.scroll_rows(-10);
                } else {
                    state.follow();
                }
                state.fragments[0].expanded = !state.fragments[0].expanded;
                state.paste("wrapped draft ");
                for millis in [500, 2100] {
                    state.elapsed = Duration::from_millis(millis);
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    let mut geometry = RenderGeometry::default();
                    let before = state.formatting_passes.get();
                    terminal
                        .draw(|frame| {
                            geometry = state.render_frame(frame, true);
                        })
                        .unwrap();
                    assert_eq!(state.formatting_passes.get(), before + 1);
                    let cached = terminal.backend().buffer().clone();
                    state.elapsed += Duration::from_millis(100);
                    terminal
                        .draw(|frame| {
                            frame.buffer_mut().clone_from(&cached);
                            state.render_identity(frame, true, geometry);
                        })
                        .unwrap();
                    assert_eq!(state.formatting_passes.get(), before + 1);
                    let animated = terminal.backend().buffer().clone();
                    terminal.draw(|frame| state.render(frame, true)).unwrap();
                    assert_eq!(&animated, terminal.backend().buffer());
                }
            }
        }
    }

    #[test]
    fn paste_never_submits_and_overlays_preserve_draft_focus_and_scroll() {
        let mut state = shell();
        state.paste("hola\n世界\u{1b}[31m!");
        assert_eq!(state.draft.text, "hola\n世界!");
        for _ in 0..40 {
            state.stream("retained output");
        }
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
        control(&mut state, 'c');
        key(&mut state, KeyCode::Enter);
        assert!(!state.quit);
        control(&mut state, 'c');
        key(&mut state, KeyCode::Tab);
        key(&mut state, KeyCode::Enter);
        assert!(state.quit);
    }

    #[test]
    fn top_loader_tracks_work_and_returns_to_static_idle() {
        let mut state = shell();
        state.truecolor = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        let idle = terminal.backend().buffer().clone();
        assert_eq!(idle[(0, 0)].symbol(), " ");
        assert_eq!(idle[(8, 0)].symbol(), "▔");
        let background = ratatui::style::Color::Rgb(32, 31, 38);
        for cell in &idle.content {
            if cell.symbol() == " " {
                assert_eq!(cell.bg, background);
            }
        }
        assert_eq!(idle[(8, 0)].bg, background);
        state.action(0);
        state.elapsed = Duration::from_millis(400);
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert_eq!(terminal.backend().buffer()[(16, 0)].symbol(), "▔");
        assert_eq!(terminal.backend().buffer()[(15, 0)].symbol(), " ");
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("esc cancel"));
        key(&mut state, KeyCode::Esc);
        state.elapsed = Duration::from_secs(20);
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("tab focus chat"));
        assert!(!content.contains("esc cancel"));
        for row in 0..4 {
            for column in 0..80 {
                assert_eq!(
                    idle[(column, row)],
                    terminal.backend().buffer()[(column, row)]
                );
            }
        }
        state.playing = true;
        state.preferences.begin();
        state.preferences.change("tui.reduced_motion", 1).unwrap();
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert_eq!(terminal.backend().buffer()[(8, 0)].symbol(), "▔");
        state.preferences.cancel();
        state.preferences.begin();
        for _ in 0..60 {
            state.preferences.change("tui.animation_fps", -1).unwrap();
        }
        state.preferences.ascii = true;
        terminal.draw(|frame| state.render(frame, false)).unwrap();
        let frozen = terminal.backend().buffer().clone();
        assert_eq!(frozen[(8, 0)].symbol(), "-");
        state.elapsed = Duration::from_secs(40);
        terminal.draw(|frame| state.render(frame, false)).unwrap();
        for column in 0..80 {
            assert_eq!(
                frozen[(column, 0)],
                terminal.backend().buffer()[(column, 0)]
            );
        }
    }

    #[test]
    fn wheel_scroll_is_viewport_scoped_bounded_and_preserves_input() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        let mut state = shell();
        state.paste("preserve draft");
        for index in 0..60 {
            state.stream(&format!("line {index}"));
        }
        let wheel = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert!(!state.mouse(wheel(MouseEventKind::ScrollUp, 100, 8), area));
        assert!(!state.mouse(wheel(MouseEventKind::ScrollUp, 5, 35), area));
        assert!(state.mouse(wheel(MouseEventKind::ScrollUp, 5, 8), area));
        let anchor = state.anchor;
        state.stream("new output");
        assert_eq!(state.anchor, anchor);
        assert_eq!(state.unread, 1);
        assert_eq!(state.focus, Focus::Composer);
        assert_eq!(state.draft.text, "preserve draft");
        state.action(5);
        assert!(!state.mouse(wheel(MouseEventKind::ScrollDown, 5, 8), area));
        state.close_preview();
        for _ in 0..100 {
            state.mouse(wheel(MouseEventKind::ScrollUp, 5, 8), area);
        }
        assert_eq!(state.anchor, Some(state.first_line));
        for _ in 0..100 {
            state.mouse(wheel(MouseEventKind::ScrollDown, 5, 8), area);
        }
        assert_eq!(state.anchor, None);
        assert_eq!(state.unread, 0);
        let mut empty = shell();
        assert!(!empty.mouse(wheel(MouseEventKind::ScrollUp, 5, 8), area));
        assert_eq!(empty.anchor, None);
    }

    #[test]
    fn sidebar_scroll_indicator_tracks_top_middle_bottom_and_short_content() {
        let mut state = shell();
        state.truecolor = true;
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        let conversation = state.body_layout(area)[1];
        let visible = usize::from(conversation.height);
        let column = conversation.right();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert!((0..40).all(|row| terminal.backend().buffer()[(column, row)].symbol() != "┃"));
        for index in 0..100 {
            state.stream(&format!("line {index}"));
        }
        let maximum = state.lines.len() - visible;
        let thumb = (visible * visible / state.lines.len()).max(1);
        for position in [0, maximum / 2, maximum] {
            state.scrollbar_until = Duration::from_secs(2);
            state.anchor = Some(state.first_line + position);
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let marked: Vec<_> = (0..40)
                .filter(|row| terminal.backend().buffer()[(column, *row)].symbol() == "┃")
                .collect();
            assert_eq!(marked.len(), thumb);
            for row in conversation.y..conversation.bottom() {
                let cell = &terminal.backend().buffer()[(column, row)];
                let selected = marked.contains(&row);
                assert_eq!(cell.symbol(), if selected { "┃" } else { "│" });
                assert_eq!(
                    cell.fg,
                    if selected {
                        ratatui::style::Color::Rgb(255, 96, 255)
                    } else {
                        ratatui::style::Color::Rgb(58, 57, 67)
                    }
                );
            }
            assert_eq!(
                marked[0],
                conversation.y + (position * (visible - thumb) / maximum) as u16
            );
        }
        state.follow();
        state.preferences.ascii = true;
        terminal.draw(|frame| state.render(frame, false)).unwrap();
        assert_eq!(
            terminal.backend().buffer()[(column, conversation.y + visible as u16 - 1)].symbol(),
            "#"
        );
        state.elapsed = Duration::from_secs(2);
        assert!(!state.scrollbar_visible());
        terminal.draw(|frame| state.render(frame, false)).unwrap();
        assert!(
            (conversation.y..conversation.bottom())
                .all(|row| terminal.backend().buffer()[(column, row)].symbol() == " ")
        );
        state.resize(area);
        state.scroll_rows(-1);
        assert!(state.scrollbar_visible());
        state.elapsed = Duration::from_millis(3999);
        state.stream("incoming output");
        assert!(state.scrollbar_visible());
        state.elapsed = Duration::from_secs(4);
        assert!(!state.scrollbar_visible());
    }

    #[test]
    fn synthetic_fragments_select_without_effects_and_finish_with_example_metrics() {
        let mut state = shell();
        let area = ratatui::layout::Rect::new(0, 0, 120, 40);
        state.paste("keep draft");
        state.synthetic_fragment_tick(0);
        let start = state.fragments[0].start;
        assert!(!state.fragments[0].complete);
        assert!(!state.lines.iter().any(|line| line.contains("tokens")));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        let row = state.body_layout(area)[1].y + (start - state.first_line) as u16;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert!(state.mouse(click, area));
        assert_eq!(state.selected_fragment, Some(start));
        assert_eq!(state.focus, Focus::Conversation);
        assert_eq!(state.draft.text, "keep draft");
        assert!(!state.playing);
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert_eq!(terminal.backend().buffer()[(1, row)].symbol(), "▌");
        assert!(state.lines[start - state.first_line + 1].starts_with("  "));
        state.synthetic_fragment_tick(1);
        assert!(state.fragments[0].complete);
        assert_eq!(state.selected_fragment, Some(start));
        let footer = &state.footers[0];
        assert_eq!(footer.1.model, "GTP-6 Astra");
        assert_eq!(footer.1.provider, "Github Copilot");
        assert_eq!(footer.1.duration, "1m21s");
        assert!(footer.0 >= state.fragments[0].end);
        assert!(!state.lines.iter().any(|line| line.contains("tokens")));
        state.synthetic_fragment_tick(2);
        let stopped = state.active_fragment.unwrap();
        state.action(1);
        assert!(
            !state
                .fragments
                .iter()
                .find(|fragment| fragment.start == stopped)
                .unwrap()
                .complete
        );
        assert!(state.lines[stopped - state.first_line].contains("Demo stopped"));
        state.focus = Focus::Conversation;
        key(&mut state, KeyCode::Char('J'));
        assert_eq!(
            state.selected_fragment,
            state.messages.back().map(|(start, _, _)| *start)
        );
        key(&mut state, KeyCode::Char('J'));
        assert_eq!(state.selected_fragment, Some(stopped));
        for index in 0..400 {
            state.synthetic_fragment_tick(index);
        }
        assert_eq!(state.lines.len(), HISTORY_LIMIT);
        assert!(state.fragments.len() <= HISTORY_LIMIT);
        assert_eq!(state.selected_fragment, None);
    }

    #[test]
    fn fragment_status_icons_and_titles_are_thin_and_distinct() {
        for color in [false, true] {
            let mut state = shell();
            state.synthetic_fragment_tick(0);
            let start = state.fragments[0].start;
            state.selected_fragment = Some(start);
            state.focus = Focus::Conversation;
            let area = ratatui::layout::Rect::new(0, 0, 120, 40);
            let row = state.body_layout(area)[1].y + (start - state.first_line) as u16;
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            for complete in [false, true] {
                if complete {
                    state.synthetic_fragment_tick(1);
                }
                terminal.draw(|frame| state.render(frame, color)).unwrap();
                let buffer = terminal.backend().buffer();
                assert_eq!(buffer[(1, row)].symbol(), "▌");
                assert_eq!(buffer[(3, row)].symbol(), if complete { "✓" } else { "●" });
                assert!(
                    !buffer[(3, row)]
                        .modifier
                        .contains(ratatui::style::Modifier::BOLD)
                );
                assert!(
                    !buffer[(5, row)]
                        .modifier
                        .contains(ratatui::style::Modifier::BOLD)
                );
            }
            state.preferences.ascii = true;
            terminal.draw(|frame| state.render(frame, color)).unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(3, row)].symbol(), "[");
            assert_eq!(buffer[(4, row)].symbol(), "v");
            assert_eq!(buffer[(5, row)].symbol(), "]");
        }
    }

    #[test]
    fn wrapped_messages_keep_selection_scroll_and_complete_text_across_resize() {
        use unicode_width::UnicodeWidthStr;
        let mut state = shell();
        state.paste("keep draft");
        state.append("User preview", &"wrapped words 世界 e\u{301} ".repeat(20));
        let identity = state.messages.back().unwrap().0;
        let snapshot = state.snapshot.clone();
        state.selected_fragment = Some(identity);
        state.focus = Focus::Conversation;
        state.anchor = Some(identity);
        for (width, height) in [(80, 24), (120, 40), (60, 16), (240, 80)] {
            let area = Rect::new(0, 0, width, height);
            state.resize(area);
            let conversation = state.body_layout(area)[1];
            let rows = state.display_rows(conversation.width, true);
            assert!(
                rows.iter()
                    .all(|row| row.line.width() <= usize::from(conversation.width).min(122))
            );
            let start = state.display_start(&rows, conversation.height);
            assert_eq!(rows[start].source, identity);
            assert!(rows[start].line.spans[0].content.contains('▌'));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            assert!(terminal.get_cursor_position().unwrap().x < state.body_area(area).right());
            assert_eq!(state.draft.text, "keep draft");
            assert_eq!(state.snapshot, snapshot);
        }
        assert!(state.scroll_rows(3));
        let anchor = (state.anchor, state.anchor_part);
        state.stream("new content");
        assert_eq!((state.anchor, state.anchor_part), anchor);
        assert!("世界".width() == 4);
    }

    #[test]
    fn playback_completion_finalizes_partial_fragment_without_creating_another() {
        let mut state = shell();
        state.action(0);
        state.synthetic_fragment_tick(0);
        state.finish_playback();
        assert!(!state.playing);
        assert!(state.active_fragment.is_none());
        assert!(state.fragments.back().unwrap().complete);
        let count = state.footers.len();
        state.finish_playback();
        assert_eq!(state.footers.len(), count);
        state.action(0);
        state.synthetic_fragment_tick(2);
        state.action(1);
        assert!(!state.fragments.back().unwrap().complete);
        assert_eq!(state.footers.len(), count);
    }

    #[test]
    fn loading_timer_tracks_playback_across_fragments_and_disabled_motion() {
        let mut state = shell();
        state.elapsed = Duration::from_secs(10);
        state.action(0);
        state.synthetic_fragment_tick(0);
        assert!(state.message_loading(false).to_string().ends_with(" 0s"));
        state.elapsed = Duration::from_secs(71);
        assert!(state.message_loading(false).to_string().ends_with(" 1m01s"));
        state.synthetic_fragment_tick(1);
        assert_eq!(state.loading_timer_second(), None);
        state.synthetic_fragment_tick(2);
        assert_eq!(state.loading_timer_second(), Some(61));
        let mut options = VisualOptions::default();
        options.settings.animation_fps = 0;
        options.settings.reduced_motion = true;
        state.preferences = Preferences::new(options).unwrap();
        state.elapsed = Duration::from_secs(72);
        assert!(state.message_loading(false).to_string().ends_with(" 1m02s"));
        state.action(1);
        assert_eq!(state.loading_timer_second(), None);
        state.action(0);
        state.synthetic_fragment_tick(0);
        assert!(state.message_loading(false).to_string().ends_with(" 0s"));
    }

    #[test]
    fn inline_wave_exists_only_while_waiting_and_never_enters_history() {
        let mut state = shell();
        state.paste("keep draft");
        let loading = |state: &Shell| {
            state.display_rows(85, false).iter().any(|row| {
                row.line
                    .spans
                    .iter()
                    .any(|span| span.content.contains("Working (demo)"))
            })
        };
        assert!(!loading(&state));
        state.synthetic_fragment_tick(0);
        assert!(loading(&state));
        let before = state.lines.clone();
        let first = state.message_loading(false);
        state.elapsed = Duration::from_millis(300);
        assert_ne!(first, state.message_loading(false));
        assert_eq!(before, state.lines);
        state.preferences.begin();
        state.preferences.change("tui.reduced_motion", 1).unwrap();
        assert_eq!(first, state.message_loading(false));
        state.synthetic_fragment_tick(1);
        assert!(!loading(&state));
        state.synthetic_fragment_tick(2);
        state.action(1);
        assert!(!loading(&state));
        assert_eq!(state.draft.text, "keep draft");
        assert!(
            state
                .lines
                .iter()
                .all(|line| !line.contains("Working (demo)"))
        );
    }

    #[test]
    fn tool_expansion_keeps_hidden_output_and_does_not_dispatch() {
        let mut state = shell();
        state.paste("keep draft");
        state.synthetic_fragment_tick(2);
        state.synthetic_fragment_tick(3);
        let start = state.fragments.back().unwrap().start;
        state.selected_fragment = Some(start);
        state.focus = Focus::Conversation;
        let text = |state: &Shell| {
            state
                .display_rows(86, false)
                .into_iter()
                .flat_map(|row| row.line.spans)
                .map(|span| span.content.into_owned())
                .collect::<String>()
        };
        assert!(text(&state).contains("more lines"));
        assert!(!text(&state).contains("14  synthetic"));
        key(&mut state, KeyCode::Char(' '));
        assert!(text(&state).contains("14  synthetic"));
        assert!(!text(&state).contains("more lines"));
        key(&mut state, KeyCode::Char(' '));
        assert!(text(&state).contains("more lines"));
        assert!(!state.playing);
        assert_eq!(state.draft.text, "keep draft");
    }

    #[test]
    fn user_preview_during_tool_output_does_not_join_the_tool_block() {
        let mut state = shell();
        state.synthetic_fragment_tick(0);
        state.paste("user text");
        key(&mut state, KeyCode::Enter);
        assert!(state.active_fragment.is_none());
        let user_start = state.messages.back().unwrap().0;
        state.synthetic_fragment_tick(2);
        assert!(
            state
                .fragments
                .iter()
                .all(|fragment| user_start < fragment.start || user_start >= fragment.end)
        );
        assert!(state.lines.iter().any(|line| line.contains("Demo stopped")));
    }

    #[test]
    fn relief_wordmark_fits_sidebar_and_honors_motion_preferences() {
        let mut state = shell();
        state.truecolor = true;
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let logo = |terminal: &Terminal<TestBackend>| {
            (1..4)
                .flat_map(|row| {
                    (88..118).map(move |column| terminal.backend().buffer()[(column, row)].clone())
                })
                .collect::<Vec<_>>()
        };
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        let idle = logo(&terminal);
        assert!(
            idle.iter()
                .any(|cell| matches!(cell.symbol(), "▀" | "▄" | "█"))
        );
        state.playing = true;
        state.elapsed = Duration::from_secs(4);
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert_ne!(idle, logo(&terminal));
        state.preferences.begin();
        state.preferences.change("tui.reduced_motion", 1).unwrap();
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        let static_running = logo(&terminal);
        assert_ne!(idle, static_running);
        state.elapsed = Duration::from_secs(10);
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert_eq!(static_running, logo(&terminal));
        state.preferences.cancel();
        state.preferences.begin();
        for _ in 0..60 {
            state.preferences.change("tui.animation_fps", -1).unwrap();
        }
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert_eq!(static_running, logo(&terminal));
    }

    #[test]
    fn compact_model_details_share_sidebar_content_and_toggle_without_effects() {
        let mut state = shell();
        state.set_repository("~/work/fluzo");
        state.paste("retained draft");
        for (width, height) in [(60, 16), (80, 24), (120, 40)] {
            state.resize(Rect::new(0, 0, width, height));
            state.hide_sidebar = width >= 120;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let header: String = (0..width)
                .map(|column| terminal.backend().buffer()[(column, 1)].symbol())
                .collect();
            assert!(header.contains("~/work/fluzo • 75% • ctrl+d open"));
            assert!(!header.contains("GTP-6"));
            control(&mut state, 'd');
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(1, 2)].symbol(), "╭");
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("ctrl+d close"));
            assert!(text.contains("◇ GTP-6 Astra via Github Copilot"));
            assert!(text.contains("  Reasoning X-High"));
            assert!(text.contains("━━━━━━━━━─── 75% (303.8K)"));
            assert!(text.contains("━━━━━━────── $50.00"));
            assert!(text.contains("Example data / offline"));
            control(&mut state, 'd');
            assert!(state.overlay.is_none());
            assert_eq!(state.draft.text, "retained draft");
            assert!(!state.playing);
            assert!(!state.quit);
        }
        let theme = Theme::new(state.preferences.effective(), true);
        let narrow = presentation::demo_model_info(29, theme, false);
        assert_eq!(narrow[0].to_string(), "◇ GTP-6 Astra");
        assert_eq!(narrow[1].to_string(), "  via Github Copilot");
        assert_eq!(
            narrow[0].spans[0].style.fg,
            Some(ratatui::style::Color::Rgb(96, 95, 107))
        );
        assert_eq!(
            narrow[0]
                .spans
                .iter()
                .find(|span| span.content == "G")
                .unwrap()
                .style,
            theme.base
        );
        for width in 0..80 {
            let header = presentation::compact_details(
                &"/世界/e\u{301}".repeat(50),
                width,
                false,
                theme,
                false,
            );
            assert!(header.width() <= usize::from(width));
        }
    }

    #[test]
    fn sidebar_orders_context_and_never_overlaps_the_body() {
        let mut state = shell();
        state.set_repository("/tmp/demo\u{1b}]52;c;hidden\u{7}/repo\u{202e}\nname");
        assert!(!state.repository.contains('\u{1b}'));
        assert!(!state.repository.contains("hidden"));
        assert!(state.repository.contains("[U+202E]"));
        assert!(state.repository.contains("[newline]"));
        state.paste(&"draft".repeat(25));
        for (width, height) in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let body = state.body_area(ratatui::layout::Rect::new(0, 0, width, height));
            if presentation::compact(Rect::new(0, 0, width, height)) {
                assert_eq!(body.width, width - 2);
                assert!(terminal.get_cursor_position().unwrap().x < body.right());
                continue;
            }
            let rows: Vec<String> = (1..height - 2)
                .map(|row| {
                    (body.right() + 1..width)
                        .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                        .collect()
                })
                .collect();
            let session = rows
                .iter()
                .position(|row| row.contains("Demo session"))
                .unwrap();
            let repository = rows
                .iter()
                .position(|row| row.contains("/tmp/demo"))
                .unwrap();
            let model = rows
                .iter()
                .position(|row| row.contains("◇ GTP-6 Astra"))
                .unwrap();
            assert!(session < repository && repository < model);
            assert!(rows[model + 1].contains("via Github Copilot"));
            assert!(rows[model + 2].contains("  Reasoning X-High"));
            assert!(rows[model + 3].contains("━━━━━━━━━─── 75% (303.8K)"));
            assert!(rows[model + 4].contains("━━━━━━────── $50.00"));
            assert!(rows[model + 5].contains("Cost scale: $100 (demo)"));
            assert!(rows[model + 6].contains("Example data / offline"));
            assert!(
                !rows
                    .iter()
                    .any(|row| matches!(row.trim(), "Session" | "Repository" | "Model"))
            );
            assert!(!rows.iter().any(|row| row.contains("draft")));
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < body.right());
        }
        state.set_repository(&"x".repeat(5000));
        assert!(state.repository.ends_with("[truncated]"));
        assert!(state.repository.len() < 1100);
    }

    #[test]
    fn visual_previews_preserve_navigation_and_revert_on_every_exit() {
        let mut state = shell();
        state.paste("keep draft");
        key(&mut state, KeyCode::Tab);
        key(&mut state, KeyCode::PageUp);
        let anchor = state.anchor;
        state.action(5);
        key(&mut state, KeyCode::Right);
        assert_eq!(state.preferences.effective().theme, "high-contrast");
        state.stream("background preview");
        key(&mut state, KeyCode::Esc);
        assert_eq!(state.preferences.effective().theme, "default");
        assert_eq!(state.anchor, anchor);
        assert_eq!(state.focus, Focus::Conversation);
        assert_eq!(state.draft.text, "keep draft");
        state.action(5);
        key(&mut state, KeyCode::Right);
        control(&mut state, 'c');
        assert_eq!(state.preferences.effective().theme, "default");
        assert!(!state.quit);
        state.action(5);
        key(&mut state, KeyCode::Right);
        control(&mut state, 'a');
        assert_eq!(state.preferences.effective().theme, "high-contrast");
        assert_eq!(state.preferences.version, 1);
        assert!(state.status.contains("no file saved"));
    }

    #[test]
    fn theme_reset_preserves_other_preferences_and_remains_reversible() {
        for locked in [false, true] {
            let mut state = shell();
            let mut options = VisualOptions::default();
            options.settings.dev_menu = true;
            options.settings.theme = "high-contrast".into();
            options.settings.animation_fps = 15;
            options.settings.reduced_motion = true;
            options.settings.flags.render_diagnostics = true;
            if locked {
                options.locked.insert("tui.theme".into());
            }
            let original = options.settings.clone();
            state.preferences = Preferences::new(options).unwrap();
            state.paste("keep draft");
            key(&mut state, KeyCode::Tab);
            key(&mut state, KeyCode::PageUp);
            let anchor = state.anchor;
            let mut expected = original.clone();
            if !locked {
                expected.theme = "default".into();
            }
            state.action(5);
            control(&mut state, 'r');
            assert_eq!(state.preferences.effective(), &expected);
            key(&mut state, KeyCode::Esc);
            assert_eq!(state.preferences.effective(), &original);
            state.action(5);
            control(&mut state, 'r');
            control(&mut state, 'a');
            assert_eq!(state.preferences.effective(), &expected);
            assert_eq!(state.anchor, anchor);
            assert_eq!(state.focus, Focus::Conversation);
            assert_eq!(state.draft.text, "keep draft");
            state.action(6);
            control(&mut state, 'r');
            assert_eq!(state.preferences.effective().animation_fps, 60);
            assert!(!state.preferences.effective().reduced_motion);
            assert!(!state.preferences.effective().flags.render_diagnostics);
            key(&mut state, KeyCode::Esc);
            assert_eq!(state.preferences.effective(), &expected);
        }
    }

    #[test]
    fn visual_lock_feedback_tracks_selection_instead_of_last_error() {
        for size in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut state = shell();
            let options =
                VisualOptions::parse(&["--dev-menu".into(), "--animation-fps".into(), "0".into()])
                    .unwrap();
            state.preferences = Preferences::new(options).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
            let mut content = |state: &Shell| {
                terminal.draw(|frame| state.render(frame, false)).unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            };
            state.action(6);
            key(&mut state, KeyCode::Down);
            assert!(content(&state).contains("Locked by command line"));
            key(&mut state, KeyCode::Right);
            assert_eq!(state.preferences.effective().animation_fps, 0);
            key(&mut state, KeyCode::Up);
            assert!(!content(&state).contains("Locked by command line"));
            key(&mut state, KeyCode::Right);
            assert_eq!(state.preferences.effective().theme, "high-contrast");
            key(&mut state, KeyCode::Down);
            assert!(content(&state).contains("Locked by command line"));
            control(&mut state, 'r');
            assert!(content(&state).contains("Locked by command line"));
            key(&mut state, KeyCode::Esc);
            state.action(5);
            assert!(!content(&state).contains("Locked by command line"));
        }
    }

    #[test]
    fn animation_only_frames_match_full_render_and_preserve_overlay() {
        use fluzo_core::application::*;
        let mut state = shell();
        state.paste("draft\nsecond");
        state.snapshot.tasks.push(TaskSnapshot {
            id: TaskId(1),
            session: SessionId(1),
            workspace: WorkspaceId(1),
            version: 1,
            state: TaskState::Running,
            reason: None,
            attempt: None,
            operation: None,
            verification: VerificationState::NotRun,
            active_milliseconds: 0,
            title: "Synthetic".into(),
        });
        state.synthetic_fragment_tick(0);
        for overlay in [
            None,
            Some(Overlay::Developer),
            Some(Overlay::Theme),
            Some(Overlay::Palette),
            Some(Overlay::Help),
            Some(Overlay::Models),
            Some(Overlay::Sessions),
            Some(Overlay::Details),
            Some(Overlay::Quit),
        ] {
            state.overlay = overlay;
            for size in [(60, 16), (80, 24), (120, 40), (160, 50)] {
                let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                state.elapsed = Duration::ZERO;
                let mut geometry = RenderGeometry::default();
                terminal
                    .draw(|frame| {
                        geometry = state.render_frame(frame, true);
                    })
                    .unwrap();
                let passes = state.formatting_passes.get();
                let cached = terminal.backend().buffer().clone();
                state.elapsed = Duration::from_millis(700);
                terminal
                    .draw(|frame| {
                        frame.buffer_mut().clone_from(&cached);
                        state.render_identity(frame, true, geometry);
                        assert_eq!(state.formatting_passes.get(), passes);
                    })
                    .unwrap();
                let animated = terminal.backend().buffer().clone();
                terminal.draw(|frame| state.render(frame, true)).unwrap();
                assert_eq!(&animated, terminal.backend().buffer());
            }
        }
        state.overlay = None;
        state.action(5);
        let base = state.snapshot.clone();
        let mut simulated = base.clone();
        simulated.tasks[0].state = TaskState::Failed;
        state.set_snapshot(simulated).unwrap();
        state.close_preview();
        assert_eq!(state.snapshot, base);
    }

    #[test]
    fn developer_menu_is_conditional_searchable_and_bounded() {
        let mut state = shell();
        assert!(state.actions().contains(&6));
        state.preferences =
            Preferences::new(VisualOptions::parse(&["--no-dev-menu".into()]).unwrap()).unwrap();
        assert!(!state.actions().contains(&6));
        state.action(6);
        assert!(state.overlay.is_none());
        let mut options = VisualOptions::default();
        options.settings.dev_menu = true;
        state.preferences = Preferences::new(options).unwrap();
        assert!(state.actions().contains(&6));
        state.action(6);
        for character in "animation_fps".chars() {
            key(&mut state, KeyCode::Char(character));
        }
        assert_eq!(state.visual_keys(), vec!["tui.animation_fps"]);
        for _ in 0..100 {
            key(&mut state, KeyCode::Left);
        }
        assert_eq!(state.preferences.effective().animation_fps, 0);
        for size in [(60, 16), (80, 24), (120, 40), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
            terminal.draw(|frame| state.render(frame, false)).unwrap();
            let content: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(content.contains("Ctrl+A Apply session"));
            assert!(content.contains("Esc Revert"));
            assert!(content.contains("demo following"));
        }
        control(&mut state, 'n');
        assert!(state.next_scene);
        key(&mut state, KeyCode::Esc);
        assert!(!state.next_scene);
        assert_eq!(state.preferences.effective().animation_fps, 60);
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
    fn tab_focus_selects_latest_item_dims_editor_and_restores_draft() {
        let mut state = shell();
        state.truecolor = true;
        state.resize(Rect::new(0, 0, 120, 40));
        state.paste("keep draft");
        for index in 0..50 {
            state.stream(&format!("message {index}"));
        }
        key(&mut state, KeyCode::PageUp);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let latest = state.messages.back().unwrap().0;
        key(&mut state, KeyCode::Tab);
        assert_eq!(state.focus, Focus::Conversation);
        assert_eq!(state.selected_fragment, Some(latest));
        assert!(state.anchor.is_none());
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        let editor = state.body_layout(state.viewport)[2];
        let theme = Theme::new(state.preferences.effective(), true);
        assert_eq!(
            terminal.backend().buffer()[(editor.x + 4, editor.y + 1)].fg,
            theme.muted.fg.unwrap()
        );
        let rows = state.display_rows(85, true);
        assert!(
            rows.iter()
                .any(|row| row.source == latest && row.line.spans[0].content == "▌ ")
        );
        key(&mut state, KeyCode::Tab);
        assert_eq!(state.focus, Focus::Composer);
        assert_eq!(state.selected_fragment, Some(latest));
        assert!(state.display_rows(85, true).iter().all(|row| {
            row.line
                .spans
                .first()
                .is_none_or(|span| span.content != "▌ ")
        }));
        terminal.draw(|frame| state.render(frame, true)).unwrap();
        assert_eq!(
            terminal.backend().buffer()[(editor.x + 4, editor.y + 1)].fg,
            theme.base.fg.unwrap()
        );
        assert_eq!(state.draft.text, "keep draft");
        assert!(!state.playing);
        state.stream("newest");
        key(&mut state, KeyCode::Tab);
        assert_eq!(
            state.selected_fragment,
            Some(state.messages.back().unwrap().0)
        );
        assert!(state.mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: editor.x + 4,
                row: editor.y + 1,
                modifiers: KeyModifiers::NONE
            },
            state.viewport
        ));
        assert_eq!(state.focus, Focus::Composer);
        assert_eq!(state.draft.text, "keep draft");
    }

    #[test]
    fn footer_hints_follow_focus_activity_and_available_width() {
        for width in [60, 80, 120, 196] {
            let mut state = shell();
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            let footer = |state: &Shell, terminal: &mut Terminal<TestBackend>| {
                terminal.draw(|frame| state.render(frame, true)).unwrap();
                (0..width)
                    .map(|column| terminal.backend().buffer()[(column, 38)].symbol())
                    .collect::<String>()
            };
            let idle = footer(&state, &mut terminal);
            assert!(idle.contains("tab focus chat"));
            assert!(idle.contains("demo following"));
            assert!(!idle.contains("NORMAL"));
            assert!(!idle.contains("WORK"));
            if width >= 80 {
                assert!(idle.contains("/ or ctrl+p commands"));
            }
            assert!(!idle.contains("ctrl+j"));
            if width == 196 {
                assert!(idle.contains("shift+enter newline"));
            }
            assert!(HELP_ROWS.contains(&("Editor", "Shift+Enter", "Insert newline")));
            state.paste("draft");
            key(&mut state, KeyCode::Tab);
            assert!(footer(&state, &mut terminal).contains("tab focus editor"));
            state.action(0);
            let running = footer(&state, &mut terminal);
            assert!(running.contains("esc cancel"));
            assert!(running.contains("demo following"));
            control(&mut state, 'g');
            let modal = footer(&state, &mut terminal);
            assert!(modal.contains("esc close"));
            assert!(!modal.contains("esc cancel"));
            assert_eq!(state.draft.text, "draft");
        }
    }

    #[test]
    fn help_table_aligns_columns_without_rules_and_pages_all_shortcuts() {
        for (width, height) in [(60, 16), (80, 24), (120, 40)] {
            let mut state = shell();
            state.resize(Rect::new(0, 0, width, height));
            state.paste("keep draft");
            control(&mut state, 'g');
            let capacity = Shell::help_capacity(state.viewport);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let popup_width = (width - 4).min(70);
            let popup_height = (height - 4).min(20);
            let left = (width - popup_width) / 2 + 2;
            let top = (height - popup_height) / 2;
            for page in 0..HELP_ROWS.len().div_ceil(capacity) {
                terminal.draw(|frame| state.render(frame, false)).unwrap();
                let buffer = terminal.backend().buffer();
                for (offset, (scope, keys, action)) in HELP_ROWS
                    .iter()
                    .skip(page * capacity)
                    .take(capacity)
                    .enumerate()
                {
                    let row = top + 4 + offset as u16;
                    for (column, expected) in
                        [(left, *scope), (left + 8, *keys), (left + 29, *action)]
                    {
                        let actual: String = (column..column + expected.len() as u16)
                            .map(|column| buffer[(column, row)].symbol())
                            .collect();
                        assert_eq!(actual, expected);
                    }
                }
                assert_eq!(buffer[(left - 2, top)].symbol(), "╭");
                assert_eq!(buffer[(left + popup_width - 5, top + 1)].symbol(), "╱");
                for row in top + 2..top + popup_height - 1 {
                    for column in left..left + popup_width - 4 {
                        assert!(!matches!(
                            buffer[(column, row)].symbol(),
                            "│" | "─" | "╭" | "╰" | "┼"
                        ));
                    }
                }
                key(&mut state, KeyCode::PageDown);
            }
            assert_eq!(state.draft.text, "keep draft");
            assert_eq!(state.focus, Focus::Composer);
            control(&mut state, 'g');
            assert!(state.overlay.is_none());
        }
    }

    #[test]
    fn crush_shortcuts_respect_context_and_preserve_demo_authority() {
        let mut state = shell();
        state.resize(Rect::new(0, 0, 120, 40));
        key(&mut state, KeyCode::Char('/'));
        assert_eq!(state.overlay, Some(Overlay::Palette));
        key(&mut state, KeyCode::Esc);
        state.paste("first");
        control(&mut state, 'j');
        state.paste("second");
        assert_eq!(state.draft.text, "first\nsecond");
        let count = state.messages.len();
        key(&mut state, KeyCode::Enter);
        assert_eq!(state.messages.len(), count + 1);
        assert!(!state.playing);
        assert_eq!(state.draft.text, "first\nsecond");
        control(&mut state, 's');
        assert_eq!(state.overlay, Some(Overlay::Sessions));
        key(&mut state, KeyCode::Esc);
        control(&mut state, 'l');
        assert_eq!(state.overlay, Some(Overlay::Models));
        key(&mut state, KeyCode::Esc);
        control(&mut state, 'g');
        assert_eq!(state.overlay, Some(Overlay::Help));
        control(&mut state, 'g');
        assert!(state.overlay.is_none());
        control(&mut state, 'b');
        assert!(state.compact(state.viewport));
        assert_eq!(state.body_area(state.viewport).width, 118);
        control(&mut state, 'b');
        assert!(!state.compact(state.viewport));
        state.action(0);
        key(&mut state, KeyCode::Esc);
        assert!(!state.playing);
        assert!(!state.quit);
        control(&mut state, 'c');
        assert_eq!(state.overlay, Some(Overlay::Quit));
        key(&mut state, KeyCode::Esc);
        state.key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert!(state.overlay.is_none());
        assert!(!state.quit);
    }

    #[test]
    fn crush_chat_navigation_and_editor_history_have_distinct_keys() {
        let mut state = shell();
        state.resize(Rect::new(0, 0, 120, 40));
        state.paste("previous");
        key(&mut state, KeyCode::Enter);
        state.draft = Editor::default();
        state.paste("draft");
        key(&mut state, KeyCode::Up);
        assert_eq!(state.draft.text, "previous");
        key(&mut state, KeyCode::Down);
        assert_eq!(state.draft.text, "draft");
        for index in 0..100 {
            state.stream(&format!("line {index}"));
        }
        key(&mut state, KeyCode::Tab);
        key(&mut state, KeyCode::Char('g'));
        assert_eq!(state.anchor, Some(state.first_line));
        key(&mut state, KeyCode::Char('j'));
        assert_eq!(state.anchor, Some(state.first_line + 1));
        key(&mut state, KeyCode::Char('k'));
        assert_eq!(state.anchor, Some(state.first_line));
        key(&mut state, KeyCode::Char('J'));
        assert!(state.selected_fragment.is_some());
        key(&mut state, KeyCode::Char('G'));
        assert!(state.anchor.is_none());
        assert_eq!(state.draft.text, "draft");
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
        key(&mut state, KeyCode::Esc);
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
        assert!(!rows[2].contains("Commands"));
        assert!(rows[3].contains("Commands ╱"));
        assert!(rows[4].contains("Search commands..."));
        for (position, action) in ACTIONS.iter().take(6).enumerate() {
            assert!(rows[5 + position].contains(action));
        }
        assert!(rows[12].contains("Enter run"));
        for character in "missing action".chars() {
            key(&mut state, KeyCode::Char(character));
        }
        key(&mut state, KeyCode::Enter);
        assert_eq!(state.overlay, Some(Overlay::Palette));
        assert!(!state.playing);
        assert!(!state.quit);
    }

    #[test]
    fn dialog_frames_keep_theme_fallbacks_and_read_only_content() {
        for (color, ascii, contrast) in [
            (true, false, false),
            (false, true, false),
            (true, false, true),
        ] {
            let mut state = shell();
            state.truecolor = true;
            state.preferences.ascii = ascii;
            if contrast {
                state.action(5);
                key(&mut state, KeyCode::Right);
                control(&mut state, 'a');
            }
            for (width, height) in [(60, 16), (80, 24), (120, 40), (240, 80)] {
                for (overlay, expected) in [
                    (Overlay::Palette, "Play synthetic stream"),
                    (Overlay::Models, "model selection is not implemented"),
                    (Overlay::Sessions, "Persistent sessions are not implemented"),
                    (Overlay::Details, "No runtime connected"),
                    (Overlay::Quit, "Keep editing"),
                ] {
                    state.overlay = Some(overlay);
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal.draw(|frame| state.render(frame, color)).unwrap();
                    let buffer = terminal.backend().buffer();
                    let popup = state.overlay_area(buffer.area);
                    assert_eq!(
                        buffer[(popup.x, popup.y)].symbol(),
                        if ascii { "+" } else { "╭" }
                    );
                    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
                    assert!(text.contains(expected));
                    if overlay != Overlay::Quit
                        && !(overlay == Overlay::Details && state.compact(buffer.area))
                    {
                        let diagonal = popup.right() - 3;
                        assert_eq!(
                            buffer[(diagonal, popup.y + 1)].symbol(),
                            if ascii { "/" } else { "╱" }
                        );
                        if color && !contrast {
                            assert_eq!(
                                buffer[(diagonal, popup.y + 1)].fg,
                                ratatui::style::Color::Rgb(255, 96, 255)
                            );
                        }
                    } else if overlay == Overlay::Quit && color && !contrast {
                        assert!(
                            buffer
                                .content
                                .iter()
                                .any(|cell| cell.bg == ratatui::style::Color::Rgb(255, 96, 255))
                        );
                    }
                    if !color {
                        for row in popup.y..popup.bottom() {
                            for column in popup.x..popup.right() {
                                let cell = &buffer[(column, row)];
                                assert!(cell.symbol().is_ascii());
                                assert_eq!(cell.fg, ratatui::style::Color::Reset);
                                assert_eq!(cell.bg, ratatui::style::Color::Reset);
                            }
                        }
                    }
                    if matches!(
                        overlay,
                        Overlay::Models | Overlay::Sessions | Overlay::Details
                    ) {
                        key(&mut state, KeyCode::Enter);
                        assert!(!state.playing);
                        assert!(!state.quit);
                        assert_eq!(state.overlay, Some(overlay));
                        key(&mut state, KeyCode::Esc);
                        assert!(state.overlay.is_none());
                    }
                }
            }
        }
    }

    #[test]
    fn dialog_composition_preserves_search_cursor_and_safe_quit() {
        for (width, height) in [(60, 16), (80, 24), (120, 40), (240, 80)] {
            let mut state = shell();
            state.resize(Rect::new(0, 0, width, height));
            state.paste("keep draft");
            let mut options = VisualOptions::default();
            options.settings.dev_menu = true;
            state.preferences = Preferences::new(options).unwrap();
            state.truecolor = true;
            control(&mut state, 'p');
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let content: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            for action in ACTIONS {
                assert!(
                    content.contains(action),
                    "missing {action} at {width}x{height}"
                );
            }
            for character in "界e\u{301}".repeat(20).chars() {
                key(&mut state, KeyCode::Char(character));
            }
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            assert_eq!(
                terminal.backend().buffer()[(cursor.x, cursor.y)].symbol(),
                " "
            );
            assert!(cursor.x < width - 3);
            let mut geometry = RenderGeometry::default();
            terminal
                .draw(|frame| {
                    geometry = state.render_frame(frame, true);
                })
                .unwrap();
            let cached = terminal.backend().buffer().clone();
            terminal
                .draw(|frame| {
                    frame.buffer_mut().clone_from(&cached);
                    state.render_identity(frame, true, geometry);
                })
                .unwrap();
            assert_eq!(terminal.get_cursor_position().unwrap(), cursor);
            let content: String = cached.content.iter().map(|cell| cell.symbol()).collect();
            assert!(content.contains("No matching actions"));
            key(&mut state, KeyCode::Enter);
            assert!(!state.playing);
            key(&mut state, KeyCode::Esc);
            control(&mut state, 'c');
            terminal.draw(|frame| state.render(frame, true)).unwrap();
            let buffer = terminal.backend().buffer();
            let question_row = (0..height)
                .find(|row| {
                    let text: String = (0..width)
                        .map(|column| buffer[(column, *row)].symbol())
                        .collect();
                    text.contains("Discard unsaved demo draft?")
                })
                .unwrap();
            let button_row: String = (0..width)
                .map(|column| buffer[(column, question_row + 2)].symbol())
                .collect();
            assert!(button_row.contains("Keep editing"));
            assert!(button_row.contains("Discard and exit"));
            state.paste("\t\n");
            state.key(KeyEvent::new_with_kind(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyEventKind::Repeat,
            ));
            assert!(!state.quit);
            key(&mut state, KeyCode::Enter);
            assert!(state.overlay.is_none());
            assert!(!state.quit);
            assert_eq!(state.draft.text, "keep draft");
            control(&mut state, 'c');
            key(&mut state, KeyCode::Tab);
            key(&mut state, KeyCode::Enter);
            assert!(state.quit);
        }
    }

    #[test]
    fn renders_supported_viewports_and_small_notice_without_controls() {
        for (width, height) in [(80, 24), (120, 40), (160, 50), (240, 80), (60, 16), (30, 8)] {
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
                assert!(content.contains("Unicode"));
                assert!(content.contains("demo following"));
            } else {
                assert!(content.contains("resize"));
            }
        }
    }
}
