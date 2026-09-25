pub mod config;
pub mod scenario;

#[cfg(test)]
mod scenario_tests;

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
