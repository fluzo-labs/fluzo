use fluzo_core::RuntimeAvailability;

pub fn availability_text(availability: RuntimeAvailability) -> &'static str {
    match availability {
        RuntimeAvailability::NotImplemented => "The agent runtime is not implemented yet.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_status_is_explicit() {
        assert_eq!(
            availability_text(RuntimeAvailability::NotImplemented),
            "The agent runtime is not implemented yet."
        );
    }
}
