use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Privacy {
    Public,
    Sensitive,
    CredentialReference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApplicationRule {
    Presentation,
    FutureTasks,
    ExplicitApply,
    Restart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SettingOrigin {
    BuiltIn,
    Repository,
    ExplicitFile,
    CommandLine,
    Draft,
    SavedFuture,
    ActiveSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    Boolean(bool),
    Integer(u64),
    Fraction(f64),
    Text(String),
    TextList(Vec<String>),
    TextMap(BTreeMap<String, String>),
    Unset,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SettingKind {
    Boolean,
    Integer,
    Fraction,
    Text,
    Choice(Vec<String>),
    TextList,
    TextMap,
    CredentialReference,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Constraint {
    Any,
    Positive,
    Fraction,
    FixedFalse,
    SchemaVersion,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingDescriptor {
    pub key: String,
    pub kind: SettingKind,
    pub optional: bool,
    pub integer_maximum: Option<u64>,
    pub unit: String,
    pub default: SettingValue,
    pub constraint: Constraint,
    pub description: String,
    pub privacy: Privacy,
    pub application: ApplicationRule,
}

trait SettingField {
    fn kind() -> SettingKind;
    fn value(&self) -> SettingValue;
    fn optional() -> bool {
        false
    }
    fn integer_maximum() -> Option<u64> {
        None
    }
}

macro_rules! scalar_field {
    ($type:ty, $kind:ident, $value:ident) => {
        impl SettingField for $type {
            fn kind() -> SettingKind {
                SettingKind::$kind
            }
            fn value(&self) -> SettingValue {
                SettingValue::$value(self.clone().into())
            }
        }
    };
}

scalar_field!(bool, Boolean, Boolean);
macro_rules! integer_field {
    ($type:ty, $maximum:expr) => {
        impl SettingField for $type {
            fn kind() -> SettingKind {
                SettingKind::Integer
            }
            fn value(&self) -> SettingValue {
                SettingValue::Integer(u64::from(*self))
            }
            fn integer_maximum() -> Option<u64> {
                Some($maximum)
            }
        }
    };
}
integer_field!(u32, u64::from(u32::MAX));
integer_field!(u64, i64::MAX as u64);
scalar_field!(f64, Fraction, Fraction);
scalar_field!(String, Text, Text);
scalar_field!(Vec<String>, TextList, TextList);
scalar_field!(BTreeMap<String, String>, TextMap, TextMap);

impl<Value: SettingField> SettingField for Option<Value> {
    fn kind() -> SettingKind {
        Value::kind()
    }
    fn value(&self) -> SettingValue {
        self.as_ref()
            .map_or(SettingValue::Unset, SettingField::value)
    }
    fn optional() -> bool {
        true
    }
    fn integer_maximum() -> Option<u64> {
        Value::integer_maximum()
    }
}

macro_rules! choices {
    ($name:ident, $default:ident, {$($variant:ident => $text:literal),+ $(,)?}) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }
        impl Default for $name {
            fn default() -> Self { Self::$default }
        }
        impl SettingField for $name {
            fn kind() -> SettingKind {
                SettingKind::Choice(vec![$($text.to_owned()),+])
            }
            fn value(&self) -> SettingValue {
                SettingValue::Text(match self { $(Self::$variant => $text),+ }.to_owned())
            }
        }
    };
}

choices!(AgentRuntime, Native, {Native => "native"});
choices!(Provider, OpenAiCompatible, {OpenAiCompatible => "openai_compatible", LayaSystemOne => "laya_systemone"});
choices!(Authentication, None, {None => "none", Environment => "env", Credential => "credential"});
choices!(DecisionMode, Shadow, {Shadow => "shadow"});
choices!(Permission, Ask, {Ask => "ask", Deny => "deny", Allow => "allow"});
choices!(ShellInput, Closed, {Closed => "closed"});
choices!(OtlpProtocol, HttpProtobuf, {HttpProtobuf => "http/protobuf"});

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialReference(String);

impl CredentialReference {
    pub fn new(name: impl Into<String>) -> Result<Self, ValidationError> {
        let name = name.into();
        if !valid_identifier(&name) {
            return Err(ValidationError::new(
                "credential_ref",
                ValidationCode::InvalidReference,
            ));
        }
        Ok(Self(name))
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialReference([redacted])")
    }
}

impl SettingField for CredentialReference {
    fn kind() -> SettingKind {
        SettingKind::CredentialReference
    }
    fn value(&self) -> SettingValue {
        SettingValue::Text(self.0.clone())
    }
}

trait Group {
    fn entries(&self, prefix: &str) -> Vec<(SettingDescriptor, SettingValue)>;
}

macro_rules! group {
    ($name:ident { $($field:ident: $type:ty = $default:expr => ($unit:literal, $constraint:ident, $privacy:ident, $application:ident, $description:literal)),+ $(,)? } $(, { $($nested:ident: $nested_type:ty),+ $(,)? })?) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name { $(pub $field: $type,)+ $($(pub $nested: $nested_type,)+)? }
        impl Default for $name {
            fn default() -> Self { Self { $($field: $default,)+ $($($nested: <$nested_type>::default(),)+)? } }
        }
        impl Group for $name {
            fn entries(&self, prefix: &str) -> Vec<(SettingDescriptor, SettingValue)> {
                let defaults = Self::default();
                vec![$((SettingDescriptor {
                    key: format!("{prefix}.{}", stringify!($field)),
                    kind: <$type as SettingField>::kind(),
                    optional: <$type as SettingField>::optional(),
                    integer_maximum: <$type as SettingField>::integer_maximum(),
                    unit: $unit.to_owned(),
                    default: defaults.$field.value(),
                    constraint: Constraint::$constraint,
                    description: $description.to_owned(),
                    privacy: Privacy::$privacy,
                    application: ApplicationRule::$application,
                }, self.$field.value())),+].into_iter()
                    $( $(.chain(self.$nested.entries(&format!("{prefix}.{}", stringify!($nested)))))* )?
                    .collect()
            }
        }
    };
}

group!(ProjectSettings {
    name: String = String::new() => ("", Any, Sensitive, FutureTasks, "Optional project display name")
});
group!(AgentSettings {
    runtime: AgentRuntime = AgentRuntime::Native => ("", Any, Public, Restart, "Built-in agent runtime"),
    model: Option<String> = None => ("", Any, Sensitive, ExplicitApply, "Selected generative model alias; setup is required when absent")
});
group!(ModelSettings {
    provider: Provider = Provider::OpenAiCompatible => ("", Any, Public, ExplicitApply, "Provider protocol, not an authorization to connect"),
    base_url: Option<String> = None => ("", Any, Sensitive, ExplicitApply, "Chat Completions base URL supplied by the user"),
    endpoint: Option<String> = None => ("", Any, Sensitive, ExplicitApply, "Laya endpoint supplied by the user"),
    model: Option<String> = None => ("", Any, Sensitive, ExplicitApply, "Provider model identifier"),
    auth: Authentication = Authentication::None => ("", Any, Public, ExplicitApply, "Authentication reference mode"),
    api_key_env: Option<String> = None => ("", Any, CredentialReference, ExplicitApply, "Name of a credential environment variable, never its value"),
    credential_ref: Option<CredentialReference> = None => ("", Any, CredentialReference, ExplicitApply, "Endpoint-scoped user-local credential reference"),
    context_window: u64 = 32768 => ("tokens", Positive, Public, ExplicitApply, "Setup suggestion requiring confirmation for the model"),
    max_output_tokens: u64 = 4096 => ("tokens", Positive, Public, ExplicitApply, "Maximum reserved response output"),
    capacity_id: String = String::new() => ("", Any, Sensitive, ExplicitApply, "Shared identity for aliases of the same backend model"),
    pool: String = String::new() => ("", Any, Sensitive, ExplicitApply, "Explicit capacity pool membership"),
    max_in_flight: u32 = 1 => ("requests", Positive, Public, ExplicitApply, "Per-process model ceiling"),
    connect_timeout_seconds: u64 = 10 => ("seconds", Positive, Public, ExplicitApply, "Provider connection deadline"),
    first_output_timeout_seconds: u64 = 90 => ("seconds", Positive, Public, ExplicitApply, "Deadline for actual output, not headers"),
    idle_stream_timeout_seconds: u64 = 30 => ("seconds", Positive, Public, ExplicitApply, "Maximum interval without model progress"),
    request_timeout_seconds: u64 = 180 => ("seconds", Positive, Public, ExplicitApply, "Whole generative request deadline"),
    currency: Option<String> = None => ("", Any, Public, ExplicitApply, "Three-letter currency for optional price data"),
    input_price_microunits_per_million: Option<u64> = None => ("microunits/million tokens", Any, Public, ExplicitApply, "Conservative input price including cached input"),
    output_price_microunits_per_million: Option<u64> = None => ("microunits/million tokens", Any, Public, ExplicitApply, "Conservative output price including reasoning")
});
group!(CapacityPoolSettings {
    max_in_flight: u32 = 1 => ("requests", Positive, Public, ExplicitApply, "Per-process pool ceiling, not physical server capacity"),
    foreground_reserved_slots: u32 = 1 => ("requests", Positive, Public, ExplicitApply, "Capacity unavailable to shadow work"),
    queue_capacity: u32 = 8 => ("requests", Positive, Public, ExplicitApply, "Maximum foreground waiters"),
    queue_timeout_seconds: u64 = 30 => ("seconds", Positive, Public, ExplicitApply, "Queue deadline bounded by remaining task time")
});
group!(DecisionSettings {
    enabled: bool = false => ("", Any, Public, ExplicitApply, "Enable optional shadow observations after consent"),
    mode: DecisionMode = DecisionMode::Shadow => ("", Any, Public, ExplicitApply, "Observations never route or execute work"),
    model: Option<String> = None => ("", Any, Sensitive, ExplicitApply, "Selected Laya model alias"),
    timeout_seconds: u64 = 5 => ("seconds", Positive, Public, ExplicitApply, "Whole observation deadline, without automatic retries"),
    max_observations_per_task: u32 = 30 => ("observations", Positive, Public, ExplicitApply, "Observation budget"),
    failure_pause_threshold: u32 = 3 => ("failures", Positive, Public, ExplicitApply, "Consecutive failures before pausing observations")
});
group!(HarnessSettings {
    max_turns: u32 = 30 => ("turns", Positive, Public, ExplicitApply, "Logical agent interactions per task"),
    max_active_seconds: u64 = 1200 => ("seconds", Positive, Public, ExplicitApply, "Active elapsed task time excluding human waits"),
    shell_timeout_seconds: u64 = 120 => ("seconds", Positive, Public, ExplicitApply, "Command deadline capped by remaining task time"),
    provider_retries: u32 = 2 => ("retries", Any, Public, ExplicitApply, "Additional safe transient retries; never quota or capacity recovery"),
    max_tool_calls: u32 = 100 => ("calls", Positive, Public, ExplicitApply, "Tool dispatch budget including verification"),
    max_total_tokens: u64 = 200000 => ("tokens", Positive, Public, ExplicitApply, "Input and output budget including compaction"),
    max_cost_microunits: Option<u64> = None => ("microunits", Positive, Public, ExplicitApply, "Optional monetary budget; requires currency and model prices"),
    currency: Option<String> = None => ("", Any, Public, ExplicitApply, "Three-letter currency of the optional monetary budget")
});
group!(ShellSettings {
    stdin: ShellInput = ShellInput::Closed => ("", Any, Public, ExplicitApply, "Child standard input is closed"),
    termination_grace_seconds: u64 = 5 => ("seconds", Positive, Public, ExplicitApply, "Graceful termination deadline"),
    inherit_env: Vec<String> = Vec::new() => ("", Any, Sensitive, ExplicitApply, "Explicit reviewed parent environment names"),
    env: BTreeMap<String, String> = BTreeMap::from([("RUST_BACKTRACE".to_owned(), "1".to_owned())]) => ("", Any, Sensitive, ExplicitApply, "Non-secret literal tool environment, never interpolation")
});
group!(PolicySettings {
    shell: Permission = Permission::Ask => ("", Any, Public, ExplicitApply, "Requested shell policy, not a user grant"),
    tool_network: Permission = Permission::Deny => ("", Any, Public, ExplicitApply, "Requested tool network policy"),
    git_commit: Permission = Permission::Ask => ("", Any, Public, ExplicitApply, "Requested Git commit policy"),
    git_push: Permission = Permission::Ask => ("", Any, Public, ExplicitApply, "Requested Git push policy")
});
group!(ContextSettings {
    compaction_threshold: f64 = 0.8 => ("fraction", Fraction, Public, ExplicitApply, "Fraction of available input capacity that triggers compaction"),
    safety_reserve_tokens: u64 = 1024 => ("tokens", Positive, Public, ExplicitApply, "Minimum additional input safety reserve"),
    safety_reserve_fraction: f64 = 0.05 => ("fraction", Fraction, Public, ExplicitApply, "Context fraction reserved in addition to response output"),
    compaction_max_output_tokens: u64 = 2048 => ("tokens", Positive, Public, ExplicitApply, "Compaction summary output reservation"),
    tool_context_max_bytes: u64 = 65536 => ("bytes", Positive, Public, ExplicitApply, "Tool content supplied to the model"),
    tool_capture_max_bytes: u64 = 2097152 => ("bytes", Positive, Public, ExplicitApply, "Combined stdout and stderr capture ceiling")
});
group!(StorageSettings {
    auto_expire: bool = false => ("", FixedFalse, Public, ExplicitApply, "Retained history never expires automatically"),
    session_max_bytes: u64 = 10737418240 => ("bytes", Positive, Public, ExplicitApply, "History admission quota, not automatic deletion"),
    diagnostic_max_bytes: u64 = 2147483648 => ("bytes", Positive, Public, ExplicitApply, "Local diagnostic capture quota"),
    min_free_bytes: u64 = 1073741824 => ("bytes", Positive, Public, ExplicitApply, "Free-space admission reserve")
});
group!(TelemetrySettings {
    export_enabled: bool = false => ("", Any, Public, ExplicitApply, "Opt-in export requiring separate destination consent"),
    otlp_protocol: OtlpProtocol = OtlpProtocol::HttpProtobuf => ("", Any, Public, Restart, "Initial OTLP transport"),
    otlp_endpoint: Option<String> = None => ("", Any, Sensitive, ExplicitApply, "User-selected OTLP endpoint; no default service is contacted"),
    capture_content: bool = false => ("", Any, Sensitive, ExplicitApply, "Content capture requires separate consent")
});
group!(TuiSettings {
    theme: String = "default".to_owned() => ("", Any, Public, Presentation, "Presentation theme identifier"),
    animation_fps: u32 = 60 => ("frames/second", Any, Public, Presentation, "Animation cadence; zero never disables input"),
    reduced_motion: bool = false => ("", Any, Public, Presentation, "Reduce animated motion"),
    dev_menu: bool = false => ("", Any, Public, Presentation, "Development presentation controls without runtime privileges")
}, { notifications: NotificationSettings, flags: UiFlags });
group!(NotificationSettings {
    desktop_enabled: bool = false => ("", Any, Public, Presentation, "Opt-in desktop notifications"),
    duration_seconds: u64 = 5 => ("seconds", Positive, Public, Presentation, "Transient notification duration"),
    max_visible: u32 = 3 => ("notifications", Positive, Public, Presentation, "Maximum visible notification stack")
});
group!(UiFlags {
    render_diagnostics: bool = false => ("", Any, Public, Presentation, "Display rendering diagnostics when implemented")
});

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolSettings {
    pub shell: ShellSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub project: ProjectSettings,
    pub agent: AgentSettings,
    pub models: BTreeMap<String, ModelSettings>,
    pub capacity_pools: BTreeMap<String, CapacityPoolSettings>,
    pub decision: DecisionSettings,
    pub harness: HarnessSettings,
    pub tools: ToolSettings,
    pub policy: PolicySettings,
    pub context: ContextSettings,
    pub storage: StorageSettings,
    pub telemetry: TelemetrySettings,
    pub tui: TuiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            project: ProjectSettings::default(),
            agent: AgentSettings::default(),
            models: BTreeMap::new(),
            capacity_pools: BTreeMap::new(),
            decision: DecisionSettings::default(),
            harness: HarnessSettings::default(),
            tools: ToolSettings::default(),
            policy: PolicySettings::default(),
            context: ContextSettings::default(),
            storage: StorageSettings::default(),
            telemetry: TelemetrySettings::default(),
            tui: TuiSettings::default(),
        }
    }
}

