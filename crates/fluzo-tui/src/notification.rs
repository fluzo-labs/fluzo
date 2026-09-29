use std::io::{self, Write};
use std::time::Duration;

const COMPLETED: &[u8] = b"\x1b]777;notify;Fluzo is waiting...;Synthetic playback completed. No agent work was executed.\x1b\\";
#[cfg(test)]
const TEST_COMPLETED: &[u8] = b"\x1b]777;notify;Fluzo notification test;Synthetic completion test. No agent work was executed.\x1b\\";

pub(crate) fn safe_test_text(text: &str) -> String {
    let bounded: String = text.chars().take(1024).collect();
    let safe = crate::shell::Sanitizer::default()
        .push(&bounded)
        .replace(['\n', ';'], " ");
    let mut output = String::new();
    for glyph in unicode_segmentation::UnicodeSegmentation::graphemes(safe.as_str(), true) {
        if output.len() + glyph.len() > 256 {
            break;
        }
        output.push_str(glyph);
    }
    output
}

pub(crate) struct Notifications {
    enabled: bool,
    supported: bool,
    focused: Option<bool>,
    test_deadline: Option<Duration>,
    test_message: String,
}

impl Notifications {
    pub(crate) fn new(enabled: bool, term: &str) -> Self {
        Self {
            enabled,
            supported: term == "xterm-ghostty",
            focused: None,
            test_deadline: None,
            test_message: "Synthetic completion test. No agent work was executed.".into(),
        }
    }

    pub(crate) fn focus(&mut self, focused: bool) {
        self.focused = Some(focused);
    }

