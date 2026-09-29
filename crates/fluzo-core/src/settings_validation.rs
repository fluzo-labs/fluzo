use crate::settings::*;
use std::collections::{BTreeMap, BTreeSet};

impl Settings {
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();
        for (descriptor, value) in self.entries() {
            let invalid = match (&descriptor.constraint, &value) {
                (Constraint::Positive, SettingValue::Integer(value)) => *value == 0,
                (Constraint::Fraction, SettingValue::Fraction(value)) => {
                    !value.is_finite() || *value <= 0.0 || *value >= 1.0
                }
                (Constraint::FixedFalse, SettingValue::Boolean(value)) => *value,
                (Constraint::SchemaVersion, SettingValue::Integer(value)) => {
                    *value != u64::from(SCHEMA_VERSION)
                }
                _ => false,
            };
            let out_of_range = matches!(value, SettingValue::Integer(value)
                if value > descriptor.integer_maximum.unwrap_or(i64::MAX as u64));
            if let (SettingKind::Choice(options), SettingValue::Text(value)) =
                (&descriptor.kind, &value)
                && !options.contains(value)
            {
                errors.push(ValidationError::new(
                    safe_key(&descriptor.key),
                    ValidationCode::UnsupportedValue,
                ));
            }
            if invalid || out_of_range {
                let code = match descriptor.constraint {
                    Constraint::SchemaVersion => ValidationCode::UnsupportedVersion,
                    Constraint::FixedFalse => ValidationCode::UnsupportedValue,
                    _ => ValidationCode::OutOfRange,
                };
                errors.push(ValidationError::new(safe_key(&descriptor.key), code));
            }
        }
        for (name, pool) in &self.capacity_pools {
            let prefix = collection_key("capacity_pools", name);
            if !valid_identifier(name) {
                errors.push(ValidationError::new(
                    &prefix,
                    ValidationCode::InvalidReference,
                ));
            }
            if pool.foreground_reserved_slots > pool.max_in_flight {
                errors.push(ValidationError::new(
                    format!("{prefix}.foreground_reserved_slots"),
                    ValidationCode::OutOfRange,
                ));
            }
        }
        self.validate_models(&mut errors);
        if let Some(name) = &self.agent.model
            && !self
                .models
                .get(name)
                .is_some_and(|model| model.provider == Provider::OpenAiCompatible)
        {
            errors.push(ValidationError::new(
                "agent.model",
                ValidationCode::MissingReference,
            ));
        }
        if self.decision.enabled && self.decision.model.is_none() {
            errors.push(ValidationError::new(
                "decision.model",
                ValidationCode::MissingSetting,
            ));
        }
        if let Some(name) = &self.decision.model
            && !self
                .models
                .get(name)
                .is_some_and(|model| model.provider == Provider::LayaSystemOne)
        {
            errors.push(ValidationError::new(
                "decision.model",
                ValidationCode::MissingReference,
            ));
        }
        if self.context.tool_context_max_bytes > self.context.tool_capture_max_bytes {
            errors.push(ValidationError::new(
                "context.tool_context_max_bytes",
                ValidationCode::OutOfRange,
            ));
        }
        if self.context.tool_capture_max_bytes > self.storage.session_max_bytes {
            errors.push(ValidationError::new(
                "context.tool_capture_max_bytes",
                ValidationCode::OutOfRange,
            ));
        }
        if self.harness.max_cost_microunits.is_some() != self.harness.currency.is_some()
            || self
                .harness
                .currency
                .as_ref()
                .is_some_and(|currency| !valid_currency(currency))
        {
            errors.push(ValidationError::new(
                "harness.currency",
                ValidationCode::MissingPrice,
            ));
        }
        self.validate_environment(&mut errors);
        if self.telemetry.export_enabled && self.telemetry.otlp_endpoint.is_none() {
            errors.push(ValidationError::new(
                "telemetry.otlp_endpoint",
                ValidationCode::MissingSetting,
            ));
        }
        if self
            .telemetry
            .otlp_endpoint
            .as_ref()
            .is_some_and(|endpoint| !valid_endpoint(endpoint))
        {
            errors.push(ValidationError::new(
                "telemetry.otlp_endpoint",
                ValidationCode::InvalidEndpoint,
            ));
        }
        if !valid_identifier(&self.tui.theme) {
            errors.push(ValidationError::new(
                "tui.theme",
                ValidationCode::InvalidReference,
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn validate_for_execution(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = self.validate().err().unwrap_or_default();
        if self.agent.model.is_none() {
            errors.push(ValidationError::new(
                "agent.model",
                ValidationCode::MissingSetting,
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn validate_models(&self, errors: &mut Vec<ValidationError>) {
        let mut identities: BTreeMap<&str, &ModelSettings> = BTreeMap::new();
        let mut backends: BTreeMap<(String, Option<&str>, bool), &str> = BTreeMap::new();
        for (name, model) in &self.models {
            let prefix = collection_key("models", name);
            if !valid_identifier(name) {
                errors.push(ValidationError::new(
                    &prefix,
                    ValidationCode::InvalidReference,
                ));
            }
            for (field, value) in [("capacity_id", &model.capacity_id), ("pool", &model.pool)] {
                if !valid_identifier(value) {
                    errors.push(ValidationError::new(
                        format!("{prefix}.{field}"),
                        ValidationCode::InvalidReference,
                    ));
                }
            }
            if !self.capacity_pools.contains_key(&model.pool) {
                errors.push(ValidationError::new(
                    format!("{prefix}.pool"),
                    ValidationCode::MissingReference,
                ));
            }
            let generative = model.provider == Provider::OpenAiCompatible;
            let endpoint = if generative {
                &model.base_url
            } else {
                &model.endpoint
            };
            let field = if generative { "base_url" } else { "endpoint" };
            match endpoint {
                None => errors.push(ValidationError::new(
                    format!("{prefix}.{field}"),
                    ValidationCode::MissingSetting,
                )),
                Some(value) if !valid_endpoint(value) => errors.push(ValidationError::new(
                    format!("{prefix}.{field}"),
                    ValidationCode::InvalidEndpoint,
                )),
                _ => {}
            }
            if (generative && model.endpoint.is_some())
                || (!generative && (model.base_url.is_some() || model.model.is_some()))
            {
                errors.push(ValidationError::new(
                    format!("{prefix}.provider"),
                    ValidationCode::ConflictingFields,
                ));
            }
            if generative
                && model
                    .model
                    .as_ref()
                    .is_none_or(|name| name.trim().is_empty() || name.chars().any(char::is_control))
            {
                errors.push(ValidationError::new(
                    format!("{prefix}.model"),
                    ValidationCode::MissingSetting,
                ));
            }
            let valid_auth = match model.auth {
                Authentication::None => {
                    model.api_key_env.is_none() && model.credential_ref.is_none()
                }
                Authentication::Environment => {
                    model
                        .api_key_env
                        .as_ref()
                        .is_some_and(|name| valid_environment_name(name))
                        && model.credential_ref.is_none()
                }
                Authentication::Credential => {
                    model
                        .credential_ref
                        .as_ref()
                        .is_some_and(|reference| valid_identifier(reference.name()))
                        && model.api_key_env.is_none()
                }
            };
            if !valid_auth {
                errors.push(ValidationError::new(
                    format!("{prefix}.auth"),
                    ValidationCode::InvalidReference,
                ));
            }
            if generative {
                let reserve = self.context.safety_reserve_tokens.max(
                    (model.context_window as f64 * self.context.safety_reserve_fraction).ceil()
                        as u64,
                );
                if model
                    .max_output_tokens
                    .checked_add(reserve)
                    .is_none_or(|reserved| reserved >= model.context_window)
                {
                    errors.push(ValidationError::new(
                        format!("{prefix}.context_window"),
                        ValidationCode::OutOfRange,
                    ));
                }
                if self.context.compaction_max_output_tokens > model.max_output_tokens {
                    errors.push(ValidationError::new(
                        "context.compaction_max_output_tokens",
                        ValidationCode::OutOfRange,
                    ));
                }
                if model.max_output_tokens >= self.harness.max_total_tokens {
                    errors.push(ValidationError::new(
                        "harness.max_total_tokens",
                        ValidationCode::OutOfRange,
                    ));
                }
                for (field, value) in [
                    ("connect_timeout_seconds", model.connect_timeout_seconds),
                    (
                        "first_output_timeout_seconds",
                        model.first_output_timeout_seconds,
                    ),
                    (
                        "idle_stream_timeout_seconds",
                        model.idle_stream_timeout_seconds,
                    ),
                ] {
                    if value > model.request_timeout_seconds {
                        errors.push(ValidationError::new(
                            format!("{prefix}.{field}"),
                            ValidationCode::OutOfRange,
                        ));
                    }
                }
            }
            let any_price = model.currency.is_some()
                || model.input_price_microunits_per_million.is_some()
                || model.output_price_microunits_per_million.is_some();
            if any_price
                && (!model
                    .currency
                    .as_ref()
                    .is_some_and(|currency| valid_currency(currency))
                    || model.input_price_microunits_per_million.is_none()
                    || model.output_price_microunits_per_million.is_none())
            {
                errors.push(ValidationError::new(
                    format!("{prefix}.currency"),
                    ValidationCode::MissingPrice,
                ));
            }
            if self.harness.max_cost_microunits.is_some()
                && (model.currency != self.harness.currency
                    || model.input_price_microunits_per_million.is_none()
                    || model.output_price_microunits_per_million.is_none())
            {
                errors.push(ValidationError::new(
                    format!("{prefix}.currency"),
                    ValidationCode::MissingPrice,
                ));
            }
            if let Some(previous) = identities.insert(&model.capacity_id, model)
                && (previous.provider != model.provider
                    || previous.pool != model.pool
                    || previous.max_in_flight != model.max_in_flight
                    || previous.base_url.as_deref().map(normalize_endpoint)
                        != model.base_url.as_deref().map(normalize_endpoint)
                    || previous.endpoint.as_deref().map(normalize_endpoint)
                        != model.endpoint.as_deref().map(normalize_endpoint)
                    || previous.model != model.model)
            {
                errors.push(ValidationError::new(
                    format!("{prefix}.capacity_id"),
                    ValidationCode::ConflictingFields,
                ));
            }
            if let Some(endpoint) = endpoint {
                let backend = (
                    normalize_endpoint(endpoint),
                    model.model.as_deref(),
                    generative,
                );
                if let Some(previous) = backends.insert(backend, &model.capacity_id)
                    && previous != model.capacity_id
                {
                    errors.push(ValidationError::new(
                        format!("{prefix}.capacity_id"),
                        ValidationCode::ConflictingFields,
                    ));
                }
            }
        }
    }

    fn validate_environment(&self, errors: &mut Vec<ValidationError>) {
        let mut names = BTreeSet::new();
        let credentials: BTreeSet<_> = self
            .models
            .values()
            .filter_map(|model| model.api_key_env.as_deref())
            .collect();
        for name in self
            .tools
            .shell
            .inherit_env
            .iter()
            .chain(self.tools.shell.env.keys())
        {
            if !valid_environment_name(name)
                || !names.insert(name)
                || credentials.contains(name.as_str())
                || reserved_environment(name)
            {
                errors.push(ValidationError::new(
                    "tools.shell",
                    ValidationCode::InvalidEnvironment,
                ));
            }
        }
        if self
            .tools
            .shell
            .env
            .values()
            .any(|value| value.contains('\0'))
        {
            errors.push(ValidationError::new(
                "tools.shell.env",
                ValidationCode::InvalidEnvironment,
            ));
        }
    }
}

fn valid_currency(currency: &str) -> bool {
    currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase())
}

fn valid_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

fn reserved_environment(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("OTEL_")
        || upper.starts_with("FLUZO_")
        || upper.ends_with("_API_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper.ends_with("_PASSWORD")
}

fn collection_key(collection: &str, name: &str) -> String {
    format!(
        "{collection}.{}",
        if valid_identifier(name) {
            name
        } else {
            "<invalid-id>"
        }
    )
}

fn safe_key(key: &str) -> String {
    key.split('.')
        .map(|segment| {
            if valid_identifier(segment) {
                segment
            } else {
                "<invalid-id>"
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn normalize_endpoint(endpoint: &str) -> String {
    if let Some((scheme, rest)) = endpoint.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let authority = authority.to_ascii_lowercase();
        let authority = if scheme == "https" {
            authority.strip_suffix(":443").unwrap_or(&authority)
        } else {
            authority.strip_suffix(":80").unwrap_or(&authority)
        };
        format!("{scheme}://{authority}/{}", path.trim_end_matches('/'))
    } else {
        endpoint.to_owned()
    }
}

fn valid_endpoint(endpoint: &str) -> bool {
    if endpoint
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
        || endpoint.contains(['@', '?', '#', '\\'])
    {
        return false;
    }
    let Some(rest) = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((address, suffix)) = ipv6.split_once(']') else {
            return false;
        };
        if address.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        if suffix.is_empty() {
            (address, None)
        } else if let Some(port) = suffix.strip_prefix(':') {
            (address, Some(port))
        } else {
            return false;
        }
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        if !host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }) {
            return false;
        }
        (host, port)
    };
    !host.is_empty() && port.is_none_or(|port| port.parse::<u16>().is_ok_and(|port| port > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> Settings {
        let mut settings = Settings::default();
        settings
            .capacity_pools
            .insert("local".to_owned(), CapacityPoolSettings::default());
        settings.models.insert(
            "coder".to_owned(),
            ModelSettings {
                base_url: Some("http://127.0.0.1:8080/v1".to_owned()),
                model: Some("fixture-model".to_owned()),
                pool: "local".to_owned(),
                capacity_id: "backend".to_owned(),
                ..ModelSettings::default()
            },
        );
        settings.agent.model = Some("coder".to_owned());
        settings
    }

    #[test]
    fn defaults_are_valid_but_never_authorize_execution() {
        let settings = Settings::default();
        assert_eq!(settings.validate(), Ok(()));
        assert_eq!(
            settings.validate_for_execution(),
            Err(vec![ValidationError::new(
                "agent.model",
                ValidationCode::MissingSetting
            )])
        );
        assert_eq!(configured().validate_for_execution(), Ok(()));
    }

    #[test]
    fn every_positive_and_fraction_descriptor_is_enforced() {
        for (descriptor, value) in configured().entries() {
            if descriptor.constraint == Constraint::Positive {
                assert!(
                    matches!(value, SettingValue::Integer(value) if value > 0)
                        || value == SettingValue::Unset
                );
            }
        }
        for threshold in [0.0, 1.0, -0.1, f64::NAN, f64::INFINITY] {
            let mut settings = configured();
            settings.context.compaction_threshold = threshold;
            assert!(
                settings
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|error| error.key == "context.compaction_threshold")
            );
        }
    }

    #[test]
    fn cross_field_constraints_reject_invalid_combinations() {
        type Mutation = fn(&mut Settings);
        let cases: Vec<(&str, Mutation)> = vec![
            ("schema_version", |settings| settings.schema_version = 2),
            ("agent.model", |settings| {
                settings.agent.model = Some("missing".to_owned())
            }),
            ("models.coder.pool", |settings| {
                settings.models.get_mut("coder").unwrap().pool = "missing".to_owned()
            }),
            (
                "capacity_pools.local.foreground_reserved_slots",
                |settings| {
                    settings
                        .capacity_pools
                        .get_mut("local")
                        .unwrap()
                        .foreground_reserved_slots = 2
                },
            ),
            ("models.coder.max_in_flight", |settings| {
                settings.models.get_mut("coder").unwrap().max_in_flight = 0
            }),
            ("models.coder.context_window", |settings| {
                settings.models.get_mut("coder").unwrap().context_window = 4096
            }),
            ("models.coder.first_output_timeout_seconds", |settings| {
                settings
                    .models
                    .get_mut("coder")
                    .unwrap()
                    .first_output_timeout_seconds = 181
            }),
            ("context.compaction_max_output_tokens", |settings| {
                settings.context.compaction_max_output_tokens = 4097
            }),
            ("context.tool_context_max_bytes", |settings| {
                settings.context.tool_context_max_bytes =
                    settings.context.tool_capture_max_bytes + 1
            }),
            ("context.tool_capture_max_bytes", |settings| {
                settings.storage.session_max_bytes = 1
            }),
            ("harness.max_total_tokens", |settings| {
                settings.harness.max_total_tokens = 4096
            }),
            ("harness.max_active_seconds", |settings| {
                settings.harness.max_active_seconds = u64::MAX
            }),
            ("storage.auto_expire", |settings| {
                settings.storage.auto_expire = true
            }),
            ("telemetry.otlp_endpoint", |settings| {
                settings.telemetry.export_enabled = true
            }),
            ("decision.model", |settings| {
                settings.decision.enabled = true
            }),
            ("decision.model", |settings| {
                settings.decision.model = Some("coder".to_owned())
            }),
        ];
        for (key, mutate) in cases {
            let mut settings = configured();
            mutate(&mut settings);
            assert!(
                settings
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|error| error.key == key),
                "{key}"
            );
        }
    }

    #[test]
    fn aliases_cannot_multiply_capacity_or_mix_roles() {
        let mut settings = configured();
        let alias = settings.models["coder"].clone();
        settings.models.insert("alias".to_owned(), alias);
        assert!(settings.validate().is_ok());
        settings.models.get_mut("alias").unwrap().capacity_id = "other".to_owned();
        assert!(
            settings
                .validate()
                .unwrap_err()
                .iter()
                .any(|error| error.code == ValidationCode::ConflictingFields)
        );
        settings.models.get_mut("alias").unwrap().capacity_id = "backend".to_owned();
        settings.models.get_mut("alias").unwrap().max_in_flight = 2;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn shadow_defaults_reserve_the_only_slot_for_foreground() {
        let mut settings = configured();
        settings.models.insert(
            "observer".to_owned(),
            ModelSettings {
                provider: Provider::LayaSystemOne,
                endpoint: Some("http://127.0.0.1:8080/laya".to_owned()),
                pool: "local".to_owned(),
                capacity_id: "observer".to_owned(),
                ..ModelSettings::default()
            },
        );
        settings.decision.model = Some("observer".to_owned());
        settings.decision.enabled = true;
        assert!(settings.validate().is_ok());
        assert_eq!(
            settings.capacity_pools["local"].max_in_flight
                - settings.capacity_pools["local"].foreground_reserved_slots,
            0
        );
        settings.models.get_mut("observer").unwrap().capacity_id = "backend".to_owned();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn credentials_and_environment_do_not_become_values_or_grants() {
        let mut settings = configured();
        let model = settings.models.get_mut("coder").unwrap();
        model.auth = Authentication::Environment;
        model.api_key_env = Some("TEST_PROVIDER_API_KEY".to_owned());
        assert!(settings.validate().is_ok());
        settings
            .tools
            .shell
            .inherit_env
            .push("TEST_PROVIDER_API_KEY".to_owned());
        let errors = settings.validate().unwrap_err();
        assert_eq!(errors[0].key, "tools.shell");
        assert!(!format!("{errors:?}").contains("TEST_PROVIDER_API_KEY"));
        settings.tools.shell.inherit_env.clear();
        settings.models.get_mut("coder").unwrap().credential_ref =
            Some(CredentialReference::new("fixture").unwrap());
        assert!(settings.validate().is_err());
    }

    #[test]
    fn monetary_budget_requires_matching_complete_prices() {
        let mut settings = configured();
        settings.harness.max_cost_microunits = Some(1_000_000);
        assert!(settings.validate().is_err());
        settings.harness.currency = Some("EUR".to_owned());
        let model = settings.models.get_mut("coder").unwrap();
        model.currency = Some("EUR".to_owned());
        model.input_price_microunits_per_million = Some(500_000);
        model.output_price_microunits_per_million = Some(1_000_000);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn endpoint_errors_never_echo_values() {
        for endpoint in [
            "https://user:synthetic-secret@example.invalid/v1",
            "file:///tmp/test",
            "http://host:0",
            "http://host/path?secret=value",
            "http://[bad]/",
            "https://",
        ] {
            let mut settings = configured();
            settings.models.get_mut("coder").unwrap().base_url = Some(endpoint.to_owned());
            let errors = settings.validate().unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == ValidationCode::InvalidEndpoint)
            );
            assert!(!format!("{errors:?}").contains(endpoint));
        }
    }

    #[test]
    fn zero_animation_and_zero_retries_are_supported() {
        let mut settings = configured();
        settings.tui.animation_fps = 0;
        settings.harness.provider_retries = 0;
        assert!(settings.validate().is_ok());
    }
}
