use std::collections::BTreeSet;
use std::time::Duration;

use fluzo_core::application::TaskState;
use fluzo_core::settings::{SettingDescriptor, SettingOrigin, SettingValue, Settings, TuiSettings};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

pub const VISUAL_KEYS: [&str; 4] = [
    "tui.theme",
    "tui.animation_fps",
    "tui.reduced_motion",
    "tui.flags.render_diagnostics",
];

#[derive(Clone, Debug, Default)]
pub struct VisualOptions {
    pub settings: TuiSettings,
    pub locked: BTreeSet<String>,
    pub ascii: bool,
    pub repository: String,
}

impl VisualOptions {
    pub fn parse(arguments: &[String]) -> Result<Self, &'static str> {
        let mut options = Self::default();
        let mut arguments = arguments.iter();
        let mut disable_menu = false;
        while let Some(argument) = arguments.next() {
            let key = match argument.as_str() {
                "--animation-fps" => {
                    options.settings.animation_fps = arguments
                        .next()
                        .and_then(|value| value.parse().ok())
                        .filter(|value| *value <= 60)
                        .ok_or("--animation-fps requires an integer from 0 through 60.")?;
                    "tui.animation_fps"
                }
                "--theme" => {
                    options.settings.theme = arguments
                        .next()
                        .cloned()
                        .ok_or("--theme requires default or high-contrast.")?;
                    "tui.theme"
                }
                "--reduced-motion" => {
                    options.settings.reduced_motion = true;
                    "tui.reduced_motion"
                }
                "--dev-menu" => {
                    options.settings.dev_menu = true;
                    "tui.dev_menu"
                }
                "--no-dev-menu" => {
                    disable_menu = true;
                    "tui.dev_menu"
                }
                "--desktop-notifications" => {
                    options.settings.notifications.desktop_enabled = true;
                    "tui.notifications.desktop_enabled"
                }
                "--ascii" => {
                    options.ascii = true;
                    continue;
                }
                _ => return Err("Unknown visual option; use --help for supported demo options."),
            };
            options.locked.insert(key.to_owned());
        }
        if disable_menu {
            options.settings.dev_menu = false;
        }
        validate(&options.settings)?;
        Ok(options)
    }
}

fn validate(settings: &TuiSettings) -> Result<(), &'static str> {
    Settings {
        tui: settings.clone(),
        ..Settings::default()
    }
    .validate()
    .map_err(
        |_| "Invalid visual settings: FPS must be 0..60; theme must be default or high-contrast.",
    )
}

pub struct Preferences {
    applied: TuiSettings,
    draft: Option<TuiSettings>,
    locked: BTreeSet<String>,
    session_keys: BTreeSet<String>,
    pub ascii: bool,
    pub version: u64,
}

