//! Pure gateway unit tests: endpoint parsing, consent rules,
//! credential-reference handling and stable error codes. The HTTP
//! behavior is covered by the simulator integration suite.

use fluzo_core::admission::RequestClass;
use fluzo_core::model_discovery::AddressClass;

use super::*;

fn target(value: &str) -> Result<(String, u16), GatewayError> {
    chat_target(value).map(|target| (target.host.clone(), target.port))
}

#[test]
fn endpoint_accepts_plain_hosts_and_v1_paths() {
    assert_eq!(
        target("http://127.0.0.1:8080").unwrap(),
        ("127.0.0.1".to_owned(), 8080)
    );
    assert_eq!(
        target("http://model.local/v1").unwrap(),
        ("model.local".to_owned(), 80)
    );
    assert_eq!(
        target("http://model.local:3000/v1/").unwrap(),
        ("model.local".to_owned(), 3000)
    );
}

#[test]
fn endpoint_rejects_everything_outside_the_contract() {
    for value in [
        "https://host/v1",      // tls not wired here
        "http://host/v1?key=1", // query
        "http://user@host/v1",  // userinfo
        "http://host/v1#frag",  // fragment
        "http://host/%2ev1",    // percent escapes
        "http://host:0",        // zero port
        "http:///v1",           // empty host
        "http://host/v2",       // unexpected path
        "ftp://host",           // scheme
        "http://host/é",        // non-ascii
        "http://host\nEvil: x", // control characters
    ] {
        assert_eq!(
            target(value).err(),
            Some(GatewayError::InvalidEndpoint),
            "must reject {value}"
        );
    }
}

#[test]
fn loopback_only_allows_loopback_and_reports_the_rest() {
    let gate = LoopbackOnlyConsent;
    assert!(
        gate.allow("http://127.0.0.1:8080", AddressClass::Loopback)
            .is_ok()
    );
    for class in [AddressClass::Private, AddressClass::Public] {
        assert_eq!(
            gate.allow("http://box.lan:8080", class).err(),
            Some(ConsentError::Required { class }),
            "non-loopback without acceptance is Required, never a silent connect"
        );
    }
}

#[test]
fn accepted_endpoints_are_class_bound() {
    let gate = AcceptedEndpoints::new();
    gate.accept("http://box.lan:8080".to_owned(), AddressClass::Private);
    assert!(
        gate.allow("http://box.lan:8080", AddressClass::Private)
            .is_ok(),
        "the accepted class may connect"
    );
    assert_eq!(
        gate.allow("http://box.lan:8080", AddressClass::Public)
            .err(),
        Some(ConsentError::Denied {
            class: AddressClass::Public
        }),
        "a class change after acceptance fails closed"
    );
    assert_eq!(
        gate.allow("http://other.lan:8080", AddressClass::Private)
            .err(),
        Some(ConsentError::Required {
            class: AddressClass::Private
        })
    );
}

#[test]
fn unresolved_acceptance_is_ignored_and_revocation_works() {
    let gate = AcceptedEndpoints::new();
    gate.accept("http://box.lan:8080".to_owned(), AddressClass::Unresolved);
    assert_eq!(
        gate.allow("http://box.lan:8080", AddressClass::Private)
            .err(),
        Some(ConsentError::Required {
            class: AddressClass::Private
        }),
        "an unresolved acceptance is never a usable acceptance"
    );
    gate.accept("http://box.lan:8080".to_owned(), AddressClass::Private);
    gate.revoke("http://box.lan:8080");
    assert!(
        gate.allow("http://box.lan:8080", AddressClass::Private)
            .is_err()
    );
}

#[test]
fn unresolved_class_never_connects() {
    let gate = AcceptedEndpoints::new();
    gate.accept("http://box.lan:8080".to_owned(), AddressClass::Private);
    assert_eq!(
        gate.allow("http://box.lan:8080", AddressClass::Unresolved)
            .err(),
        Some(ConsentError::Denied {
            class: AddressClass::Unresolved
        })
    );
}

struct ScriptedSecrets(Option<String>);

impl CredentialSource for ScriptedSecrets {
    fn resolve(&self, _name: &str) -> Option<String> {
        self.0.clone()
    }
}

