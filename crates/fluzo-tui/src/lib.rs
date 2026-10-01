pub mod configuration;
pub mod identity;
pub mod inspection;
mod notification;
pub mod notification_stack;
pub mod presentation;
pub mod setup;
pub mod shell;
pub mod terminal;
pub mod visual;
pub mod workspace;

use fluzo_core::RuntimeAvailability;

pub fn availability_text(availability: RuntimeAvailability) -> &'static str {
    match availability {
        RuntimeAvailability::NotImplemented => "The agent runtime is not implemented yet.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_controls_share_core_defaults_and_validation() {
        use fluzo_core::settings::{ApplicationRule, SettingValue, Settings, ValidationCode};

        let mut draft = Settings::default();
        let descriptors = draft.descriptors();
        let animation = descriptors
            .iter()
            .find(|entry| entry.key == "tui.animation_fps")
            .unwrap();
        assert_eq!(
            animation.default,
            SettingValue::Integer(u64::from(draft.tui.animation_fps))
        );
        assert_eq!(animation.application, ApplicationRule::Presentation);
        let budget = descriptors
            .iter()
            .find(|entry| entry.key == "harness.max_turns")
            .unwrap();
        assert_eq!(budget.application, ApplicationRule::ExplicitApply);
        draft.tui.animation_fps = 0;
        assert!(draft.validate().is_ok());
        draft.harness.max_turns = 0;
        assert!(
            draft
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.key == "harness.max_turns"
                    && error.code == ValidationCode::OutOfRange)
        );
    }

    #[test]
    fn visual_metadata_and_direct_validation_enforce_the_same_choices() {
        use fluzo_core::settings::{SettingKind, Settings};
        let mut settings = Settings::default();
        let descriptors = settings.descriptors();
        let fps = descriptors
            .iter()
            .find(|entry| entry.key == "tui.animation_fps")
            .unwrap();
        assert_eq!(fps.integer_maximum, Some(60));
        settings.tui.animation_fps = 61;
        assert!(
            settings
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.key == fps.key)
        );
        settings.tui.animation_fps = 60;
        let theme = descriptors
            .iter()
            .find(|entry| entry.key == "tui.theme")
            .unwrap();
        let SettingKind::Choice(choices) = &theme.kind else {
            panic!("typed theme choices required");
        };
        for choice in choices {
            settings.tui.theme = choice.clone();
            assert!(settings.validate().is_ok());
        }
        settings.tui.theme = "unimplemented".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn bootstrap_status_is_explicit() {
        assert_eq!(
            availability_text(RuntimeAvailability::NotImplemented),
            "The agent runtime is not implemented yet."
        );
    }
}
