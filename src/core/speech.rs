use super::token::VocalEvent;
use rust_decimal::Decimal;

/// WAV file indices for normal voice assets.
///
/// These map 1:1 to the files in `resource/Vocal/Normal/` by their numeric prefix.
pub const WAV_LING: u16 = 0; // 00_0x00_零.wav
pub const WAV_YI: u16 = 1; // 01_0x01_一.wav
pub const WAV_ER: u16 = 2; // 02_0x02_二.wav
pub const WAV_SAN: u16 = 3; // 03_0x03_三.wav
pub const WAV_SI: u16 = 4; // 04_0x04_四.wav
pub const WAV_WU: u16 = 5; // 05_0x05_五.wav
pub const WAV_LIU: u16 = 6; // 06_0x06_六.wav
pub const WAV_QI: u16 = 7; // 07_0x07_七.wav
pub const WAV_BA: u16 = 8; // 08_0x08_八.wav
pub const WAV_JIU: u16 = 9; // 09_0x09_九.wav
pub const WAV_SHI: u16 = 10; // 10_0x0A_十.wav
pub const WAV_BAI: u16 = 11; // 11_0x0B_百.wav
pub const WAV_QIAN: u16 = 12; // 12_0x0C_千.wav
pub const WAV_WAN: u16 = 13; // 13_0x0D_万.wav
pub const WAV_YI_UNIT: u16 = 14; // 14_0x0E_亿.wav
pub const WAV_DIVIDE: u16 = 15; // 15_0x0F_除以.wav
pub const WAV_MULTIPLY: u16 = 16; // 16_0x10_乘以.wav
pub const WAV_SUBTRACT: u16 = 17; // 17_0x11_减.wav
pub const WAV_ADD: u16 = 18; // 18_0x12_加.wav
pub const WAV_DIAN: u16 = 19; // 19_0x13_点.wav
pub const WAV_PERCENT: u16 = 20; // 20_0x14_百分比.wav
pub const WAV_BACKSPACE: u16 = 21; // 21_0x15_退位.wav
pub const WAV_EQUAL: u16 = 22; // 22_0x16_等于.wav
pub const WAV_ERROR: u16 = 23; // 23_0x17_错误.wav
pub const WAV_MEMORY: u16 = 24; // 24_0x18_记忆.wav
pub const WAV_NEGATIVE: u16 = 25; // 25_0x19_负.wav
pub const WAV_POSITIVE: u16 = 26; // 26_0x1A_正.wav
pub const WAV_ZERO: u16 = 27; // 27_0x1B_归零.wav
pub const WAV_CLEAR: u16 = 28; // 28_0x1C_清除.wav
pub const WAV_MU: u16 = 29; // 29_0x1D_MU.wav
pub const WAV_SUM: u16 = 30; // 30_0x1E_总和.wav
pub const WAV_SQRT: u16 = 31; // 31_0x1F_平方根.wav

/// Convert a VocalEvent to a sequence of WAV file indices for normal speech mode.
pub fn event_to_wav_indices(event: &VocalEvent) -> Vec<u16> {
    match event {
        VocalEvent::Digit(d) => vec![*d as u16],
        VocalEvent::DecimalPoint => vec![WAV_DIAN],
        VocalEvent::Operator(op) => match op {
            super::token::BinaryOp::Add => vec![WAV_ADD],
            super::token::BinaryOp::Subtract => vec![WAV_SUBTRACT],
            super::token::BinaryOp::Multiply => vec![WAV_MULTIPLY],
            super::token::BinaryOp::Divide => vec![WAV_DIVIDE],
        },
        VocalEvent::Equals => vec![WAV_EQUAL],
        VocalEvent::Percent => vec![WAV_PERCENT],
        VocalEvent::MU => vec![WAV_MU],
        VocalEvent::SquareRoot => vec![WAV_SQRT],
        VocalEvent::Backspace => vec![WAV_BACKSPACE],
        VocalEvent::Clear => vec![WAV_CLEAR],
        VocalEvent::AllClear => vec![WAV_ZERO],
        VocalEvent::MemoryRecall
        | VocalEvent::MemoryAdd
        | VocalEvent::MemorySubtract
        | VocalEvent::MemoryClear => vec![WAV_MEMORY],
        VocalEvent::SignNegative => vec![WAV_NEGATIVE],
        VocalEvent::SignPositive => vec![WAV_POSITIVE],
        VocalEvent::Error(_) => vec![WAV_ERROR],
        VocalEvent::Result(d) => decimal_to_speech_wavs(d),
    }
}

