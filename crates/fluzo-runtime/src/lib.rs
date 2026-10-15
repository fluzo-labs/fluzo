pub mod artifact;
pub mod config;
pub mod configuration;
mod configuration_file;
pub mod migrations;
pub mod model_discovery;
pub mod scenario;
pub mod store;

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
