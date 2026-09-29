use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::visual::Theme;

pub struct AssistantFooter {
    pub model: String,
    pub provider: String,
    pub duration: String,
}

impl AssistantFooter {
    pub fn render(&self, width: u16, theme: Theme, ascii: bool) -> Line<'static> {
        let subtle = if theme.base.bg == Some(Color::Rgb(32, 31, 38)) {
            theme.base.fg(Color::Rgb(96, 95, 107))
        } else {
            theme.muted
        }
        .remove_modifier(Modifier::BOLD);
        let separator = if theme.base.bg == Some(Color::Rgb(32, 31, 38)) {
            theme.base.fg(Color::Rgb(58, 57, 67))
        } else {
            theme.muted
        }
        .remove_modifier(Modifier::BOLD);
        let mut spans = vec![
            Span::styled(if ascii { "<> " } else { "◇ " }, subtle),
            Span::styled(
                self.model.clone(),
                theme.muted.remove_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" via {} in {}", self.provider, self.duration),
                subtle,
            ),
        ];
        let length: usize = spans.iter().map(Span::width).sum();
        let remaining = usize::from(width.saturating_sub(2)).saturating_sub(length);
        if remaining > 1 {
            spans.push(Span::styled(
                format!(" {}", if ascii { "-" } else { "─" }.repeat(remaining - 1)),
                separator,
            ));
        }
        spans.insert(0, Span::styled("  ", theme.base));
        Line::from(spans)
    }
}

pub fn demo_model_info(width: u16, theme: Theme, ascii: bool) -> Vec<Line<'static>> {
    let subtle = if theme.base.bg == Some(Color::Rgb(32, 31, 38)) {
        theme.base.fg(Color::Rgb(96, 95, 107))
    } else {
        theme.muted
    };
    let mut name = vec![
        Span::styled(if ascii { "<> " } else { "◇ " }, subtle),
        Span::styled("GTP-6 Astra", theme.base),
    ];
    let provider = Span::styled("via Github Copilot", theme.muted);
    let fits =
        name.iter().map(Span::width).sum::<usize>() + 1 + provider.width() <= usize::from(width);
    if fits {
        name.extend([Span::raw(" "), provider.clone()]);
    }
    let mut lines = wrap(name, usize::from(width));
    if !fits {
        lines.push(Line::from(vec![Span::raw("  "), provider]));
    }
    lines.push(Line::from(Span::styled("  Reasoning X-High", subtle)));
    lines.push(model_meter(width, 75, "75% (303.8K)", theme, ascii));
    lines.push(model_meter(width, 50, "$50.00", theme, ascii));
    lines.push(Line::from(Span::styled(
        "  Cost scale: $100 (demo)",
        subtle,
    )));
    lines.push(Line::from(Span::styled("  Example data / offline", subtle)));
    lines
}

fn model_meter(width: u16, percent: u16, value: &str, theme: Theme, ascii: bool) -> Line<'static> {
    let bar_width = usize::from(width)
        .saturating_sub(3 + "75% (303.8K)".width())
        .min(12);
    let filled = bar_width * usize::from(percent.min(100)) / 100;
    let mut spans = vec![Span::raw("  ")];
    for column in 0..filled {
        let style = if theme.base.bg == Some(Color::Rgb(32, 31, 38)) {
            theme.base.fg(crate::identity::gradient(
                column as f64 / filled.saturating_sub(1).max(1) as f64,
                0.0,
                1.0,
                true,
            ))
        } else if theme.base.bg == Some(Color::Black) {
            theme.base.fg(crate::identity::fire_gradient(
                column as f64 / filled.saturating_sub(1).max(1) as f64,
                0.0,
                1.0,
                true,
            ))
        } else {
            theme.success.remove_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(if ascii { "#" } else { "━" }, style));
    }
    spans.extend([
        Span::styled(
            if ascii { "-" } else { "─" }.repeat(bar_width - filled),
            theme.muted,
        ),
        Span::styled(format!(" {value}"), theme.muted),
    ]);
    Line::from(spans)
}