    #[cfg(test)]
    pub(crate) fn schedule_test(&mut self, now: Duration) -> &'static str {
        self.schedule_message(
            now,
            "Synthetic completion test. No agent work was executed.",
        )
    }

    pub(crate) fn schedule_message(&mut self, now: Duration, text: &str) -> &'static str {
        if !self.enabled {
            return "Notifications disabled; start with --desktop-notifications.";
        }
        if !self.supported {
            return "Notification transport unavailable; this preview supports Ghostty OSC 777.";
        }
        let message = safe_test_text(text);
        if message.trim().is_empty() {
            return "Notification text is empty; pending test unchanged.";
        }
        self.test_message = message;
        self.test_deadline = Some(now.saturating_add(Duration::from_secs(3)));
        "Notification test in 3 seconds; focus another window to receive it."
    }

    pub(crate) fn suppression_reason(&self) -> Option<&'static str> {
        if !self.enabled {
            Some("Notifications disabled; use --desktop-notifications.")
        } else if !self.supported {
            Some("Notifications unavailable: terminal is not identified as Ghostty.")
        } else {
            match self.focused {
                None => Some("Notification suppressed: no window focus event received."),
                Some(true) => Some("Notification suppressed: terminal reports window focused."),
                Some(false) => None,
            }
        }
    }

    fn send(&self, writer: &mut impl Write, test: bool) -> io::Result<bool> {
        if self.suppression_reason().is_some() {
            return Ok(false);
        }
        if test {
            let message = format!(
                "\u{1b}]777;notify;Fluzo notification test;{}\u{1b}\\",
                self.test_message
            );
            writer.write_all(message.as_bytes())?;
        } else {
            writer.write_all(COMPLETED)?;
        }
        writer.flush()?;
        Ok(true)
    }

    pub(crate) fn completed(&self, writer: &mut impl Write) -> io::Result<bool> {
        self.send(writer, false)
    }

    pub(crate) fn tick(
        &mut self,
        now: Duration,
        writer: &mut impl Write,
    ) -> Option<io::Result<bool>> {
        if self.test_deadline.is_some_and(|deadline| now >= deadline) {
            self.test_deadline = None;
            Some(self.send(writer, true))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_test_message_is_bounded_sanitized_and_snapshotted() {
        let mut notifications = Notifications::new(true, "xterm-ghostty");
        notifications.focus(false);
        let text = "Hello;world\n\u{1b}]52;c;hidden\u{7}\u{1b}[2J\u{202e}";
        notifications.schedule_message(Duration::ZERO, text);
        assert!(
            notifications
                .schedule_message(Duration::from_secs(1), "\u{1b}[2J")
                .contains("empty")
        );
        let mut output = Vec::new();
        assert!(
            notifications
                .tick(Duration::from_secs(2), &mut output)
                .is_none()
        );
        assert!(
            notifications
                .tick(Duration::from_secs(3), &mut output)
                .unwrap()
                .unwrap()
        );
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\u{1b}]777;notify;Fluzo notification test;Hello world [U+202E]\u{1b}\\"
        );
        let safe = safe_test_text(&"世界".repeat(1000));
        assert!(safe.len() <= 256);
        assert_eq!(safe.chars().count(), 85);
        assert!(safe.ends_with('世'));
        assert!(!safe.contains('\u{1b}'));
        assert!(
            notifications
                .tick(Duration::from_secs(4), &mut Vec::new())
                .is_none()
        );
    }

    #[test]
    fn completion_requires_opt_in_supported_transport_and_observed_blur() {
        for enabled in [false, true] {
            for term in ["xterm-ghostty", "xterm-256color", "dumb", ""] {
                for focus in [None, Some(true), Some(false)] {
                    let mut notifications = Notifications::new(enabled, term);
                    if let Some(focused) = focus {
                        notifications.focus(focused);
                    }
                    let mut output = Vec::new();
                    let sent = notifications.completed(&mut output).unwrap();
                    assert_eq!(
                        sent,
                        enabled && term == "xterm-ghostty" && focus == Some(false)
                    );
                    assert_eq!(output.as_slice(), if sent { COMPLETED } else { b"" });
                }
            }
        }
    }

    #[test]
    fn test_deadline_is_bounded_replaced_and_consumed_once_even_when_suppressed() {
        let mut notifications = Notifications::new(true, "xterm-ghostty");
        let mut output = Vec::new();
        notifications.schedule_test(Duration::ZERO);
        notifications.schedule_test(Duration::from_secs(1));
        notifications.focus(false);
        assert!(
            notifications
                .tick(Duration::from_secs(3), &mut output)
                .is_none()
        );
        assert!(
            notifications
                .tick(Duration::from_secs(4), &mut output)
                .unwrap()
                .unwrap()
        );
        assert_eq!(output, TEST_COMPLETED);
        assert!(
            notifications
                .tick(Duration::from_secs(5), &mut output)
                .is_none()
        );
        notifications.schedule_test(Duration::from_secs(5));
        notifications.focus(true);
        assert!(
            !notifications
                .tick(Duration::from_secs(8), &mut output)
                .unwrap()
                .unwrap()
        );
        notifications.focus(false);
        assert!(
            notifications
                .tick(Duration::from_secs(9), &mut output)
                .is_none()
        );
        assert_eq!(output, TEST_COMPLETED);
    }

    #[test]
    fn suppression_diagnostics_distinguish_disabled_unsupported_and_focus() {
        assert!(
            Notifications::new(false, "xterm-ghostty")
                .suppression_reason()
                .unwrap()
                .contains("disabled")
        );
        assert!(
            Notifications::new(true, "dumb")
                .suppression_reason()
                .unwrap()
                .contains("unavailable")
        );
        let mut notifications = Notifications::new(true, "xterm-ghostty");
        assert!(
            notifications
                .suppression_reason()
                .unwrap()
                .contains("no window focus event")
        );
        notifications.focus(true);
        assert!(
            notifications
                .suppression_reason()
                .unwrap()
                .contains("window focused")
        );
        notifications.focus(false);
        assert_eq!(notifications.suppression_reason(), None);
    }

    #[test]
    fn failed_notification_is_not_retried() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("fixture"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut notifications = Notifications::new(true, "xterm-ghostty");
        notifications.focus(false);
        notifications.schedule_test(Duration::ZERO);
        assert!(
            notifications
                .tick(Duration::from_secs(3), &mut FailingWriter)
                .unwrap()
                .is_err()
        );
        assert!(
            notifications
                .tick(Duration::from_secs(4), &mut FailingWriter)
                .is_none()
        );
    }
}
