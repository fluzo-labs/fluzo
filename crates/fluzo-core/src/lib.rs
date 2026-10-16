pub mod admission;
pub mod application;
pub mod configuration;
pub mod context;
pub mod model_discovery;
pub mod policy;
pub mod settings;
mod settings_validation;
pub mod telemetry;

#[cfg(test)]
#[path = "context_tests.rs"]
mod context_tests;

#[cfg(test)]
#[path = "policy_tests.rs"]
mod policy_tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeAvailability {
    NotImplemented,
}