pub fn compact_details(
    repository: &str,
    width: u16,
    open: bool,
    theme: Theme,
    ascii: bool,
) -> Line<'static> {
    let components: Vec<_> = repository
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let path = if components.len() > 4 {
        format!(".../{}", components[components.len() - 4..].join("/"))
    } else {
        repository.to_owned()
    };
    let separator = if ascii { " * " } else { " • " };
    let text = format!(
        "{path}{separator}75%{separator}ctrl+d {}",
        if open { "close" } else { "open " }
    );
    let limit = usize::from(width);
    if text.width() <= limit {
        return Line::from(Span::styled(text, theme.muted));
    }
    let suffix = if ascii { "..." } else { "…" };
    let budget = limit.saturating_sub(suffix.width());
    let mut clipped = String::new();
    for glyph in text.graphemes(true) {
        if clipped.width() + glyph.width() > budget {
            break;
        }
        clipped.push_str(glyph);
    }
    if limit >= suffix.width() {
        clipped.push_str(suffix);
    }
    Line::from(Span::styled(clipped, theme.muted))
}

pub fn compact(area: Rect) -> bool {
    area.width < 120 || area.height < 30
}

pub fn body(area: Rect) -> Rect {
    body_mode(area, compact(area))
}

pub fn body_mode(area: Rect, compact: bool) -> Rect {
    Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(if compact { 2 } else { 34 }),
        area.height,
    )
}

pub fn layout(area: Rect, editor_rows: usize) -> [Rect; 4] {
    layout_mode(area, editor_rows, compact(area))
}

pub fn layout_mode(area: Rect, editor_rows: usize, compact: bool) -> [Rect; 4] {
    let body = body_mode(area, compact);
    let header = if compact { 3 } else { 1 };
    let editor_height =
        (editor_rows.clamp(3, 15) as u16 + 2).min(area.height.saturating_sub(header + 5));
    let status_y = area.bottom().saturating_sub(2);
    let editor_y = status_y.saturating_sub(editor_height);
    [
        Rect::new(body.x, area.y + 1, body.width, header - 1),
        Rect::new(
            body.x,
            area.y + header,
            body.width.saturating_sub(1),
            editor_y.saturating_sub(area.y + header + 1),
        ),
        Rect::new(body.x, editor_y, body.width, editor_height),
        Rect::new(area.x, status_y, area.width, 1),
    ]
}

pub fn wrap(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut used = 0;
    for span in spans {
        for word in span.content.split_word_bounds() {
            if !word.trim().is_empty() && word.width() <= width && used + word.width() > width {
                rows.push(Line::from(std::mem::take(&mut row)));
                used = 0;
            }
            for glyph in word.graphemes(true) {
                let size = glyph.width();
                if used + size > width {
                    rows.push(Line::from(std::mem::take(&mut row)));
                    used = 0;
                }
                if size > width {
                    row.push(Span::styled("?", span.style));
                    used += 1;
                } else if used != 0 || rows.is_empty() || !glyph.trim().is_empty() {
                    row.push(Span::styled(glyph.to_owned(), span.style));
                    used += size;
                }
            }
        }
    }
    rows.push(Line::from(row));
    rows
}

pub fn inline(text: &str, style: Style, theme: Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        let marker = if remaining.starts_with("**") {
            Some(("**", style.add_modifier(Modifier::BOLD)))
        } else if remaining.starts_with('`') {
            Some(("`", theme.warning))
        } else if remaining.starts_with('*') {
            Some(("*", style.add_modifier(Modifier::ITALIC)))
        } else {
            None
        };
        if let Some((delimiter, marked)) = marker
            && let Some(end) = remaining[delimiter.len()..].find(delimiter)
        {
            spans.push(Span::styled(
                remaining[delimiter.len()..delimiter.len() + end].to_owned(),
                marked,
            ));
            remaining = &remaining[end + delimiter.len() * 2..];
            continue;
        }
        let first = remaining.chars().next().unwrap().len_utf8();
        let length = remaining[first..]
            .find(['*', '`'])
            .map_or(remaining.len(), |offset| first + offset);
        spans.push(Span::styled(remaining[..length].to_owned(), style));
        remaining = &remaining[length..];
    }
    spans
}

