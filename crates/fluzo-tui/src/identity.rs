use std::time::Duration;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

const LETTERS: [[&str; 5]; 5] = [
    ["11111", "10000", "11110", "10000", "10000"],
    ["10000", "10000", "10000", "10000", "11111"],
    ["10001", "10001", "10001", "10001", "01110"],
    ["11111", "00010", "00100", "01000", "11111"],
    ["01110", "10001", "10001", "10001", "01110"],
];

fn ink(column: i32, row: i32) -> bool {
    if !(0..30).contains(&column) || !(0..5).contains(&row) || column % 6 == 5 {
        return false;
    }
    LETTERS[column as usize / 6][row as usize].as_bytes()[column as usize % 6] == b'1'
}

pub(crate) fn gradient(position: f64, seconds: f64, period: f64, rgb: bool) -> Color {
    let fraction = (1.0 - (std::f64::consts::PI * (position - seconds / period)).cos()) / 2.0;
    if !rgb {
        return if fraction < 0.33 {
            Color::Cyan
        } else if fraction < 0.66 {
            Color::Blue
        } else {
            Color::Magenta
        };
    }
    let stops = [
        [48.0, 224.0, 224.0],
        [77.0, 151.0, 249.0],
        [167.0, 113.0, 248.0],
    ];
    let segment = usize::from(fraction >= 0.5);
    let blend = fraction * 2.0 - segment as f64;
    let channels: [u8; 3] = std::array::from_fn(|index| {
        (stops[segment][index] + (stops[segment + 1][index] - stops[segment][index]) * blend)
            .round() as u8
    });
    Color::Rgb(channels[0], channels[1], channels[2])
}

pub(crate) fn fire_gradient(position: f64, seconds: f64, period: f64, rgb: bool) -> Color {
    let fraction = (1.0 - (std::f64::consts::PI * (position - seconds / period)).cos()) / 2.0;
    if !rgb {
        return if fraction < 0.33 {
            Color::LightRed
        } else if fraction < 0.66 {
            Color::Yellow
        } else {
            Color::LightYellow
        };
    }
    Color::Rgb(
        255,
        (80.0 + 175.0 * fraction).round() as u8,
        (48.0 * (1.0 - fraction)).round() as u8,
    )
}

pub(crate) fn fire_colors(lines: &mut [Line<'static>], age: Duration, period: f64, rgb: bool) {
    for line in lines {
        let width = line.width().saturating_sub(1).max(1);
        let mut column = 0;
        for span in &mut line.spans {
            let tint = fire_gradient(column as f64 / width as f64, age.as_secs_f64(), period, rgb);
            column += span.width();
            if span.content.trim().is_empty() {
                continue;
            }
            let recolor = |original: Color| {
                let shadow = matches!(original, Color::Rgb(red, green, blue) if red.max(green).max(blue) < 100)
                    || original == Color::DarkGray;
                match tint {
                    Color::Rgb(red, green, blue) if shadow => Color::Rgb(
                        (f64::from(red) * 0.32).round() as u8,
                        (f64::from(green) * 0.32).round() as u8,
                        (f64::from(blue) * 0.32).round() as u8,
                    ),
                    _ if shadow => Color::Red,
                    _ => tint,
                }
            };
            span.style.fg = Some(recolor(span.style.fg.unwrap_or(Color::White)));
            if let Some(background) = span.style.bg {
                span.style.bg = Some(recolor(background));
            }
        }
    }
}

fn pixel(column: i32, row: i32, elapsed: Duration, rgb: bool) -> Option<Color> {
    let color = gradient(f64::from(column) / 29.0, elapsed.as_secs_f64(), 8.0, rgb);
    if ink(column, row) {
        Some(color)
    } else if ink(column - 1, row - 1) {
        Some(match color {
            Color::Rgb(red, green, blue) => Color::Rgb(
                (f64::from(red) * 0.32).round() as u8,
                (f64::from(green) * 0.32).round() as u8,
                (f64::from(blue) * 0.32).round() as u8,
            ),
            _ => Color::DarkGray,
        })
    } else {
        None
    }
}

pub fn compact_wordmark(elapsed: Duration, color: bool, rgb: bool) -> Line<'static> {
    Line::from(
        "FLUZO"
            .chars()
            .enumerate()
            .map(|(index, letter)| {
                let style = if color {
                    Style::default().fg(gradient(
                        (index * 6 + 2) as f64 / 29.0,
                        elapsed.as_secs_f64(),
                        8.0,
                        rgb,
                    ))
                } else {
                    Style::default()
                };
                Span::styled(letter.to_string(), style)
            })
            .collect::<Vec<_>>(),
    )
}