/// Decompose a Decimal into Chinese number speech WAV indices.
///
/// Supports integer part up to 10^12 and up to 12 decimal places.
/// For values exceeding the natural speech range, falls back to digit-by-digit.
pub fn decimal_to_speech_wavs(value: &Decimal) -> Vec<u16> {
    let s = super::format::format_for_speech(value);
    let mut result = Vec::new();

    // Handle negative
    let (s, is_negative) = if let Some(rest) = s.strip_prefix('-') {
        (rest, true)
    } else {
        (s.as_str(), false)
    };
    if is_negative {
        result.push(WAV_NEGATIVE);
    }

    // Split integer and decimal parts
    let (int_part, dec_part) = if let Some(dot_pos) = s.find('.') {
        (&s[..dot_pos], Some(&s[dot_pos + 1..]))
    } else {
        (s, None)
    };

    // Speak integer part
    if int_part.is_empty() {
        result.push(WAV_LING);
    } else {
        speak_integer(int_part, &mut result);
    }

    // Speak decimal part
    if let Some(dec) = dec_part.filter(|d| !d.is_empty()) {
        result.push(WAV_DIAN);
        for ch in dec.chars() {
            result.push(ch as u16 - b'0' as u16);
        }
    }

    result
}

/// Speak an integer string using Chinese number words.
///
/// Handles 亿(10^8), 万(10^4), 千(10^3), 百(10^2), 十(10^1).
fn speak_integer(s: &str, out: &mut Vec<u16>) {
    let digits: Vec<u8> = s.bytes().map(|b| b - b'0').collect();
    let len = digits.len();

    if len == 0 {
        out.push(WAV_LING);
        return;
    }

    // For numbers > 12 digits, fall back to digit-by-digit
    if len > 12 {
        for d in &digits {
            out.push(*d as u16);
        }
        return;
    }

    // Split into groups: 亿 (10^8), 万 (10^4), singles
    let mut remaining = digits.as_slice();

    // 亿 group (digits 9-12 from left if len >= 9)
    if len > 8 {
        let yi_group_len = len - 8;
        let yi_group = &remaining[..yi_group_len];
        remaining = &remaining[yi_group_len..];
        speak_group(yi_group, out, false);
        out.push(WAV_YI_UNIT);
        // If the 万 group or below is all zeros, skip them
        if remaining.iter().all(|&d| d == 0) {
            return;
        }
        // If the 万 group starts with zeros, emit 零
        if remaining[0] == 0 {
            out.push(WAV_LING);
        }
    }

    // 万 group (next 4 digits)
    if remaining.len() > 4 {
        let wan_len = remaining.len() - 4;
        let wan_group = &remaining[..wan_len];
        remaining = &remaining[wan_len..];
        speak_group(wan_group, out, false);
        out.push(WAV_WAN);
        if remaining.iter().all(|&d| d == 0) {
            return;
        }
        if remaining[0] == 0 {
            out.push(WAV_LING);
        }
    }

    // Last group (up to 4 digits: 千百十个)
    let is_leading = len <= 4; // whether this is the leading group (affects 十 handling)
    speak_group(remaining, out, is_leading);
}

/// Speak a group of up to 4 digits with 千百十 units.
///
/// `is_leading`: if true and group length is 2, the tens place "1" is omitted
/// (e.g., "十" not "一十").
fn speak_group(group: &[u8], out: &mut Vec<u16>, is_leading: bool) {
    let len = group.len();
    match len {
        4 => {
            // 千百十个
            emit_digit(group[0], out);
            out.push(WAV_QIAN);
            if group[1] != 0 {
                emit_digit(group[1], out);
                out.push(WAV_BAI);
                if group[2] != 0 {
                    emit_digit(group[2], out);
                    out.push(WAV_SHI);
                } else if group[3] != 0 {
                    out.push(WAV_LING);
                }
            } else if group[2] != 0 {
                out.push(WAV_LING);
                emit_digit(group[2], out);
                out.push(WAV_SHI);
            } else if group[3] != 0 {
                out.push(WAV_LING);
            }
            if group[3] != 0 {
                emit_digit(group[3], out);
            }
        }
        3 => {
            // 百十个
            emit_digit(group[0], out);
            out.push(WAV_BAI);
            if group[1] != 0 {
                emit_digit(group[1], out);
                out.push(WAV_SHI);
            } else if group[2] != 0 {
                out.push(WAV_LING);
            }
            if group[2] != 0 {
                emit_digit(group[2], out);
            }
        }
        2 => {
            // 十个
            if group[0] == 1 && is_leading {
                // "十" not "一十"
                out.push(WAV_SHI);
            } else {
                emit_digit(group[0], out);
                out.push(WAV_SHI);
            }
            if group[1] != 0 {
                emit_digit(group[1], out);
            }
        }
        1 => {
            emit_digit(group[0], out);
        }
        _ => {}
    }
}