pub fn markdown(text: &str, fenced: &mut bool, theme: Theme) -> Vec<Span<'static>> {
    if let Some(language) = text.trim_start().strip_prefix("```") {
        *fenced = !*fenced;
        return vec![Span::styled(
            if *fenced {
                format!("  {language}")
            } else {
                String::new()
            },
            theme.muted,
        )];
    }
    if *fenced {
        let style = if text.starts_with('+') {
            theme.success
        } else if text.starts_with('-') {
            theme.error
        } else {
            theme.base
        };
        return vec![Span::styled(format!("  {text}"), style)];
    }
    if text.starts_with('#')
        && let Some((_, heading)) = text.split_once(' ')
    {
        return inline(heading, theme.accent, theme);
    }
    if let Some(item) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
        let mut spans = vec![Span::styled("• ", theme.accent)];
        spans.extend(inline(item, theme.base, theme));
        return spans;
    }
    if let Some(quote) = text.strip_prefix("> ") {
        return inline(&format!("│ {quote}"), theme.muted, theme);
    }
    inline(text, theme.base, theme)
}

pub fn panel(theme: Theme, color: bool) -> Style {
    if color && theme.base.bg == Some(Color::Rgb(32, 31, 38)) {
        theme.muted.bg(Color::Rgb(45, 44, 54))
    } else {
        theme.muted
    }
}

