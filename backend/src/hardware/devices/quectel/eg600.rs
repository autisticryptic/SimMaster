//! EG600-family classification, isolated from the EC2x-compatible path.

pub(super) fn matches(model: &str) -> bool {
    model.contains("EG600")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_the_eg600_family() {
        assert!(matches("EG600U-EA"));
        assert!(!matches("EG25-G"));
    }
}