pub fn wordmark(elapsed: Duration, color: bool, rgb: bool, ascii: bool) -> Vec<Line<'static>> {
    if ascii {
        return vec![Line::from("FLUZO"), Line::from(""), Line::from("")];
    }
    (0..6)
        .step_by(2)
        .map(|row| {
            let spans: Vec<_> = (0..30)
                .map(|column| {
                    let top = pixel(column, row, elapsed, rgb);
                    let bottom = pixel(column, row + 1, elapsed, rgb);
                    let (glyph, foreground, background) = match (top, bottom) {
                        (None, None) => (" ", Color::Reset, None),
                        (Some(top), None) => ("▀", top, None),
                        (None, Some(bottom)) => ("▄", bottom, None),
                        (Some(top), Some(bottom)) if top == bottom => ("█", top, None),
                        (Some(top), Some(bottom)) => ("▀", top, Some(bottom)),
                    };
                    if color {
                        Span::styled(
                            glyph,
                            Style {
                                bg: background,
                                ..Style::default().fg(foreground)
                            },
                        )
                    } else {
                        let glyph = match (ink(column, row), ink(column, row + 1)) {
                            (false, false) => " ",
                            (true, false) => "▀",
                            (false, true) => "▄",
                            (true, true) => "█",
                        };
                        Span::raw(glyph)
                    }
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

pub(crate) fn state_colors(
    lines: &mut [Line<'static>],
    state: fluzo_core::application::TaskState,
    age: Duration,
    moving: bool,
    color: bool,
    rgb: bool,
) {
    use fluzo_core::application::TaskState;
    if !color {
        return;
    }
    let seconds = if moving { age.as_secs_f64() } else { 0.0 };
    for line in lines {
        let width = line.width().max(1);
        let mut column = 0;
        for span in &mut line.spans {
            let position = column as f64 / width.saturating_sub(1).max(1) as f64;
            column += span.width();
            if span.content.trim().is_empty() {
                continue;
            }
            let (tint, strength) = match state {
                TaskState::Pending | TaskState::Ready => (None, 0.55),
                TaskState::Running => {
                    let distance = (position - 0.5).abs() * 2.0;
                    let pulse = (std::f64::consts::TAU * (distance + seconds / 1.6)).cos();
                    (None, if moving { 0.75 + 0.25 * pulse } else { 1.0 })
                }
                TaskState::Waiting => (
                    Some((255, 190, 80)),
                    0.75 + 0.15 * (std::f64::consts::TAU * seconds / 3.0).cos(),
                ),
                TaskState::Blocked | TaskState::Stopping => (Some((230, 175, 60)), 1.0),
                TaskState::Completed => (
                    Some((0, 255, 178)),
                    if moving {
                        1.0 - 0.35 * seconds.min(1.0)
                    } else {
                        0.65
                    },
                ),
                TaskState::Failed => (Some((255, 90, 90)), 1.0),
                TaskState::Cancelled => (Some((133, 131, 146)), 1.0),
            };
            let recolor = |original: Color, shadow: bool| {
                if !rgb {
                    if shadow {
                        return Color::DarkGray;
                    }
                    return match state {
                        TaskState::Pending | TaskState::Ready | TaskState::Cancelled => {
                            Color::DarkGray
                        }
                        TaskState::Waiting | TaskState::Blocked | TaskState::Stopping => {
                            Color::Yellow
                        }
                        TaskState::Completed => Color::Green,
                        TaskState::Failed => Color::Red,
                        TaskState::Running => original,
                    };
                }
                let channels = tint.or(match original {
                    Color::Rgb(red, green, blue) => Some((red, green, blue)),
                    _ => None,
                });
                channels.map_or(original, |(red, green, blue)| {
                    let factor = strength * if tint.is_some() && shadow { 0.32 } else { 1.0 };
                    Color::Rgb(
                        (f64::from(red) * factor).round() as u8,
                        (f64::from(green) * factor).round() as u8,
                        (f64::from(blue) * factor).round() as u8,
                    )
                })
            };
            let foreground = span
                .style
                .fg
                .unwrap_or_else(|| gradient(position, 0.0, 8.0, rgb));
            let shadow = matches!(foreground, Color::Rgb(red, green, blue) if red.max(green).max(blue) < 100)
                || foreground == Color::DarkGray;
            span.style.fg = Some(recolor(foreground, shadow));
            if let Some(background) = span.style.bg {
                let shadow = matches!(background, Color::Rgb(red, green, blue) if red.max(green).max(blue) < 100)
                    || background == Color::DarkGray;
                span.style.bg = Some(recolor(background, shadow));
            }
        }
    }
}

pub fn geometry(width: u16, elapsed: Duration, moving: bool) -> (u16, u16) {
    let length = (u32::from(width) * 4 / 5) as u16;
    let travel = width - length;
    let offset = if moving {
        (f64::from(travel) * (1.0 - (std::f64::consts::TAU * elapsed.as_secs_f64() / 0.8).cos())
            / 2.0)
            .round() as u16
    } else {
        travel / 2
    };
    (offset, length)
}

pub fn activity(
    width: u16,
    elapsed: Duration,
    working: bool,
    moving: bool,
    color: bool,
    rgb: bool,
    ascii: bool,
) -> Line<'static> {
    let (offset, length) = geometry(width, elapsed, moving);
    let spans: Vec<_> = (0..width)
        .map(|column| {
            if column < offset || column >= offset + length {
                return Span::raw(" ");
            }
            let glyph = if ascii { "-" } else { "▔" };
            if !color {
                return Span::raw(glyph);
            }
            let tint = if working {
                gradient(
                    f64::from(column - offset) / f64::from(length.saturating_sub(1).max(1)),
                    if moving { elapsed.as_secs_f64() } else { 0.0 },
                    0.3,
                    rgb,
                )
            } else {
                Color::DarkGray
            };
            Span::styled(glyph, Style::default().fg(tint))
        })
        .collect();
    Line::from(spans)
}

pub fn working_label(elapsed: Duration, moving: bool, color: bool, rgb: bool) -> Span<'static> {
    let seconds = if moving {
        elapsed.as_secs_f64() % 2.4
    } else {
        0.0
    };
    let style = if color {
        Style::default().fg(gradient(0.5, seconds, 1.2, rgb))
    } else {
        Style::default()
    };
    Span::styled("  Working (demo)", style)
}

pub fn message_wave(
    elapsed: Duration,
    moving: bool,
    color: bool,
    rgb: bool,
    ascii: bool,
) -> Line<'static> {
    let time = if moving {
        elapsed.as_secs_f64() % 0.6
    } else {
        0.0
    };
    let blocks = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let plain = [".", "_", "_", "=", "=", "#", "#", "#"];
    let spans: Vec<_> = (0..15)
        .map(|column| {
            if column % 2 == 1 {
                return Span::raw(" ");
            }
            let phase = std::f64::consts::TAU * (f64::from(column) / 14.0 - time / 0.6);
            let level = (((phase.sin() + 1.0) * 3.5).round() as usize).min(7);
            let glyph = if ascii { plain[level] } else { blocks[level] };
            if color {
                Span::styled(
                    glyph,
                    Style::default().fg(gradient(f64::from(column) / 14.0, time, 0.6, rgb)),
                )
            } else {
                Span::raw(glyph)
            }
        })
        .collect();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_tints_preserve_relief_geometry_and_static_fallbacks() {
        use fluzo_core::application::TaskState;
        let original = wordmark(Duration::ZERO, true, true, false);
        for state in [
            TaskState::Pending,
            TaskState::Ready,
            TaskState::Running,
            TaskState::Waiting,
            TaskState::Blocked,
            TaskState::Stopping,
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Cancelled,
        ] {
            for rgb in [false, true] {
                for ascii in [false, true] {
                    let mut first = wordmark(Duration::ZERO, true, rgb, ascii);
                    let mut later = first.clone();
                    state_colors(&mut first, state, Duration::ZERO, false, true, rgb);
                    state_colors(&mut later, state, Duration::from_secs(10), false, true, rgb);
                    assert_eq!(first, later);
                    assert_eq!(first.len(), 3);
                    assert_eq!(first[0].width(), if ascii { 5 } else { 30 });
                    if !rgb {
                        assert!(
                            first
                                .iter()
                                .flat_map(|line| &line.spans)
                                .all(|span| !matches!(span.style.fg, Some(Color::Rgb(..)))
                                    && !matches!(span.style.bg, Some(Color::Rgb(..))))
                        );
                    }
                }
            }
            let mut plain = wordmark(Duration::ZERO, false, false, false);
            let unchanged = plain.clone();
            state_colors(
                &mut plain,
                state,
                Duration::from_millis(500),
                true,
                false,
                false,
            );
            assert_eq!(plain, unchanged);
        }
        let mut completed = original.clone();
        state_colors(
            &mut completed,
            TaskState::Completed,
            Duration::ZERO,
            true,
            true,
            true,
        );
        for (before, after) in original
            .iter()
            .flat_map(|line| &line.spans)
            .zip(completed.iter().flat_map(|line| &line.spans))
        {
            assert_eq!(before.content, after.content);
            if let Some(Color::Rgb(red, green, blue)) = before.style.bg {
                let expected = if red.max(green).max(blue) < 100 {
                    Color::Rgb(0, 82, 57)
                } else {
                    Color::Rgb(0, 255, 178)
                };
                assert_eq!(after.style.bg, Some(expected));
            }
        }
        for state in [TaskState::Running, TaskState::Waiting, TaskState::Completed] {
            let mut first = original.clone();
            let mut later = original.clone();
            state_colors(&mut first, state, Duration::ZERO, true, true, true);
            state_colors(
                &mut later,
                state,
                Duration::from_millis(750),
                true,
                true,
                true,
            );
            assert_ne!(first, later);
        }
    }

    #[test]
    fn compact_logo_samples_the_large_logo_palette_and_phase() {
        for rgb in [false, true] {
            for seconds in [0, 2, 8, 16] {
                let elapsed = Duration::from_secs(seconds);
                let small = compact_wordmark(elapsed, true, rgb);
                assert_eq!(small.width(), 5);
                assert_eq!(
                    small
                        .spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>(),
                    "FLUZO"
                );
                for (index, span) in small.spans.iter().enumerate() {
                    let column = (index * 6 + 2) as i32;
                    let row = (0..5).find(|row| ink(column, *row)).unwrap();
                    assert_eq!(span.style.fg, pixel(column, row, elapsed, rgb));
                    assert_eq!(span.style.bg, None);
                }
            }
        }
        assert_eq!(
            compact_wordmark(Duration::ZERO, false, true),
            compact_wordmark(Duration::from_secs(4), false, true)
        );
        assert_ne!(
            compact_wordmark(Duration::ZERO, true, true),
            compact_wordmark(Duration::from_secs(4), true, true)
        );
    }

    #[test]
    fn working_label_cycles_color_without_changing_text_or_disabled_motion() {
        let initial = working_label(Duration::ZERO, true, true, true);
        let later = working_label(Duration::from_millis(600), true, true, true);
        assert_eq!(initial.content, later.content);
        assert_ne!(initial.style.fg, later.style.fg);
        assert_eq!(
            initial,
            working_label(Duration::from_millis(2400), true, true, true)
        );
        assert_eq!(
            initial,
            working_label(Duration::from_secs(9), false, true, true)
        );
        assert_eq!(
            working_label(Duration::ZERO, true, false, true),
            working_label(Duration::from_secs(9), true, false, true)
        );
        assert!(!matches!(
            working_label(Duration::ZERO, true, true, false).style.fg,
            Some(Color::Rgb(..))
        ));
    }

    #[test]
    fn message_wave_has_fixed_width_elapsed_motion_and_static_fallbacks() {
        let initial = message_wave(Duration::ZERO, true, false, false, false);
        assert_eq!(initial.width(), 15);
        assert_ne!(
            initial,
            message_wave(Duration::from_millis(300), true, false, false, false)
        );
        assert_eq!(
            initial,
            message_wave(Duration::from_millis(600), true, false, false, false)
        );
        for ascii in [false, true] {
            assert_eq!(
                message_wave(Duration::ZERO, false, true, true, ascii),
                message_wave(Duration::from_secs(99), false, true, true, ascii)
            );
            let line = message_wave(Duration::from_millis(300), true, true, false, ascii);
            assert_eq!(line.width(), 15);
            for (column, span) in line.spans.iter().enumerate() {
                assert_eq!(span.content == " ", column % 2 == 1);
            }
            assert!(
                line.spans
                    .iter()
                    .all(|span| !matches!(span.style.fg, Some(Color::Rgb(..))))
            );
            if ascii {
                assert!(line.spans.iter().all(|span| span.content.is_ascii()));
            }
        }
    }

    #[test]
    fn eighty_percent_bar_reaches_both_edges_without_overflow() {
        for width in [0, 1, 60, 80, 120, 160] {
            let length = width * 4 / 5;
            assert_eq!(geometry(width, Duration::ZERO, true), (0, length));
            assert_eq!(
                geometry(width, Duration::from_millis(400), true),
                (width - length, length)
            );
            assert_eq!(
                geometry(width, Duration::from_millis(800), true),
                (0, length)
            );
            for millis in 0..800 {
                let (offset, actual) = geometry(width, Duration::from_millis(millis), true);
                assert_eq!(actual, length);
                assert!(offset + actual <= width);
            }
        }
    }

    #[test]
    fn idle_and_disabled_motion_are_static() {
        for working in [false, true] {
            assert_eq!(
                activity(80, Duration::ZERO, working, false, true, true, false),
                activity(
                    80,
                    Duration::from_secs(7),
                    working,
                    false,
                    true,
                    true,
                    false
                )
            );
        }
        assert_ne!(
            wordmark(Duration::ZERO, true, true, false),
            wordmark(Duration::from_secs(2), true, true, false)
        );
    }

    #[test]
    fn transparent_void_and_terminal_fallbacks_preserve_shape() {
        let artwork = wordmark(Duration::ZERO, true, true, false);
        assert_eq!(artwork.len(), 3);
        for row in &artwork {
            assert_eq!(row.width(), 30);
            for span in &row.spans {
                if span.content == " " {
                    assert_eq!(span.style.bg, None);
                }
            }
        }
        for row in wordmark(Duration::ZERO, true, false, false) {
            assert!(
                row.spans
                    .iter()
                    .all(|span| !matches!(span.style.fg, Some(Color::Rgb(..))))
            );
        }
        assert_eq!(
            wordmark(Duration::ZERO, false, false, true)[0],
            Line::from("FLUZO")
        );
    }
}
