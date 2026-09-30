use std::time::Duration;

use fluzo_core::application::Cursor;
use fluzo_core::settings::{NotificationSettings, Settings, ValidationError};
use unicode_segmentation::UnicodeSegmentation;

pub const MAX_RETAINED: usize = 32;
pub const MAX_DISPLAY_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    Information,
    Success,
    Warning,
    Error,
    Approval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifetime {
    Transient,
    UntilDismissed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notice {
    cursor: Cursor,
    severity: Severity,
    text: Box<str>,
    received_at: Duration,
    expires_at: Option<Duration>,
}

impl Notice {
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn received_at(&self) -> Duration {
        self.received_at
    }

    pub fn expires_at(&self) -> Option<Duration> {
        self.expires_at
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertOutcome {
    Retained { evicted: Option<Cursor> },
    DuplicateOrStale,
    Dropped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StackError {
    InvalidSettings(Vec<ValidationError>),
    UnexpectedEpoch,
    TimeReversed,
    DeadlineOverflow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NotificationStack {
    settings: NotificationSettings,
    epoch: u64,
    last_sequence: Option<u64>,
    now: Duration,
    notices: Vec<Notice>,
    overflow_count: u64,
}

impl NotificationStack {
    pub fn new(settings: NotificationSettings, epoch: u64) -> Result<Self, StackError> {
        validate(&settings)?;
        Ok(Self {
            settings,
            epoch,
            last_sequence: None,
            now: Duration::ZERO,
            notices: Vec::with_capacity(MAX_RETAINED),
            overflow_count: 0,
        })
    }

    pub fn settings(&self) -> &NotificationSettings {
        &self.settings
    }

    pub fn configure(&mut self, settings: NotificationSettings) -> Result<(), StackError> {
        validate(&settings)?;
        self.settings = settings;
        Ok(())
    }

    pub fn retained(&self) -> &[Notice] {
        &self.notices
    }

    pub fn visible(&self, available_slots: usize) -> impl Iterator<Item = &Notice> {
        self.notices
            .iter()
            .take(available_slots.min(self.settings.max_visible as usize))
    }

    pub fn overflow_count(&self) -> u64 {
        self.overflow_count
    }

    pub fn advance(&mut self, now: Duration) -> Result<usize, StackError> {
        if now < self.now {
            return Err(StackError::TimeReversed);
        }
        self.now = now;
        let before = self.notices.len();
        self.notices
            .retain(|notice| notice.expires_at.is_none_or(|deadline| now < deadline));
        Ok(before - self.notices.len())
    }

    pub fn dismiss(&mut self, cursor: Cursor) -> bool {
        let before = self.notices.len();
        self.notices.retain(|notice| notice.cursor != cursor);
        self.notices.len() != before
    }

    pub fn receive(
        &mut self,
        cursor: Cursor,
        severity: Severity,
        lifetime: Lifetime,
        text: &str,
        now: Duration,
    ) -> Result<InsertOutcome, StackError> {
        if cursor.epoch != self.epoch {
            return Err(StackError::UnexpectedEpoch);
        }
        if now < self.now {
            return Err(StackError::TimeReversed);
        }
        if self
            .last_sequence
            .is_some_and(|sequence| cursor.sequence <= sequence)
        {
            self.advance(now)?;
            return Ok(InsertOutcome::DuplicateOrStale);
        }
        let expires_at = match lifetime {
            Lifetime::Transient => Some(
                now.checked_add(Duration::from_secs(self.settings.duration_seconds))
                    .ok_or(StackError::DeadlineOverflow)?,
            ),
            Lifetime::UntilDismissed => None,
        };
        self.advance(now)?;
        self.last_sequence = Some(cursor.sequence);
        let mut evicted = None;
        if self.notices.len() == MAX_RETAINED {
            let victim = self
                .notices
                .iter()
                .enumerate()
                .min_by_key(|(index, notice)| (notice.severity, *index))
                .map(|(index, notice)| (index, notice.severity));
            self.overflow_count = self.overflow_count.saturating_add(1);
            if let Some((index, priority)) = victim {
                if severity < priority {
                    return Ok(InsertOutcome::Dropped);
                }
                evicted = Some(self.notices.remove(index).cursor);
            }
        }
        self.notices.push(Notice {
            cursor,
            severity,
            text: display_text(text),
            received_at: now,
            expires_at,
        });
        Ok(InsertOutcome::Retained { evicted })
    }
}

fn validate(notifications: &NotificationSettings) -> Result<(), StackError> {
    let mut settings = Settings::default();
    settings.tui.notifications = notifications.clone();
    settings.validate().map_err(StackError::InvalidSettings)
}

fn display_text(text: &str) -> Box<str> {
    let mut sanitizer = crate::shell::Sanitizer::default();
    let mut safe = String::with_capacity(MAX_DISPLAY_BYTES);
    for character in text.chars() {
        let mut encoded = [0; 4];
        safe.push_str(
            &sanitizer
                .push(character.encode_utf8(&mut encoded))
                .replace('\n', " "),
        );
        if safe.len() > MAX_DISPLAY_BYTES {
            let mut end = 0;
            for (offset, glyph) in safe.grapheme_indices(true) {
                if offset + glyph.len() > MAX_DISPLAY_BYTES - 3 {
                    break;
                }
                end = offset + glyph.len();
            }
            safe.truncate(end);
            safe.push_str("...");
            break;
        }
    }
    safe.into_boxed_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(sequence: u64) -> Cursor {
        Cursor { epoch: 7, sequence }
    }

    fn stack() -> NotificationStack {
        NotificationStack::new(NotificationSettings::default(), 7).unwrap()
    }

    fn receive(stack: &mut NotificationStack, sequence: u64, severity: Severity) -> InsertOutcome {
        stack
            .receive(
                cursor(sequence),
                severity,
                Lifetime::Transient,
                "same text",
                Duration::ZERO,
            )
            .unwrap()
    }

    #[test]
    fn duplicates_never_extend_expiry_or_resurrect_removed_notices() {
        let mut stack = stack();
        receive(&mut stack, 1, Severity::Information);
        let notice = stack.retained()[0].clone();
        assert_eq!(
            stack.receive(
                cursor(1),
                Severity::Error,
                Lifetime::UntilDismissed,
                "changed",
                Duration::from_secs(4)
            ),
            Ok(InsertOutcome::DuplicateOrStale)
        );
        assert_eq!(stack.retained(), &[notice]);
        assert_eq!(stack.advance(Duration::from_secs(5)), Ok(1));
        assert_eq!(
            stack.receive(
                cursor(1),
                Severity::Information,
                Lifetime::Transient,
                "same text",
                Duration::from_secs(6)
            ),
            Ok(InsertOutcome::DuplicateOrStale)
        );
        assert!(stack.retained().is_empty());
        stack
            .receive(
                cursor(2),
                Severity::Approval,
                Lifetime::UntilDismissed,
                "same text",
                Duration::from_secs(6),
            )
            .unwrap();
        assert!(stack.dismiss(cursor(2)));
        assert!(!stack.dismiss(cursor(2)));
        assert_eq!(
            stack.receive(
                cursor(2),
                Severity::Approval,
                Lifetime::UntilDismissed,
                "same text",
                Duration::from_secs(7)
            ),
            Ok(InsertOutcome::DuplicateOrStale)
        );
        assert!(stack.retained().is_empty());
    }

    #[test]
    fn distinct_events_with_identical_text_are_retained() {
        let mut stack = stack();
        receive(&mut stack, 1, Severity::Information);
        receive(&mut stack, 2, Severity::Error);
        assert_eq!(stack.retained().len(), 2);
        assert_ne!(stack.retained()[0].cursor(), stack.retained()[1].cursor());
    }

    #[test]
    fn overflow_keeps_priority_then_age_without_unbounded_dedup_storage() {
        let mut stack = stack();
        for sequence in 1..=32 {
            receive(&mut stack, sequence, Severity::Error);
        }
        assert_eq!(
            receive(&mut stack, 33, Severity::Information),
            InsertOutcome::Dropped
        );
        assert_eq!(
            receive(&mut stack, 34, Severity::Warning),
            InsertOutcome::Dropped
        );
        assert_eq!(
            receive(&mut stack, 35, Severity::Approval),
            InsertOutcome::Retained {
                evicted: Some(cursor(1))
            }
        );
        assert_eq!(
            receive(&mut stack, 36, Severity::Error),
            InsertOutcome::Retained {
                evicted: Some(cursor(2))
            }
        );
        assert_eq!(stack.overflow_count(), 4);
        assert_eq!(
            receive(&mut stack, 33, Severity::Information),
            InsertOutcome::DuplicateOrStale
        );
        stack.overflow_count = u64::MAX;
        for sequence in 37..=1000 {
            receive(&mut stack, sequence, Severity::Error);
            assert_eq!(stack.retained().len(), MAX_RETAINED);
            assert_eq!(stack.notices.capacity(), MAX_RETAINED);
        }
        assert_eq!(stack.overflow_count(), u64::MAX);
        assert!(
            stack
                .retained()
                .iter()
                .any(|notice| notice.cursor() == cursor(35))
        );
        assert_eq!(
            receive(&mut stack, 1, Severity::Error),
            InsertOutcome::DuplicateOrStale
        );
    }

    #[test]
    fn oldest_information_is_discarded_before_older_errors() {
        let mut stack = stack();
        receive(&mut stack, 1, Severity::Error);
        for sequence in 2..=32 {
            receive(&mut stack, sequence, Severity::Information);
        }
        assert_eq!(
            receive(&mut stack, 33, Severity::Information),
            InsertOutcome::Retained {
                evicted: Some(cursor(2))
            }
        );
        assert_eq!(stack.retained()[0].severity(), Severity::Error);
        assert_eq!(stack.overflow_count(), 1);
    }

    #[test]
    fn pending_notices_expire_from_receipt_and_capacity_never_changes_settings() {
        let mut stack = stack();
        for sequence in 1..=8 {
            receive(&mut stack, sequence, Severity::Information);
        }
        for slots in [0, 1, 2, 3, 5, usize::MAX, 0] {
            assert_eq!(stack.visible(slots).count(), slots.min(3));
            assert_eq!(stack.settings().max_visible, 3);
            assert_eq!(stack.retained().len(), 8);
        }
        assert_eq!(stack.advance(Duration::from_secs(4)), Ok(0));
        assert_eq!(stack.advance(Duration::from_secs(5)), Ok(8));
        assert_eq!(stack.visible(5).count(), 0);
    }

    #[test]
    fn configuration_changes_are_atomic_and_only_affect_future_deadlines() {
        let mut stack = stack();
        receive(&mut stack, 1, Severity::Information);
        for settings in [
            NotificationSettings {
                duration_seconds: 0,
                ..NotificationSettings::default()
            },
            NotificationSettings {
                duration_seconds: 31,
                ..NotificationSettings::default()
            },
            NotificationSettings {
                max_visible: 0,
                ..NotificationSettings::default()
            },
            NotificationSettings {
                max_visible: 6,
                ..NotificationSettings::default()
            },
        ] {
            assert!(NotificationStack::new(settings.clone(), 7).is_err());
            let before = stack.clone();
            assert!(matches!(
                stack.configure(settings),
                Err(StackError::InvalidSettings(_))
            ));
            assert_eq!(stack, before);
        }
        stack
            .configure(NotificationSettings {
                duration_seconds: 30,
                max_visible: 5,
                desktop_enabled: true,
            })
            .unwrap();
        stack
            .receive(
                cursor(2),
                Severity::Error,
                Lifetime::Transient,
                "second",
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(
            stack.retained()[0].expires_at(),
            Some(Duration::from_secs(5))
        );
        assert_eq!(stack.retained()[1].received_at(), Duration::from_secs(1));
        assert_eq!(
            stack.retained()[1].expires_at(),
            Some(Duration::from_secs(31))
        );
        assert_eq!(stack.advance(Duration::from_secs(5)), Ok(1));
        stack.configure(NotificationSettings::default()).unwrap();
        assert_eq!(
            stack.retained()[0].expires_at(),
            Some(Duration::from_secs(31))
        );
    }

    #[test]
    fn dismissal_and_expiry_only_remove_presentation_records() {
        let mut stack = stack();
        let authoritative = [
            (cursor(1), fluzo_core::application::TaskState::Failed),
            (cursor(2), fluzo_core::application::TaskState::Waiting),
        ];
        let before = authoritative;
        stack
            .receive(
                cursor(1),
                Severity::Error,
                Lifetime::Transient,
                "failure",
                Duration::ZERO,
            )
            .unwrap();
        stack
            .receive(
                cursor(2),
                Severity::Approval,
                Lifetime::UntilDismissed,
                "approval",
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(stack.advance(Duration::from_secs(5)), Ok(1));
        assert_eq!(stack.retained()[0].cursor(), cursor(2));
        assert!(stack.dismiss(cursor(2)));
        assert_eq!(authoritative, before);
        assert!(stack.retained().is_empty());
    }

    #[test]
    fn duration_and_visibility_boundaries_apply_without_rewriting_configuration() {
        for duration_seconds in [1, 30] {
            for max_visible in [1, 5] {
                let settings = NotificationSettings {
                    duration_seconds,
                    max_visible,
                    desktop_enabled: false,
                };
                let mut stack = NotificationStack::new(settings.clone(), 7).unwrap();
                for sequence in 1..=32 {
                    receive(&mut stack, sequence, Severity::Success);
                }
                for slots in [0, 1, 2, 5, usize::MAX] {
                    assert_eq!(
                        stack.visible(slots).count(),
                        slots.min(max_visible as usize)
                    );
                    assert_eq!(stack.settings(), &settings);
                }
                assert_eq!(
                    stack.advance(Duration::from_secs(duration_seconds) - Duration::from_nanos(1)),
                    Ok(0)
                );
                assert_eq!(stack.advance(Duration::from_secs(duration_seconds)), Ok(32));
            }
        }
    }

    #[test]
    fn invalid_time_or_epoch_leaves_all_state_unchanged() {
        let mut stack = stack();
        stack.advance(Duration::from_secs(10)).unwrap();
        let before = stack.clone();
        for (id, now, expected) in [
            (
                Cursor {
                    epoch: 8,
                    sequence: 1,
                },
                Duration::from_secs(10),
                StackError::UnexpectedEpoch,
            ),
            (cursor(1), Duration::from_secs(9), StackError::TimeReversed),
            (cursor(1), Duration::MAX, StackError::DeadlineOverflow),
        ] {
            assert_eq!(
                stack.receive(id, Severity::Error, Lifetime::Transient, "test", now),
                Err(expected)
            );
            assert_eq!(stack, before);
        }
        assert_eq!(stack.advance(Duration::ZERO), Err(StackError::TimeReversed));
        assert_eq!(stack, before);
        stack
            .receive(
                cursor(u64::MAX),
                Severity::Approval,
                Lifetime::UntilDismissed,
                "approval",
                Duration::MAX,
            )
            .unwrap();
        assert_eq!(stack.advance(Duration::MAX), Ok(0));
        assert_eq!(stack.retained().len(), 1);
    }

    #[test]
    fn text_is_sanitized_bounded_and_truncated_at_grapheme_boundaries() {
        assert_eq!(
            &*display_text("safe\u{1b}]52;c;secret\u{7}\u{1b}[2J\n\t\u{202e}"),
            "safe     [U+202E]"
        );
        assert_eq!(&*display_text(&"x".repeat(1024)), "x".repeat(1024));
        assert_eq!(
            &*display_text(&"x".repeat(1025)),
            format!("{}...", "x".repeat(1021))
        );
        for glyph in ["e\u{301}", "世界", "👩‍💻", "🇪🇸"] {
            let text = glyph.repeat(1500);
            let safe = display_text(&text);
            assert!(safe.len() <= MAX_DISPLAY_BYTES);
            let prefix = safe.strip_suffix("...").unwrap();
            assert!(
                text.grapheme_indices(true)
                    .any(|(offset, _)| offset == prefix.len())
            );
            assert!(text.starts_with(prefix));
        }
        assert_eq!(
            &*display_text(&format!("e{}", "\u{301}".repeat(2000))),
            "..."
        );
        let mut stack = stack();
        for sequence in 1..=100 {
            stack
                .receive(
                    cursor(sequence),
                    Severity::Information,
                    Lifetime::Transient,
                    &"x".repeat(4096),
                    Duration::ZERO,
                )
                .unwrap();
        }
        assert_eq!(
            stack
                .retained()
                .iter()
                .map(|notice| notice.text().len())
                .sum::<usize>(),
            MAX_RETAINED * MAX_DISPLAY_BYTES
        );
    }
}