fn emit_digit(d: u8, out: &mut Vec<u16>) {
    out.push(d as u16);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn wavs(s: &str) -> Vec<u16> {
        let d = Decimal::from_str(s).unwrap();
        decimal_to_speech_wavs(&d)
    }

    #[test]
    fn zero() {
        assert_eq!(wavs("0"), vec![WAV_LING]);
    }

    #[test]
    fn simple_digits() {
        assert_eq!(wavs("5"), vec![5]);
        assert_eq!(wavs("12"), vec![WAV_SHI, 2]);
        assert_eq!(wavs("10"), vec![WAV_SHI]);
        assert_eq!(wavs("20"), vec![2, WAV_SHI]);
    }

    #[test]
    fn hundreds() {
        // 100 => 一百
        assert_eq!(wavs("100"), vec![1, WAV_BAI]);
        // 123 => 一百二十三
        assert_eq!(wavs("123"), vec![1, WAV_BAI, 2, WAV_SHI, 3]);
        // 101 => 一百零一
        assert_eq!(wavs("101"), vec![1, WAV_BAI, WAV_LING, 1]);
    }

    #[test]
    fn thousands() {
        // 1000 => 一千
        assert_eq!(wavs("1000"), vec![1, WAV_QIAN]);
        // 1234 => 一千二百三十四
        assert_eq!(wavs("1234"), vec![1, WAV_QIAN, 2, WAV_BAI, 3, WAV_SHI, 4]);
        // 1001 => 一千零一
        assert_eq!(wavs("1001"), vec![1, WAV_QIAN, WAV_LING, 1]);
    }

    #[test]
    fn wan() {
        // 10000 => 一万
        assert_eq!(wavs("10000"), vec![1, WAV_WAN]);
        // 12345 => 一万二千三百四十五
        assert_eq!(
            wavs("12345"),
            vec![1, WAV_WAN, 2, WAV_QIAN, 3, WAV_BAI, 4, WAV_SHI, 5]
        );
    }

    #[test]
    fn yi_unit() {
        // 100000000 => 一亿
        assert_eq!(wavs("100000000"), vec![1, WAV_YI_UNIT]);
    }

    #[test]
    fn negative() {
        // -5 => 负五
        assert_eq!(wavs("-5"), vec![WAV_NEGATIVE, 5]);
    }

    #[test]
    fn decimal_value() {
        // 3.14 => 三点一四
        assert_eq!(wavs("3.14"), vec![3, WAV_DIAN, 1, 4]);
    }

    #[test]
    fn event_digit() {
        assert_eq!(event_to_wav_indices(&VocalEvent::Digit(7)), vec![7]);
    }

    #[test]
    fn event_decimal_point() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::DecimalPoint),
            vec![WAV_DIAN]
        );
    }

    #[test]
    fn event_operator_add() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Operator(super::super::token::BinaryOp::Add)),
            vec![WAV_ADD]
        );
    }

    #[test]
    fn event_operator_subtract() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Operator(super::super::token::BinaryOp::Subtract)),
            vec![WAV_SUBTRACT]
        );
    }

    #[test]
    fn event_operator_multiply() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Operator(super::super::token::BinaryOp::Multiply)),
            vec![WAV_MULTIPLY]
        );
    }

    #[test]
    fn event_operator_divide() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Operator(super::super::token::BinaryOp::Divide)),
            vec![WAV_DIVIDE]
        );
    }

    #[test]
    fn event_equals() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Equals),
            vec![WAV_EQUAL]
        );
    }

    #[test]
    fn event_percent() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Percent),
            vec![WAV_PERCENT]
        );
    }

    #[test]
    fn event_mu() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::MU),
            vec![WAV_MU]
        );
    }

    #[test]
    fn event_square_root() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::SquareRoot),
            vec![WAV_SQRT]
        );
    }

    #[test]
    fn event_backspace() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Backspace),
            vec![WAV_BACKSPACE]
        );
    }

    #[test]
    fn event_clear() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Clear),
            vec![WAV_CLEAR]
        );
    }

    #[test]
    fn event_all_clear() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::AllClear),
            vec![WAV_ZERO]
        );
    }

    #[test]
    fn event_memory_recall() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::MemoryRecall),
            vec![WAV_MEMORY]
        );
    }

    #[test]
    fn event_memory_add() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::MemoryAdd),
            vec![WAV_MEMORY]
        );
    }

    #[test]
    fn event_memory_subtract() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::MemorySubtract),
            vec![WAV_MEMORY]
        );
    }

    #[test]
    fn event_memory_clear() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::MemoryClear),
            vec![WAV_MEMORY]
        );
    }

    #[test]
    fn event_sign_negative() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::SignNegative),
            vec![WAV_NEGATIVE]
        );
    }

    #[test]
    fn event_sign_positive() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::SignPositive),
            vec![WAV_POSITIVE]
        );
    }

    #[test]
    fn event_error() {
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Error(super::super::token::CalcError::DivideByZero)),
            vec![WAV_ERROR]
        );
    }

    #[test]
    fn event_result() {
        let d = Decimal::from_str("5").unwrap();
        assert_eq!(
            event_to_wav_indices(&VocalEvent::Result(d)),
            vec![5]
        );
    }
}
