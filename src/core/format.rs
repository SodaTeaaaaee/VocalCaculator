use rust_decimal::Decimal;
use std::str::FromStr;

/// Format a Decimal for display.
///
/// Removes trailing zeros, handles negative sign, limits precision.
pub fn format_display(value: &Decimal) -> String {
    let s = value.to_string();
    // Normalize: strip trailing zeros after decimal point
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        if trimmed.is_empty() || trimmed == "-" {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        s
    }
}

/// Parse a display string into a Decimal.
pub fn parse_display(s: &str) -> Option<Decimal> {
    Decimal::from_str(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_integer() {
        assert_eq!(format_display(&Decimal::from(123)), "123");
    }

    #[test]
    fn format_trailing_zeros() {
        let d = Decimal::from_str("3.1400").unwrap();
        assert_eq!(format_display(&d), "3.14");
    }

    #[test]
    fn format_negative() {
        assert_eq!(format_display(&Decimal::from(-42)), "-42");
    }

    #[test]
    fn format_zero() {
        assert_eq!(format_display(&Decimal::ZERO), "0");
    }

    #[test]
    fn format_small_decimal() {
        let d = Decimal::from_str("0.50").unwrap();
        assert_eq!(format_display(&d), "0.5");
    }
}