impl Preferences {
    pub fn new(options: VisualOptions) -> Result<Self, &'static str> {
        validate(&options.settings)?;
        Ok(Self {
            applied: options.settings,
            draft: None,
            locked: options.locked,
            session_keys: BTreeSet::new(),
            ascii: options.ascii,
            version: 0,
        })
    }

    pub fn effective(&self) -> &TuiSettings {
        self.draft.as_ref().unwrap_or(&self.applied)
    }

    pub fn begin(&mut self) {
        self.draft = Some(self.applied.clone());
    }

    pub fn cancel(&mut self) {
        self.draft = None;
    }

    pub fn apply(&mut self) {
        if let Some(draft) = self.draft.take() {
            for key in VISUAL_KEYS {
                if value(&draft, key) != value(&self.applied, key) {
                    self.session_keys.insert(key.to_owned());
                }
            }
            self.applied = draft;
            self.version = self.version.saturating_add(1);
        }
    }

    pub fn source(&self, key: &str) -> SettingOrigin {
        if self.locked.contains(key) {
            SettingOrigin::CommandLine
        } else if value(self.effective(), key) != value(&self.applied, key) {
            SettingOrigin::Draft
        } else if self.session_keys.contains(key) {
            SettingOrigin::ActiveSnapshot
        } else {
            SettingOrigin::BuiltIn
        }
    }

    pub fn change(&mut self, key: &str, direction: i32) -> Result<(), &'static str> {
        if self.locked.contains(key) {
            return Err("Locked by command line; preview cannot override it.");
        }
        let draft = self
            .draft
            .as_mut()
            .ok_or("Open a preview before editing.")?;
        match key {
            "tui.theme" => {
                draft.theme = if draft.theme == "default" {
                    "high-contrast"
                } else {
                    "default"
                }
                .into()
            }
            "tui.animation_fps" => {
                draft.animation_fps = draft.animation_fps.saturating_add_signed(direction).min(60)
            }
            "tui.reduced_motion" => draft.reduced_motion = !draft.reduced_motion,
            "tui.flags.render_diagnostics" => {
                draft.flags.render_diagnostics = !draft.flags.render_diagnostics
            }
            _ => return Err("Unknown or unavailable presentation setting."),
        }
        validate(draft)
    }

    pub fn reset_theme(&mut self) {
        if !self.locked.contains("tui.theme")
            && let Some(draft) = self.draft.as_mut()
        {
            draft.theme = TuiSettings::default().theme;
        }
    }

    pub fn reset(&mut self) {
        let defaults = TuiSettings::default();
        if let Some(draft) = self.draft.as_mut() {
            if !self.locked.contains("tui.theme") {
                draft.theme = defaults.theme;
            }
            if !self.locked.contains("tui.animation_fps") {
                draft.animation_fps = defaults.animation_fps;
            }
            if !self.locked.contains("tui.reduced_motion") {
                draft.reduced_motion = defaults.reduced_motion;
            }
            if !self.locked.contains("tui.flags.render_diagnostics") {
                draft.flags = defaults.flags;
            }
        }
    }

    pub fn entries(&self) -> Vec<(SettingDescriptor, SettingValue)> {
        Settings {
            tui: self.effective().clone(),
            ..Settings::default()
        }
        .entries()
        .into_iter()
        .filter(|(descriptor, _)| VISUAL_KEYS.contains(&descriptor.key.as_str()))
        .collect()
    }
}

fn value(settings: &TuiSettings, key: &str) -> SettingValue {
    match key {
        "tui.theme" => SettingValue::Text(settings.theme.clone()),
        "tui.animation_fps" => SettingValue::Integer(u64::from(settings.animation_fps)),
        "tui.reduced_motion" => SettingValue::Boolean(settings.reduced_motion),
        "tui.flags.render_diagnostics" => SettingValue::Boolean(settings.flags.render_diagnostics),
        _ => SettingValue::Unset,
    }
}

pub fn display_value(value: &SettingValue) -> String {
    match value {
        SettingValue::Text(value) => value.clone(),
        SettingValue::Integer(value) => value.to_string(),
        SettingValue::Boolean(value) => if *value { "on" } else { "off" }.into(),
        _ => "unavailable".into(),
    }
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub base: Style,
    pub accent: Style,
    pub muted: Style,
    pub success: Style,
    pub warning: Style,
    pub error: Style,
}

impl Theme {
    pub fn new(settings: &TuiSettings, color: bool) -> Self {
        let plain = Style::default();
        if !color {
            return Self {
                base: plain,
                accent: plain.add_modifier(Modifier::BOLD),
                muted: plain,
                success: plain.add_modifier(Modifier::BOLD),
                warning: plain.add_modifier(Modifier::UNDERLINED),
                error: plain.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            };
        }
        let contrast = settings.theme == "high-contrast";
        let base = plain
            .fg(if contrast {
                Color::White
            } else {
                Color::Rgb(236, 235, 240)
            })
            .bg(if contrast {
                Color::Black
            } else {
                Color::Rgb(32, 31, 38)
            });
        Self {
            base,
            accent: base
                .fg(if contrast {
                    Color::Yellow
                } else {
                    Color::Rgb(107, 80, 255)
                })
                .add_modifier(Modifier::BOLD),
            muted: base.fg(if contrast {
                Color::Gray
            } else {
                Color::Rgb(133, 131, 146)
            }),
            success: base.fg(if contrast {
                Color::Green
            } else {
                Color::Rgb(0, 255, 178)
            }),
            warning: base.fg(Color::Yellow),
            error: base.fg(Color::Red).add_modifier(Modifier::BOLD),
        }
    }
}

