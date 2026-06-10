use rust_decimal::Decimal;
use std::str::FromStr;

/// Normalize a Decimal string: strip trailing zeros after decimal point.
fn normalize(s: &str) -> String {
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        if trimmed.is_empty() || trimmed == "-" {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        s.to_string()
    }
}

/// Format a Decimal for display (LCD, history).
///
/// For numbers that fit within 14 characters, uses standard notation.
/// For longer numbers, switches to scientific notation with ~10 significant digits.
pub fn format_display(value: &Decimal) -> String {
    let s = normalize(&value.to_string());
    if s.len() <= 14 {
        return s;
    }
    format_scientific(value)
}

/// Format a Decimal for speech decomposition.
///
/// Always uses standard notation (no scientific notation) so the speech
/// engine can parse the integer and decimal parts correctly.
pub fn format_for_speech(value: &Decimal) -> String {
    normalize(&value.to_string())
}

fn format_scientific(value: &Decimal) -> String {
    let f: f64 = value.to_string().parse().unwrap_or(0.0);
    if f == 0.0 {
        return "0".to_string();
    }
    // 10 significant digits (9 decimal places in mantissa)
    let s = format!("{:.9e}", f);
    if let Some(e_pos) = s.find('e') {
        let mantissa = &s[..e_pos];
        let exponent = &s[e_pos..];
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{}{}", trimmed, exponent)
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

    #[test]
    fn format_large_integer_scientific() {
        let d = Decimal::from(123456789012345_i64);
        let s = format_display(&d);
        assert!(s.contains('e'), "expected scientific notation, got: {}", s);
    }

    #[test]
    fn format_for_speech_no_scientific() {
        let d = Decimal::from(123456789012345_i64);
        let s = format_for_speech(&d);
        assert!(!s.contains('e'), "speech format should not use scientific notation: {}", s);
        assert_eq!(s, "123456789012345");
    }
}