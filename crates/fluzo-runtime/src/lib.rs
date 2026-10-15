pub mod artifact;
pub mod config;
pub mod configuration;
mod configuration_file;
pub mod history;
pub mod migrations;
pub mod model_discovery;
pub mod purge;
pub mod quota;
pub mod scenario;
pub mod store;
pub mod telemetry;
pub mod telemetry_archive;
pub mod telemetry_audit;
pub mod telemetry_purge;

#[cfg(test)]
mod scenario_tests;

#[cfg(test)]
#[path = "artifact_tests.rs"]
mod artifact_tests;

#[cfg(test)]
#[path = "migration_tests.rs"]
mod migration_tests;

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;

#[cfg(test)]
mod test_diagnostics;

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod telemetry_tests;

#[cfg(test)]
#[path = "telemetry_purge_tests.rs"]
mod telemetry_purge_tests;

#[cfg(test)]
#[path = "quota_integration_tests.rs"]
mod quota_integration_tests;

#[cfg(test)]
#[path = "purge_tests.rs"]
mod purge_tests;

#[cfg(test)]
#[path = "history_tests.rs"]
mod history_tests;

use fluzo_core::RuntimeAvailability;

pub fn availability() -> RuntimeAvailability {
    RuntimeAvailability::NotImplemented
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_does_not_claim_execution_is_available() {
        assert_eq!(availability(), RuntimeAvailability::NotImplemented);
    }
}