pub fn motion_active(state: TaskState, age: Duration) -> bool {
    matches!(state, TaskState::Running | TaskState::Waiting)
        || (state == TaskState::Completed && age < Duration::from_secs(1))
}

pub(crate) fn terminal_colors(buffer: &mut ratatui::buffer::Buffer, color: bool, rgb: bool) {
    if color && rgb {
        return;
    }
    let reduce = |value: Color| {
        if !color {
            return Color::Reset;
        }
        let Color::Rgb(red, green, blue) = value else {
            return value;
        };
        if red.max(green).max(blue) - red.min(green).min(blue) < 24 {
            return match (u16::from(red) + u16::from(green) + u16::from(blue)) / 3 {
                0..=47 => Color::Black,
                48..=159 => Color::DarkGray,
                160..=223 => Color::Gray,
                _ => Color::White,
            };
        }
        let palette = [
            (Color::Black, [0, 0, 0]),
            (Color::Red, [128, 0, 0]),
            (Color::Green, [0, 128, 0]),
            (Color::Yellow, [128, 128, 0]),
            (Color::Blue, [0, 0, 128]),
            (Color::Magenta, [128, 0, 128]),
            (Color::Cyan, [0, 128, 128]),
            (Color::Gray, [192, 192, 192]),
            (Color::DarkGray, [128, 128, 128]),
            (Color::LightRed, [255, 0, 0]),
            (Color::LightGreen, [0, 255, 0]),
            (Color::LightYellow, [255, 255, 0]),
            (Color::LightBlue, [0, 0, 255]),
            (Color::LightMagenta, [255, 0, 255]),
            (Color::LightCyan, [0, 255, 255]),
            (Color::White, [255, 255, 255]),
        ];
        palette
            .into_iter()
            .min_by_key(|(_, channels)| {
                [red, green, blue]
                    .into_iter()
                    .zip(channels)
                    .map(|(channel, target)| (i32::from(channel) - target).pow(2))
                    .sum::<i32>()
            })
            .map_or(Color::Reset, |(color, _)| color)
    };
    for cell in &mut buffer.content {
        let distinct = cell.fg != cell.bg;
        cell.fg = reduce(cell.fg);
        cell.bg = reduce(cell.bg);
        if color && distinct && cell.fg == cell.bg && !cell.symbol().trim().is_empty() {
            cell.fg = if cell.bg == Color::Black {
                Color::DarkGray
            } else {
                Color::Black
            };
        }
    }
}

pub fn logo(
    state: TaskState,
    age: Duration,
    settings: &TuiSettings,
    color: bool,
    ascii: bool,
) -> Vec<Line<'static>> {
    let moving =
        motion_active(state, age) && settings.animation_fps > 0 && !settings.reduced_motion;
    let mut lines = crate::identity::wordmark(
        if moving { age } else { Duration::ZERO },
        color,
        true,
        ascii,
    );
    if color && settings.theme == "high-contrast" {
        crate::identity::fire_colors(
            &mut lines,
            if moving { age } else { Duration::ZERO },
            8.0,
            true,
        );
    }
    crate::identity::state_colors(&mut lines, state, age, moving, color, true);
    lines
}

#[derive(Default)]
pub struct AnimationClock {
    next: Option<Duration>,
    interval: Option<Duration>,
    pub skipped: u64,
}