impl Settings {
    pub fn entries(&self) -> Vec<(SettingDescriptor, SettingValue)> {
        let mut entries = vec![(
            SettingDescriptor {
                key: "schema_version".to_owned(),
                kind: SettingKind::Integer,
                optional: false,
                integer_maximum: Some(u64::from(u32::MAX)),
                unit: String::new(),
                default: SettingValue::Integer(u64::from(SCHEMA_VERSION)),
                constraint: Constraint::SchemaVersion,
                description: "Supported configuration schema version".to_owned(),
                privacy: Privacy::Public,
                application: ApplicationRule::Restart,
            },
            SettingValue::Integer(u64::from(self.schema_version)),
        )];
        for (prefix, group) in [
            ("project", &self.project as &dyn Group),
            ("agent", &self.agent),
            ("decision", &self.decision),
            ("harness", &self.harness),
            ("tools.shell", &self.tools.shell),
            ("policy", &self.policy),
            ("context", &self.context),
            ("storage", &self.storage),
            ("telemetry", &self.telemetry),
            ("tui", &self.tui),
        ] {
            entries.extend(group.entries(prefix));
        }
        for (name, model) in &self.models {
            entries.extend(model.entries(&format!("models.{name}")));
        }
        for (name, pool) in &self.capacity_pools {
            entries.extend(pool.entries(&format!("capacity_pools.{name}")));
        }
        for (descriptor, _) in &mut entries {
            match descriptor.key.as_str() {
                "tui.animation_fps" => descriptor.integer_maximum = Some(60),
                "tui.theme" => {
                    descriptor.kind =
                        SettingKind::Choice(vec!["default".to_owned(), "high-contrast".to_owned()]);
                }
                _ => {}
            }
        }
        entries
    }

