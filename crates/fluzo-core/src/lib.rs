pub mod application;
pub mod settings;
mod settings_validation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeAvailability {
    NotImplemented,
}
