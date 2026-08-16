//! Shared classification for the closely related EC20/EC25/EG25 family.

use super::QuectelFamily;

pub(super) fn classify(model: &str) -> Option<QuectelFamily> {
    if model.contains("EC20") {
        Some(QuectelFamily::Ec20)
    } else if model.contains("EC25") {
        Some(QuectelFamily::Ec25)
    } else if model.contains("EG25") {
        Some(QuectelFamily::Eg25)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ec2x_models_on_one_driver_family() {
        assert_eq!(classify("EC20F"), Some(QuectelFamily::Ec20));
        assert_eq!(classify("EC25-E"), Some(QuectelFamily::Ec25));
        assert_eq!(classify("EG25-G"), Some(QuectelFamily::Eg25));
        assert_eq!(classify("EG600U-EA"), None);
    }
}
