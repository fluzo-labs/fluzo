use fluzo_core::settings::{
    Constraint, Privacy, SettingDescriptor, SettingKind, SettingValue, Settings, ValidationError,
    valid_identifier,
};
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use toml_edit::{DocumentMut, Item, TableLike, Value};

pub const MAX_CONFIG_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigErrorCode {
    TooLarge,
    Syntax,
    UnknownField,
    InvalidType,
    InvalidValue,
    MissingVersion,
    Validation,
    Serialization,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigError {
    pub code: ConfigErrorCode,
    pub key: String,
    pub span: Option<Range<usize>>,
    pub validation: Vec<ValidationError>,
}

impl ConfigError {
    fn new(code: ConfigErrorCode, key: impl Into<String>, span: Option<Range<usize>>) -> Self {
        Self {
            code,
            key: key.into(),
            span,
            validation: Vec::new(),
        }
    }

    fn validation(errors: Vec<ValidationError>) -> Self {
        Self {
            code: ConfigErrorCode::Validation,
            key: String::new(),
            span: None,
            validation: errors,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "configuration {:?} at {}",
            self.code,
            if self.key.is_empty() {
                "<document>"
            } else {
                &self.key
            }
        )?;
        for error in &self.validation {
            write!(formatter, "; {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

pub fn parse_settings(source: &str) -> Result<Settings, ConfigError> {
    if source.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::new(ConfigErrorCode::TooLarge, "", None));
    }
    let document = toml_edit::Document::parse(source.to_owned())
        .map_err(|error| ConfigError::new(ConfigErrorCode::Syntax, "", error.span()))?;
    let descriptors: BTreeMap<_, _> = Settings::default()
        .descriptors()
        .into_iter()
        .chain(Settings::collection_descriptors())
        .map(|descriptor| (descriptor.key.clone(), descriptor))
        .collect();
    check_table(document.as_table(), "", &descriptors)?;
    if document.get("schema_version").is_none() {
        return Err(ConfigError::new(
            ConfigErrorCode::MissingVersion,
            "schema_version",
            None,
        ));
    }
    let settings: Settings = toml_edit::de::from_document(document)
        .map_err(|error| ConfigError::new(ConfigErrorCode::InvalidType, "", error.span()))?;
    settings.validate().map_err(ConfigError::validation)?;
    Ok(settings)
}

pub fn encode_settings(settings: &Settings) -> Result<String, ConfigError> {
    settings.validate().map_err(ConfigError::validation)?;
    let output = toml_edit::ser::to_string_pretty(settings)
        .map_err(|_| ConfigError::new(ConfigErrorCode::Serialization, "", None))?;
    if output.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::new(ConfigErrorCode::TooLarge, "", None));
    }
    Ok(output)
}

pub fn redacted_values(settings: &Settings) -> BTreeMap<String, SettingValue> {
    settings
        .entries()
        .into_iter()
        .map(|(descriptor, value)| {
            let value = if descriptor.privacy == Privacy::Public || value == SettingValue::Unset {
                value
            } else {
                SettingValue::Text("[redacted]".to_owned())
            };
            (descriptor.key, value)
        })
        .collect()
}

fn check_table(
    table: &dyn TableLike,
    prefix: &str,
    descriptors: &BTreeMap<String, SettingDescriptor>,
) -> Result<(), ConfigError> {
    for (name, item) in table.iter() {
        let key = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}.{name}")
        };
        if !valid_identifier(name) {
            return Err(ConfigError::new(
                ConfigErrorCode::UnknownField,
                format!("{prefix}.<invalid-key>"),
                item.span(),
            ));
        }
        let schema_key = template_key(&key);
        if let Some(descriptor) = descriptors.get(&schema_key) {
            check_value(item, &key, descriptor)?;
        } else if matches!(key.as_str(), "models" | "capacity_pools")
            || descriptors
                .keys()
                .any(|candidate| candidate.starts_with(&format!("{schema_key}.")))
        {
            let child = item
                .as_table_like()
                .ok_or_else(|| ConfigError::new(ConfigErrorCode::InvalidType, &key, item.span()))?;
            check_table(child, &key, descriptors)?;
        } else {
            return Err(ConfigError::new(
                ConfigErrorCode::UnknownField,
                key,
                item.span(),
            ));
        }
    }
    Ok(())
}