fn gateway_with(secrets: ScriptedSecrets, key: Option<&str>) -> ProviderGateway {
    use crate::admission::AdmissionController;
    use crate::telemetry::{DiagnosticHub, HubLimits, RedactionPolicy};
    let config = GatewayConfig {
        endpoint: "http://127.0.0.1:9".to_owned(),
        api_key_env: key.map(str::to_owned),
        pool: PoolId("p".to_owned()),
        capacity: ModelId("m".to_owned()),
        retries: 0,
        connect_timeout: Duration::from_secs(1),
        first_output_timeout: Duration::from_secs(1),
        idle_stream_timeout: Duration::from_secs(1),
        request_timeout: Duration::from_secs(2),
    };
    ProviderGateway::new(
        config,
        std::sync::Arc::new(AdmissionController::new()),
        std::sync::Arc::new(LoopbackOnlyConsent),
        std::sync::Arc::new(secrets),
        std::sync::Arc::new(DiagnosticHub::new(
            HubLimits::default(),
            RedactionPolicy::default(),
        )),
        ResourceIdentity {
            service_name: "test".to_owned(),
            service_version: "0".to_owned(),
        },
    )
}

#[test]
fn credential_reference_resolution_never_leaks_through_errors() {
    let gateway = gateway_with(ScriptedSecrets(None), Some("FLUZO_TEST_KEY"));
    assert_eq!(
        gateway.authorization().err(),
        Some(GatewayError::CredentialUnavailable)
    );

    let gateway = gateway_with(
        ScriptedSecrets(Some("sk-value".to_owned())),
        Some("FLUZO_TEST_KEY"),
    );
    let header = gateway.authorization().unwrap().expect("header built");
    assert_eq!(header, "Bearer sk-value");
    // Debug of the header is the leak surface the gateway must avoid;
    // the error type never carries the value in the first place.
    assert!(!format!("{:?}", gateway.config.endpoint).contains("sk-value"));

    let too_long = "x".repeat(5000);
    for bad in ["", too_long.as_str(), "has\nnewline"] {
        let gateway = gateway_with(ScriptedSecrets(Some(bad.to_owned())), Some("K"));
        assert_eq!(
            gateway.authorization().err(),
            Some(GatewayError::CredentialInvalid),
            "invalid secret shape fails typed"
        );
    }
}

#[test]
fn credential_reference_name_is_validated() {
    for bad in ["", "1ABC", "has-dash", "a".repeat(129).as_str()] {
        let gateway = gateway_with(ScriptedSecrets(Some("x".to_owned())), Some(bad));
        assert_eq!(
            gateway.authorization().err(),
            Some(GatewayError::CredentialInvalid),
            "invalid reference name {bad:?}"
        );
    }
    let gateway = gateway_with(ScriptedSecrets(Some("x".to_owned())), Some("_MY_KEY2"));
    assert!(gateway.authorization().unwrap().is_some());
}

#[test]
fn error_codes_are_stable_machine_readable() {
    let cases: Vec<(GatewayError, &str)> = vec![
        (GatewayError::InvalidEndpoint, "invalid_endpoint"),
        (
            GatewayError::Consent(ConsentError::Required {
                class: AddressClass::Private,
            }),
            "consent_required",
        ),
        (
            GatewayError::Consent(ConsentError::Denied {
                class: AddressClass::Public,
            }),
            "consent_denied",
        ),
        (GatewayError::RedirectBlocked, "redirect_blocked"),
        (
            GatewayError::Rejected {
                status: 429,
                rejection: fluzo_core::admission::ProviderRejection::RateLimited,
            },
            "rate_limited",
        ),
        (
            GatewayError::Timeout(TimeoutKind::FirstOutput),
            "timeout_first_output",
        ),
        (
            GatewayError::Stream(ChatError::InvalidToolCall {
                index: 0,
                reason: "x".to_owned(),
            }),
            "stream_invalid_tool_call",
        ),
        (GatewayError::Cancelled, "cancelled"),
        (
            GatewayError::PartialStream(Box::new(super::PartialStreamInfo {
                text: String::new(),
                finish_reasons: Vec::new(),
                usage: None,
            })),
            "partial_stream",
        ),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code);
    }
}

#[test]
fn purpose_codes_cover_every_request_class() {
    assert_eq!(purpose_code(RequestClass::AgentTurn), "agent_turn");
    assert_eq!(purpose_code(RequestClass::Compaction), "compaction");
    assert_eq!(purpose_code(RequestClass::SetupProbe), "setup_probe");
    assert_eq!(purpose_code(RequestClass::LiveTest), "live_test");
    assert_eq!(
        purpose_code(RequestClass::ShadowObservation),
        "shadow_observation"
    );
}
