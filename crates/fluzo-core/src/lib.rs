pub mod application;
pub mod configuration;
pub mod model_discovery;
pub mod settings;
mod settings_validation;
pub mod telemetry;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeAvailability {
    NotImplemented,
}