fn template_key(key: &str) -> String {
    let mut parts: Vec<_> = key.split('.').collect();
    if matches!(parts.first(), Some(&"models" | &"capacity_pools")) && parts.len() > 1 {
        parts[1] = "<id>";
    }
    parts.join(".")
}

fn check_value(item: &Item, key: &str, descriptor: &SettingDescriptor) -> Result<(), ConfigError> {
    let value = item.as_value();
    let valid = match &descriptor.kind {
        SettingKind::Boolean => value.is_some_and(Value::is_bool),
        SettingKind::Integer => value.and_then(Value::as_integer).is_some_and(|value| {
            value >= 0
                && descriptor
                    .integer_maximum
                    .is_none_or(|maximum| value as u64 <= maximum)
        }),
        SettingKind::Fraction => value.is_some_and(|value| value.is_float() || value.is_integer()),
        SettingKind::Text | SettingKind::CredentialReference => value.is_some_and(Value::is_str),
        SettingKind::Choice(options) => value
            .and_then(Value::as_str)
            .is_some_and(|value| options.iter().any(|option| option == value)),
        SettingKind::TextList => value
            .and_then(Value::as_array)
            .is_some_and(|array| array.iter().all(Value::is_str)),
        SettingKind::TextMap => item
            .as_table_like()
            .is_some_and(|table| table.iter().all(|(_, value)| value.as_str().is_some())),
    };
    if !valid {
        let code = if matches!(descriptor.kind, SettingKind::Choice(_)) {
            ConfigErrorCode::InvalidValue
        } else {
            ConfigErrorCode::InvalidType
        };
        return Err(ConfigError::new(code, key, item.span()));
    }
    if descriptor.constraint == Constraint::SchemaVersion
        && value.and_then(Value::as_integer)
            != Some(i64::from(fluzo_core::settings::SCHEMA_VERSION))
    {
        return Err(ConfigError::validation(vec![ValidationError::new(
            key,
            fluzo_core::settings::ValidationCode::UnsupportedVersion,
        )]));
    }
    Ok(())
}