    pub fn descriptors(&self) -> Vec<SettingDescriptor> {
        self.entries()
            .into_iter()
            .map(|(descriptor, _)| descriptor)
            .collect()
    }

    pub fn collection_descriptors() -> Vec<SettingDescriptor> {
        ModelSettings::default()
            .entries("models.<id>")
            .into_iter()
            .chain(CapacityPoolSettings::default().entries("capacity_pools.<id>"))
            .map(|(descriptor, _)| descriptor)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ValidationCode {
    UnsupportedVersion,
    OutOfRange,
    InvalidReference,
    MissingReference,
    ConflictingFields,
    InvalidEndpoint,
    MissingSetting,
    UnsupportedValue,
    InvalidEnvironment,
    MissingPrice,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationError {
    pub key: String,
    pub code: ValidationCode,
}

impl ValidationError {
    pub fn new(key: impl Into<String>, code: ValidationCode) -> Self {
        Self {
            key: key.into(),
            code,
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {:?}", self.key, self.code)
    }
}

impl std::error::Error for ValidationError {}

pub fn valid_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_defaults_and_metadata_agree() {
        let settings = Settings::default();
        for (descriptor, value) in settings.entries() {
            assert_eq!(descriptor.default, value, "{}", descriptor.key);
            assert!(!descriptor.description.is_empty());
        }
        assert_eq!(settings.harness.max_turns, 30);
        assert_eq!(settings.harness.max_active_seconds, 1200);
        assert_eq!(settings.harness.provider_retries, 2);
        assert_eq!(settings.context.compaction_threshold, 0.8);
        assert_eq!(settings.tui.animation_fps, 60);
        assert!(settings.models.is_empty());
    }

    #[test]
    fn collection_metadata_is_tied_to_typed_fields() {
        let mut settings = Settings::default();
        settings
            .models
            .insert("coder".to_owned(), ModelSettings::default());
        settings
            .capacity_pools
            .insert("local".to_owned(), CapacityPoolSettings::default());
        let templates = Settings::collection_descriptors();
        for descriptor in settings.descriptors().into_iter().filter(|descriptor| {
            descriptor.key.starts_with("models.") || descriptor.key.starts_with("capacity_pools.")
        }) {
            let template_key = descriptor
                .key
                .replace(".coder.", ".<id>.")
                .replace(".local.", ".<id>.");
            let template = templates
                .iter()
                .find(|template| template.key == template_key)
                .unwrap();
            assert_eq!(template.default, descriptor.default);
            assert_eq!(template.kind, descriptor.kind);
        }
    }

    #[test]
    fn credentials_are_references_not_debug_values() {
        let reference = CredentialReference::new("test-credential").unwrap();
        assert_eq!(reference.name(), "test-credential");
        assert!(!format!("{reference:?}").contains("test-credential"));
        assert!(CredentialReference::new("literal credential with spaces").is_err());
    }
}