impl AnimationClock {
    pub fn tick(
        &mut self,
        now: Duration,
        state: TaskState,
        age: Duration,
        settings: &TuiSettings,
    ) -> bool {
        let interval =
            if settings.animation_fps == 0 || settings.reduced_motion || !motion_active(state, age)
            {
                None
            } else {
                Some(Duration::from_nanos(
                    1_000_000_000u64.div_ceil(u64::from(settings.animation_fps)),
                ))
            };
        let stopped = self.interval.is_some() && interval.is_none();
        if interval != self.interval {
            self.interval = interval;
            self.next = interval.map(|_| now);
        }
        let Some(interval) = interval else {
            return stopped;
        };
        let next = self.next.unwrap_or(now);
        if now < next {
            return false;
        }
        self.skipped = self.skipped.saturating_add(
            ((now - next).as_nanos() / interval.as_nanos()).min(u128::from(u64::MAX)) as u64,
        );
        self.next = Some(now.saturating_add(interval));
        true
    }

    pub fn wait(&self, now: Duration) -> Duration {
        self.next.map_or(Duration::from_millis(50), |next| {
            next.saturating_sub(now).min(Duration::from_millis(50))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_reversible_and_cannot_change_authority() {
        let mut preferences = Preferences::new(VisualOptions::default()).unwrap();
        let original = preferences.effective().clone();
        preferences.begin();
        preferences.change("tui.theme", 1).unwrap();
        assert_eq!(preferences.source("tui.theme"), SettingOrigin::Draft);
        for key in [
            "decision.enabled",
            "policy.yolo",
            "harness.max_turns",
            "tui.flags.unknown",
        ] {
            assert!(preferences.change(key, 1).is_err());
        }
        preferences.cancel();
        assert_eq!(preferences.effective(), &original);
        preferences.begin();
        preferences.change("tui.theme", 1).unwrap();
        preferences.apply();
        assert_eq!(preferences.version, 1);
        preferences.begin();
        preferences.reset();
        preferences.cancel();
        assert_eq!(preferences.effective().theme, "high-contrast");
    }

    #[test]
    fn ansi_reduction_preserves_visible_rules_and_is_idempotent() {
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 1, 1));
        buffer[(0, 0)]
            .set_symbol("|")
            .set_fg(Color::Rgb(58, 57, 67))
            .set_bg(Color::Rgb(32, 31, 38));
        terminal_colors(&mut buffer, true, false);
        assert_eq!(buffer[(0, 0)].fg, Color::DarkGray);
        assert_eq!(buffer[(0, 0)].bg, Color::Black);
        let expected = buffer.clone();
        terminal_colors(&mut buffer, true, false);
        assert_eq!(buffer, expected);
    }

    #[test]
    fn waiting_ticks_and_completion_settles_without_an_idle_loop() {
        for rate in [0, 15, 30, 60] {
            for reduced in [false, true] {
                let settings = TuiSettings {
                    animation_fps: rate,
                    reduced_motion: reduced,
                    ..TuiSettings::default()
                };
                let animated = rate > 0 && !reduced;
                let mut clock = AnimationClock::default();
                assert_eq!(
                    clock.tick(
                        Duration::ZERO,
                        TaskState::Waiting,
                        Duration::ZERO,
                        &settings
                    ),
                    animated
                );
                assert_eq!(
                    clock.tick(
                        Duration::from_secs(1),
                        TaskState::Completed,
                        Duration::ZERO,
                        &settings
                    ),
                    animated
                );
                assert_eq!(
                    clock.tick(
                        Duration::from_secs(2),
                        TaskState::Completed,
                        Duration::from_secs(1),
                        &settings
                    ),
                    animated
                );
                assert!(!clock.tick(
                    Duration::from_secs(3),
                    TaskState::Completed,
                    Duration::from_secs(2),
                    &settings
                ));
                assert_eq!(
                    clock.wait(Duration::from_secs(3)),
                    Duration::from_millis(50)
                );
            }
        }
    }

    #[test]
    fn notification_opt_in_preserves_disabled_default() {
        assert!(
            !VisualOptions::default()
                .settings
                .notifications
                .desktop_enabled
        );
        let options = VisualOptions::parse(&["--desktop-notifications".into()]).unwrap();
        assert!(options.settings.notifications.desktop_enabled);
        assert!(options.locked.contains("tui.notifications.desktop_enabled"));
    }

    #[test]
    fn cli_locks_and_invalid_values_are_enforced() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        for value in ["-1", "61", "1.5", "wat"] {
            assert!(VisualOptions::parse(&args(&["--animation-fps", value])).is_err());
        }
        assert!(VisualOptions::parse(&args(&["--theme", "missing"])).is_err());
        assert!(VisualOptions::parse(&args(&["--unknown"])).is_err());
        let options = VisualOptions::parse(&args(&[
            "--no-dev-menu",
            "--dev-menu",
            "--animation-fps",
            "15",
        ]))
        .unwrap();
        assert!(!options.settings.dev_menu);
        let mut preferences = Preferences::new(options).unwrap();
        preferences.begin();
        assert!(preferences.change("tui.animation_fps", 1).is_err());
        preferences.reset();
        assert_eq!(preferences.effective().animation_fps, 15);
    }