pub fn update_draft(source: &str, key: &str, value: SettingValue) -> Result<String, ConfigError> {
    let settings = parse_settings(source)?;
    let descriptor = settings
        .descriptors()
        .into_iter()
        .find(|descriptor| descriptor.key == key)
        .ok_or_else(|| ConfigError::new(ConfigErrorCode::UnknownField, "<unknown-key>", None))?;
    let mut document = source
        .parse::<DocumentMut>()
        .map_err(|error| ConfigError::new(ConfigErrorCode::Syntax, "", error.span()))?;
    let item = match value {
        SettingValue::Boolean(value) => toml_edit::value(value),
        SettingValue::Integer(value) => toml_edit::value(
            i64::try_from(value)
                .map_err(|_| ConfigError::new(ConfigErrorCode::InvalidValue, key, None))?,
        ),
        SettingValue::Fraction(value) => toml_edit::value(value),
        SettingValue::Text(value) => toml_edit::value(value),
        SettingValue::TextList(values) => {
            toml_edit::value(values.into_iter().collect::<toml_edit::Array>())
        }
        SettingValue::TextMap(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values {
                table.insert(&key, Value::from(value));
            }
            Item::Value(Value::InlineTable(table))
        }
        SettingValue::Unset if descriptor.optional => Item::None,
        SettingValue::Unset => {
            return Err(ConfigError::new(ConfigErrorCode::InvalidValue, key, None));
        }
    };
    if !item.is_none() {
        check_value(&item, key, &descriptor)?;
    }
    let mut parts = key.split('.').peekable();
    let mut table: &mut dyn TableLike = document.as_table_mut();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            if item.is_none() {
                table.remove(part);
            } else {
                table.insert(part, item);
            }
            break;
        }
        if !table.contains_key(part) {
            table.insert(part, Item::Table(toml_edit::Table::new()));
        }
        table = table
            .get_mut(part)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| ConfigError::new(ConfigErrorCode::InvalidType, key, None))?;
    }
    let output = document.to_string();
    parse_settings(&output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluzo_core::settings::*;

    const MODEL: &str = "schema_version = 1\n[agent]\nmodel = 'coder'\n[capacity_pools.local]\n[models.coder]\nbase_url = 'http://127.0.0.1:8080/v1'\nmodel = 'fixture-model'\ncapacity_id = 'fixture'\npool = 'local'\n";

    #[test]
    fn full_defaults_round_trip_and_never_select_a_service() {
        let defaults = Settings::default();
        let text = encode_settings(&defaults).unwrap();
        assert_eq!(parse_settings(&text).unwrap(), defaults);
        assert!(!text.contains("http://"));
        assert!(!text.contains("https://"));
        assert_eq!(parse_settings("schema_version = 1").unwrap(), defaults);
        assert_eq!(
            parse_settings("schema_version = 1\n[tui]\nreduced_motion = true")
                .unwrap()
                .tui
                .animation_fps,
            60
        );
    }

    #[test]
    fn all_serialized_leaves_match_descriptors_and_defaults() {
        let settings = parse_settings(MODEL).unwrap();
        let text = encode_settings(&settings).unwrap();
        let document: DocumentMut = text.parse().unwrap();
        for (descriptor, value) in settings.entries() {
            let mut item = document.as_item();
            for part in descriptor.key.split('.') {
                if let Some(child) = item.get(part) {
                    item = child;
                } else {
                    assert_eq!(value, SettingValue::Unset, "{}", descriptor.key);
                    break;
                }
            }
            if value != SettingValue::Unset {
                check_value(item, &descriptor.key, &descriptor).unwrap();
            }
        }
        assert_eq!(parse_settings(&text).unwrap(), settings);
        assert_eq!(
            settings
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.key == "models.coder.max_in_flight")
                .unwrap()
                .default,
            SettingValue::Integer(1)
        );
    }

    #[test]
    fn unknown_fields_and_unsupported_modes_fail_at_every_level() {
        for (text, key, code) in [
            (
                "schema_version = 1\nyolo = true",
                "yolo",
                ConfigErrorCode::UnknownField,
            ),
            (
                "schema_version = 1\n[tui]\nunknown = 1",
                "tui.unknown",
                ConfigErrorCode::UnknownField,
            ),
            (
                "schema_version = 1\n[tui.notifications]\nunknown = 1",
                "tui.notifications.unknown",
                ConfigErrorCode::UnknownField,
            ),
            (
                "schema_version = 1\n[models.coder]\napi_key = 'synthetic-secret'",
                "models.coder.api_key",
                ConfigErrorCode::UnknownField,
            ),
            (
                "schema_version = 1\n[capacity_pools.local]\nunknown = 1",
                "capacity_pools.local.unknown",
                ConfigErrorCode::UnknownField,
            ),
            (
                "schema_version = 1\n[decision]\nmode = 'active'",
                "decision.mode",
                ConfigErrorCode::InvalidValue,
            ),
            (
                "schema_version = 1\n[tools.shell]\nstdin = 'inherit'",
                "tools.shell.stdin",
                ConfigErrorCode::InvalidValue,
            ),
            (
                "schema_version = 1\n[tui]\nanimation_fps = 'synthetic-secret'",
                "tui.animation_fps",
                ConfigErrorCode::InvalidType,
            ),
        ] {
            let error = parse_settings(text).unwrap_err();
            assert_eq!(error.code, code, "{key}");
            assert_eq!(error.key, key);
            assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
        }
    }

    #[test]
    fn every_registered_constraint_is_enforced_through_draft_edits() {
        let settings = parse_settings(MODEL).unwrap();
        let source = encode_settings(&settings).unwrap();
        for descriptor in settings.descriptors() {
            let invalid = match descriptor.constraint {
                Constraint::Positive => Some(SettingValue::Integer(0)),
                Constraint::Fraction => Some(SettingValue::Fraction(f64::NAN)),
                Constraint::FixedFalse => Some(SettingValue::Boolean(true)),
                Constraint::SchemaVersion => Some(SettingValue::Integer(2)),
                Constraint::Any => None,
            };
            if let Some(invalid) = invalid {
                assert!(
                    update_draft(&source, &descriptor.key, invalid).is_err(),
                    "{}",
                    descriptor.key
                );
            }
            if let Some(maximum) = descriptor.integer_maximum {
                assert!(
                    update_draft(&source, &descriptor.key, SettingValue::Integer(maximum + 1))
                        .is_err(),
                    "{}",
                    descriptor.key
                );
            }
        }
    }

    #[test]
    fn malformed_collections_and_numeric_boundaries_are_rejected() {
        for source in [
            "schema_version = 1\n[models.'bad.name']",
            "schema_version = 1\n[[models.coder]]",
            "schema_version = 1\n[tui]\nanimation_fps = 4294967296",
            "schema_version = 1\n[tui]\nanimation_fps = -1",
            "schema_version = 1\n[context]\ncompaction_threshold = nan",
            "schema_version = 1\n[context]\ncompaction_threshold = inf",
            "schema_version = 1\n[tools.shell]\nenv = { TEST = 1 }",
            "schema_version = 1\n[tools.shell]\ninherit_env = ['TEST', 1]",
        ] {
            assert!(parse_settings(source).is_err());
        }
    }

    #[test]
    fn errors_are_bounded_and_do_not_echo_invalid_source() {
        for source in [
            "schema_version = 'synthetic-secret'",
            "schema_version = 1\n[tui\nsynthetic-secret",
            "schema_version = 1\nschema_version = 2 # synthetic-secret",
        ] {
            let error = parse_settings(source).unwrap_err();
            assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
        }
        assert_eq!(
            parse_settings("").unwrap_err().code,
            ConfigErrorCode::MissingVersion
        );
        assert_eq!(
            parse_settings(&" ".repeat(MAX_CONFIG_BYTES + 1))
                .unwrap_err()
                .code,
            ConfigErrorCode::TooLarge
        );
        assert_eq!(
            parse_settings("schema_version = 2").unwrap_err().validation[0].code,
            ValidationCode::UnsupportedVersion
        );
    }

    #[test]
    fn file_and_direct_submissions_share_cross_field_validation() {
        let source = format!("{MODEL}\ncontext_window = 4096");
        let file_error = parse_settings(&source).unwrap_err();
        let mut settings = parse_settings(MODEL).unwrap();
        settings.models.get_mut("coder").unwrap().context_window = 4096;
        assert_eq!(file_error.validation, settings.validate().unwrap_err());
        assert_eq!(
            encode_settings(&settings).unwrap_err().validation,
            file_error.validation
        );
    }

    #[test]
    fn references_round_trip_without_credential_resolution() {
        for extra in [
            "auth = 'env'\napi_key_env = 'FIXTURE_API_KEY'",
            "auth = 'credential'\ncredential_ref = 'fixture-reference'",
        ] {
            let settings = parse_settings(&format!("{MODEL}{extra}")).unwrap();
            assert_eq!(
                parse_settings(&encode_settings(&settings).unwrap()).unwrap(),
                settings
            );
            let view = format!("{:?}", redacted_values(&settings));
            assert!(!view.contains("FIXTURE_API_KEY"));
            assert!(!view.contains("fixture-reference"));
            assert!(!view.contains("127.0.0.1"));
        }
        assert!(
            parse_settings(&format!(
                "{MODEL}auth = 'none'\napi_key_env = 'FIXTURE_API_KEY'"
            ))
            .is_err()
        );
    }

    #[test]
    fn data_is_not_interpolated_or_executed() {
        let source = "schema_version = 1\n[project]\nname = '$(do-not-execute)'\n[tools.shell]\nenv = { FIXTURE = '${UNSET_VARIABLE}' }";
        let settings = parse_settings(source).unwrap();
        assert_eq!(settings.project.name, "$(do-not-execute)");
        assert_eq!(settings.tools.shell.env["FIXTURE"], "${UNSET_VARIABLE}");
        assert_eq!(
            parse_settings(&encode_settings(&settings).unwrap()).unwrap(),
            settings
        );
    }

    #[test]
    fn typed_draft_edit_preserves_unrelated_comments_and_revalidates() {
        let original = "schema_version = 1\n# keep this\n[project]\nname = 'fixture' # untouched\n[tui]\nanimation_fps = 60\n";
        let edited = update_draft(original, "tui.animation_fps", SettingValue::Integer(0)).unwrap();
        assert!(edited.contains("# keep this"));
        assert!(edited.contains("name = 'fixture' # untouched"));
        assert_eq!(parse_settings(&edited).unwrap().tui.animation_fps, 0);
        assert!(update_draft(original, "harness.max_turns", SettingValue::Integer(0)).is_err());
        assert!(
            update_draft(
                original,
                "tui.animation_fps",
                SettingValue::Text("invalid".to_owned())
            )
            .is_err()
        );
        assert!(update_draft(original, "unknown", SettingValue::Boolean(true)).is_err());
        assert!(update_draft(original, "tui.animation_fps", SettingValue::Unset).is_err());
        assert_eq!(parse_settings(original).unwrap().tui.animation_fps, 60);
    }
}