pub fn editor(text: &str, cursor: usize, width: usize) -> (Vec<String>, usize, usize) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut column = 0;
    let mut cursor_position = (0, 0);
    for (offset, glyph) in text.grapheme_indices(true) {
        if offset == cursor {
            cursor_position = (rows.len() - 1, column);
        }
        if glyph == "\n" {
            rows.push(String::new());
            column = 0;
            continue;
        }
        if column + glyph.width() > width {
            rows.push(String::new());
            column = 0;
            if offset == cursor {
                cursor_position = (rows.len() - 1, 0);
            }
        }
        rows.last_mut().unwrap().push_str(glyph);
        column += glyph.width();
        if column >= width {
            rows.push(String::new());
            column = 0;
        }
    }
    if cursor == text.len() {
        cursor_position = (rows.len() - 1, column);
    }
    (rows, cursor_position.0, cursor_position.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluzo_core::settings::TuiSettings;

    #[test]
    fn model_meters_align_values_and_distinguish_usage_from_demo_cost() {
        for color in [false, true] {
            let theme = Theme::new(&TuiSettings::default(), color);
            for ascii in [false, true] {
                for width in [29, 54, 66, 114] {
                    let lines = demo_model_info(width, theme, ascii);
                    let usage = lines
                        .iter()
                        .position(|line| line.to_string().contains("75%"))
                        .unwrap();
                    let context = &lines[usage];
                    let cost = &lines[usage + 1];
                    assert!(context.to_string().ends_with(" 75% (303.8K)"));
                    assert!(cost.to_string().ends_with(" $50.00"));
                    for (line, filled) in [(context, 9), (cost, 6)] {
                        assert_eq!(
                            line.spans[1..=filled]
                                .iter()
                                .map(Span::width)
                                .sum::<usize>(),
                            filled
                        );
                        assert_eq!(line.spans[filled + 1].width(), 12 - filled);
                        if color {
                            assert_eq!(line.spans[1].style.fg, Some(Color::Rgb(48, 224, 224)));
                            assert_eq!(
                                line.spans[filled].style.fg,
                                Some(Color::Rgb(167, 113, 248))
                            );
                            for column in 0..filled {
                                assert_eq!(
                                    line.spans[column + 1].style.fg,
                                    Some(crate::identity::gradient(
                                        column as f64 / (filled - 1) as f64,
                                        0.0,
                                        1.0,
                                        true
                                    ))
                                );
                            }
                        } else {
                            assert!(line.spans.iter().all(|span| span.style.fg.is_none()));
                        }
                    }
                    assert!(
                        lines[usage + 2]
                            .to_string()
                            .contains("Cost scale: $100 (demo)")
                    );
                    assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
                    if ascii {
                        assert!(lines.iter().all(|line| line.to_string().is_ascii()));
                    }
                }
            }
        }
    }

    #[test]
    fn model_footer_is_one_muted_line_with_a_full_width_separator() {
        let footer = AssistantFooter {
            model: "GTP-6 Astra".into(),
            provider: "Github Copilot".into(),
            duration: "1m21s".into(),
        };
        for color in [false, true] {
            let theme = Theme::new(&TuiSettings::default(), color);
            for width in [58, 85, 125, 205] {
                let line = footer.render(width, theme, false);
                assert_eq!(line.width(), usize::from(width));
                let text: String = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
                assert!(text.starts_with("  ◇ GTP-6 Astra via Github Copilot in 1m21s "));
                assert!(text.ends_with('─'));
                assert!(!text.contains("tokens"));
                assert!(
                    line.spans
                        .iter()
                        .all(|span| !span.style.add_modifier.contains(Modifier::BOLD))
                );
            }
            let ascii = footer.render(85, theme, true);
            assert!(ascii.spans.iter().all(|span| span.content.is_ascii()));
        }
    }

    #[test]
    fn responsive_layout_has_no_window_maximum_and_reserves_editor() {
        for (width, height) in [
            (60, 16),
            (80, 24),
            (119, 40),
            (120, 29),
            (120, 40),
            (240, 80),
        ] {
            let area = Rect::new(0, 0, width, height);
            let regions = layout(area, 30);
            assert!(
                regions
                    .iter()
                    .all(|region| area.contains((region.x, region.y).into())
                        && region.right() <= area.right()
                        && region.bottom() <= area.bottom())
            );
            assert!(regions[1].height >= 2);
            assert!(regions[1].bottom() < regions[2].y);
            assert_eq!(body(area).width, width - if compact(area) { 2 } else { 34 });
        }
    }

    #[test]
    fn unicode_wrap_and_markdown_keep_styles_and_complete_content() {
        let theme = Theme::new(&TuiSettings::default(), true);
        let mut fenced = false;
        let spans = markdown("**Bold** `code` 世界 e\u{301}", &mut fenced, theme);
        let rows = wrap(spans, 9);
        assert!(rows.iter().all(|line| line.width() <= 9));
        assert!(
            rows.iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );
        let text: String = rows
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("世界"));
        assert!(text.contains("e\u{301}"));
        assert!(!text.contains('`'));
        let rows = wrap(inline("one longer word", theme.base, theme), 10);
        assert_eq!(
            rows[0]
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "one longer"
        );
        let rows = wrap(vec![Span::raw("  indented")], 20);
        assert_eq!(rows[0].width(), 10);
        markdown("```rust", &mut fenced, theme);
        assert!(fenced);
        assert_eq!(
            markdown("let value = 2;", &mut fenced, theme)[0].content,
            "  let value = 2;"
        );
        markdown("```", &mut fenced, theme);
        assert!(!fenced);
    }

    #[test]
    fn editor_wraps_preserving_graphemes_and_cursor() {
        let text = "abcd世界e\u{301}\nlast";
        let (rows, row, column) = editor(text, text.len(), 4);
        assert_eq!(rows, ["abcd", "世界", "e\u{301}", "last", ""]);
        assert_eq!((row, column), (4, 0));
        assert_eq!(editor(text, 4, 4).1, 1);
    }
}