    #[test]
    fn clock_caps_animation_skips_backlog_and_never_ticks_idle() {
        for rate in [0, 15, 30, 60] {
            let mut settings = TuiSettings {
                animation_fps: rate,
                ..TuiSettings::default()
            };
            let mut clock = AnimationClock::default();
            let ticks = (0..1000)
                .filter(|millis| {
                    let now = Duration::from_millis(*millis);
                    clock.tick(now, TaskState::Running, now, &settings)
                })
                .count();
            assert!(ticks <= rate as usize);
            assert!(rate == 0 || ticks >= rate as usize - 1);
            settings.reduced_motion = true;
            assert_eq!(
                rate > 0,
                clock.tick(
                    Duration::from_secs(2),
                    TaskState::Running,
                    Duration::ZERO,
                    &settings
                )
            );
            settings.reduced_motion = false;
            assert!(!clock.tick(
                Duration::from_secs(3),
                TaskState::Pending,
                Duration::ZERO,
                &settings
            ));
        }
        let mut clock = AnimationClock::default();
        let settings = TuiSettings::default();
        assert!(clock.tick(
            Duration::ZERO,
            TaskState::Running,
            Duration::ZERO,
            &settings
        ));
        assert!(clock.tick(
            Duration::from_secs(10),
            TaskState::Running,
            Duration::from_secs(10),
            &settings
        ));
        assert!(clock.skipped > 500);
        assert!(!clock.tick(
            Duration::from_secs(10),
            TaskState::Running,
            Duration::from_secs(10),
            &settings
        ));
    }

    #[test]
    fn artwork_uses_elapsed_time_not_frame_count() {
        let mut settings = TuiSettings::default();
        let expected = logo(
            TaskState::Running,
            Duration::from_millis(500),
            &settings,
            true,
            false,
        );
        for rate in [15, 30, 60] {
            settings.animation_fps = rate;
            assert_eq!(
                logo(
                    TaskState::Running,
                    Duration::from_millis(500),
                    &settings,
                    true,
                    false
                ),
                expected
            );
        }
        settings.animation_fps = 0;
        assert_eq!(
            logo(TaskState::Running, Duration::ZERO, &settings, true, false),
            logo(
                TaskState::Running,
                Duration::from_secs(10),
                &settings,
                true,
                false
            )
        );
        for state in [
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Cancelled,
        ] {
            assert!(!motion_active(state, Duration::from_secs(1)));
        }
    }
}
