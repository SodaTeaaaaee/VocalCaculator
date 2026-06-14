//! Music mode tone generation using AR7778 harmonic synthesis.
//!
//! Waveform algorithm ported from mkwave.py in the AR7778-digitized-MIDI
//! repository (https://github.com/evnchn-AR7778/AR7778-digitized-MIDI).
//! Sums odd harmonics with per-harmonic cosine envelope decay and
//! logarithmic attenuation, producing the characteristic buzzy/organ-like
//! calculator timbre.

use crate::core::token::{BinaryOp, VocalEvent};
use kira::sound::static_sound::StaticSoundData;
use std::io::Cursor;

const PI: f64 = std::f64::consts::PI;

// Note frequencies (Hz) - C major scale
const FREQ_C3: f64 = 130.81;
const FREQ_D3: f64 = 146.83;
const FREQ_E3: f64 = 164.81;
const FREQ_F3: f64 = 174.61;
const FREQ_G3: f64 = 196.00;
const FREQ_A3: f64 = 220.00;
const FREQ_B3: f64 = 246.94;
const FREQ_C4: f64 = 261.63;
const FREQ_D4: f64 = 293.66;
const FREQ_E4: f64 = 329.63;
const FREQ_F4: f64 = 349.23;
const FREQ_G4: f64 = 392.00;
const FREQ_A4: f64 = 440.00;
const FREQ_B4: f64 = 493.88;
const FREQ_C5: f64 = 523.25;
const FREQ_D5: f64 = 587.33;
const FREQ_E5: f64 = 659.26;

// Tone pool indices
const TONE_C3: usize = 0;
#[allow(dead_code)]
const TONE_D3: usize = 1;
const TONE_E3: usize = 2;
#[allow(dead_code)]
const TONE_F3: usize = 3;
const TONE_G3: usize = 4;
const TONE_A3: usize = 5;
const TONE_B3: usize = 6;
const TONE_C4: usize = 7;
const TONE_D4: usize = 8;
const TONE_E4: usize = 9;
const TONE_F4: usize = 10;
const TONE_G4: usize = 11;
const TONE_A4: usize = 12;
const TONE_B4: usize = 13;
const TONE_C5: usize = 14;
const TONE_D5: usize = 15;
const TONE_E5: usize = 16;
const TONE_CHORD_MAJOR: usize = 17;
#[allow(dead_code)]
const TONE_CHORD_DIS: usize = 18;
const TONE_ASCEND: usize = 19;
const TONE_DESCEND: usize = 20;
const TONE_CLICK: usize = 21;
const TONE_ERROR: usize = 22;

const SAMPLE_RATE: u32 = 44100;
const TONE_DURATION: f64 = 0.20;
const CHORD_DURATION: f64 = 0.30;

/// Holds pre-generated musical tones as `StaticSoundData` and raw WAV bytes.
pub struct MusicTones {
    sounds: Vec<StaticSoundData>,
    wav_bytes: Vec<Vec<u8>>,
}

impl MusicTones {
    pub fn new() -> Self {
        let mut sounds = Vec::with_capacity(23);
        let mut wav_bytes = Vec::with_capacity(23);

        let note_freqs: [f64; 17] = [
            FREQ_C3, FREQ_D3, FREQ_E3, FREQ_F3, FREQ_G3, FREQ_A3, FREQ_B3,
            FREQ_C4, FREQ_D4, FREQ_E4, FREQ_F4, FREQ_G4, FREQ_A4, FREQ_B4,
            FREQ_C5, FREQ_D5, FREQ_E5,
        ];
        for freq in &note_freqs {
            let wav = generate_tone_wav(*freq, TONE_DURATION, SAMPLE_RATE);
            sounds.push(wav_to_sound_data(&wav));
            wav_bytes.push(wav);
        }
        // Major chord
        let wav = generate_chord_wav(&[FREQ_C4, FREQ_E4, FREQ_G4], CHORD_DURATION, SAMPLE_RATE);
        sounds.push(wav_to_sound_data(&wav));
        wav_bytes.push(wav);
        // Dissonant chord
        let wav = generate_chord_wav(&[FREQ_C4, FREQ_D4, FREQ_E4], CHORD_DURATION, SAMPLE_RATE);
        sounds.push(wav_to_sound_data(&wav));
        wav_bytes.push(wav);
        // Ascending sweep
        let wav = generate_sweep_wav(&[FREQ_C4, FREQ_E4, FREQ_G4], 0.08, SAMPLE_RATE);
        sounds.push(wav_to_sound_data(&wav));
        wav_bytes.push(wav);
        // Descending sweep
        let wav = generate_sweep_wav(&[FREQ_G4, FREQ_E4, FREQ_C4], 0.08, SAMPLE_RATE);
        sounds.push(wav_to_sound_data(&wav));
        wav_bytes.push(wav);
        // Click
        let wav = generate_tone_wav(FREQ_C5, 0.05, SAMPLE_RATE);
        sounds.push(wav_to_sound_data(&wav));
        wav_bytes.push(wav);
        // Error
        let wav = generate_chord_wav(&[FREQ_C3, FREQ_D3, FREQ_E3], 0.4, SAMPLE_RATE);
        sounds.push(wav_to_sound_data(&wav));
        wav_bytes.push(wav);

        Self { sounds, wav_bytes }
    }

    pub fn get_sound(&self, index: usize) -> Option<&StaticSoundData> {
        self.sounds.get(index)
    }

    pub fn get_wav_bytes(&self, index: usize) -> Option<&[u8]> {
        self.wav_bytes.get(index).map(|v| v.as_slice())
    }

    pub fn count(&self) -> usize {
        self.sounds.len()
    }
}

impl Default for MusicTones {
    fn default() -> Self {
        Self::new()
    }
}

/// Map vocal events to tone indices for music mode.
pub fn events_to_tone_indices(events: &[VocalEvent]) -> Vec<usize> {
    let mut result = Vec::new();
    for event in events {
        match event {
            VocalEvent::Digit(d) => {
                result.push(match *d {
                    0 => TONE_C3, 1 => TONE_C4, 2 => TONE_D4, 3 => TONE_E4,
                    4 => TONE_F4, 5 => TONE_G4, 6 => TONE_A4, 7 => TONE_B4,
                    8 => TONE_C5, 9 => TONE_D5, _ => TONE_C4,
                });
            }
            VocalEvent::Operator(op) => {
                result.push(match op {
                    BinaryOp::Add => TONE_E5, BinaryOp::Subtract => TONE_G3,
                    BinaryOp::Multiply => TONE_A3, BinaryOp::Divide => TONE_E3,
                });
            }
            VocalEvent::DecimalPoint => result.push(TONE_B3),
            VocalEvent::Equals => result.push(TONE_CHORD_MAJOR),
            VocalEvent::Percent => { result.push(TONE_G4); result.push(TONE_C5); }
            VocalEvent::MU => result.push(TONE_F4),
            VocalEvent::SquareRoot => result.push(TONE_ASCEND),
            VocalEvent::Backspace => result.push(TONE_CLICK),
            VocalEvent::Clear => result.push(TONE_CLICK),
            VocalEvent::AllClear => result.push(TONE_DESCEND),
            VocalEvent::MemoryRecall | VocalEvent::MemoryAdd
            | VocalEvent::MemorySubtract | VocalEvent::MemoryClear => result.push(TONE_E4),
            VocalEvent::SignNegative => result.push(TONE_G3),
            VocalEvent::SignPositive => result.push(TONE_C4),
            VocalEvent::Error(_) => result.push(TONE_ERROR),
            VocalEvent::Result(d) => {
                let s = crate::core::format::format_for_speech(d);
                for ch in s.chars() {
                    if ch.is_ascii_digit() {
                        result.push(match ch as u8 - b'0' {
                            0 => TONE_C3, 1 => TONE_C4, 2 => TONE_D4, 3 => TONE_E4,
                            4 => TONE_F4, 5 => TONE_G4, 6 => TONE_A4, 7 => TONE_B4,
                            8 => TONE_C5, 9 => TONE_D5, _ => TONE_C4,
                        });
                    }
                }
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// AR7778 harmonic synthesis (ported from mkwave.py)
// ---------------------------------------------------------------------------

/// Generate an AR7778-style tone using odd-harmonic additive synthesis.
///
/// Each odd harmonic is a sine wave multiplied by a cosine envelope decay
/// and attenuated by `1 / 10^(ln(harmonic + 1) / 2)`.
fn generate_ar7778_tone(frequency: f64, duration: f64, sample_rate: u32) -> Vec<f64> {
    let num_samples = (sample_rate as f64 * duration) as usize;
    let mut samples = vec![0.0f64; num_samples];
    let nyquist = sample_rate as f64 / 2.0;
    let max_mult = (nyquist / frequency) as u32;

    for mult in (1..=max_mult).step_by(2) {
        let harm_freq = frequency * mult as f64;
        if harm_freq >= nyquist {
            break;
        }
        let attenuation = 1.0 / 10f64.powf((mult as f64 + 1.0).ln() / 2.0);
        let env_freq = mult as f64 / duration / 2.0;

        for (i, sample) in samples.iter_mut().enumerate() {
            let t = i as f64 / sample_rate as f64;
            let signal = (2.0 * PI * harm_freq * t).sin();
            let envelope = (2.0 * PI * env_freq * t).cos();
            *sample += signal * envelope * attenuation;
        }
    }

    let max_val = samples.iter().map(|s| s.abs()).fold(0.0f64, f64::max);
    if max_val > 0.0 {
        for s in &mut samples {
            *s /= max_val;
        }
    }
    samples
}

/// Convert f64 samples to 16-bit PCM mono WAV bytes.
fn samples_to_wav(samples: &[f64], sample_rate: u32) -> Vec<u8> {
    let num_samples = samples.len() as u32;
    let data_size = num_samples * 2;
    let file_size = 36 + data_size;
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    write_wav_header(&mut wav, file_size, sample_rate, data_size);
    for &s in samples {
        let pcm = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        wav.extend_from_slice(&pcm.to_le_bytes());
    }
    wav
}

fn generate_tone_wav(frequency: f64, duration: f64, sample_rate: u32) -> Vec<u8> {
    let samples = generate_ar7778_tone(frequency, duration, sample_rate);
    samples_to_wav(&samples, sample_rate)
}

fn generate_chord_wav(freqs: &[f64], duration: f64, sample_rate: u32) -> Vec<u8> {
    let num_samples = (sample_rate as f64 * duration) as usize;
    let mut mixed = vec![0.0f64; num_samples];
    let amp_per = 1.0 / freqs.len() as f64;
    for freq in freqs {
        let tone = generate_ar7778_tone(*freq, duration, sample_rate);
        for (i, s) in tone.iter().enumerate() {
            if i < mixed.len() {
                mixed[i] += s * amp_per;
            }
        }
    }
    let max_val = mixed.iter().map(|s| s.abs()).fold(0.0f64, f64::max);
    if max_val > 0.0 {
        for s in &mut mixed {
            *s /= max_val;
        }
    }
    samples_to_wav(&mixed, sample_rate)
}

fn generate_sweep_wav(freqs: &[f64], note_duration: f64, sample_rate: u32) -> Vec<u8> {
    let total_duration = note_duration * freqs.len() as f64;
    let num_samples = (sample_rate as f64 * total_duration) as usize;
    let mut samples = vec![0.0f64; num_samples];
    let note_samples = (sample_rate as f64 * note_duration) as usize;
    for (idx, freq) in freqs.iter().enumerate() {
        let tone = generate_ar7778_tone(*freq, note_duration, sample_rate);
        let offset = idx * note_samples;
        for (i, s) in tone.iter().enumerate() {
            if offset + i < samples.len() {
                samples[offset + i] = *s;
            }
        }
    }
    samples_to_wav(&samples, sample_rate)
}

fn write_wav_header(wav: &mut Vec<u8>, file_size: u32, sample_rate: u32, data_size: u32) {
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2;
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
}

fn wav_to_sound_data(wav_bytes: &[u8]) -> StaticSoundData {
    StaticSoundData::from_cursor(Cursor::new(wav_bytes.to_vec()))
        .expect("Failed to create sound data from generated WAV")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::token::{BinaryOp, CalcError, VocalEvent};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    // --- events_to_tone_indices: Digit variants ---

    #[test]
    fn digit_0_maps_to_c3() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(0)]), vec![TONE_C3]);
    }

    #[test]
    fn digit_1_maps_to_c4() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(1)]), vec![TONE_C4]);
    }

    #[test]
    fn digit_2_maps_to_d4() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(2)]), vec![TONE_D4]);
    }

    #[test]
    fn digit_3_maps_to_e4() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(3)]), vec![TONE_E4]);
    }

    #[test]
    fn digit_4_maps_to_f4() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(4)]), vec![TONE_F4]);
    }

    #[test]
    fn digit_5_maps_to_g4() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(5)]), vec![TONE_G4]);
    }

    #[test]
    fn digit_6_maps_to_a4() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(6)]), vec![TONE_A4]);
    }

    #[test]
    fn digit_7_maps_to_b4() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(7)]), vec![TONE_B4]);
    }

    #[test]
    fn digit_8_maps_to_c5() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(8)]), vec![TONE_C5]);
    }

    #[test]
    fn digit_9_maps_to_d5() {
        assert_eq!(events_to_tone_indices(&[VocalEvent::Digit(9)]), vec![TONE_D5]);
    }

    // --- events_to_tone_indices: Operator variants ---

    #[test]
    fn operator_add_maps_to_e5() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Operator(BinaryOp::Add)]),
            vec![TONE_E5]
        );
    }

    #[test]
    fn operator_subtract_maps_to_g3() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Operator(BinaryOp::Subtract)]),
            vec![TONE_G3]
        );
    }

    #[test]
    fn operator_multiply_maps_to_a3() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Operator(BinaryOp::Multiply)]),
            vec![TONE_A3]
        );
    }

    #[test]
    fn operator_divide_maps_to_e3() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Operator(BinaryOp::Divide)]),
            vec![TONE_E3]
        );
    }

    // --- events_to_tone_indices: other single-event variants ---

    #[test]
    fn decimal_point_maps_to_b3() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::DecimalPoint]),
            vec![TONE_B3]
        );
    }

    #[test]
    fn equals_maps_to_major_chord() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Equals]),
            vec![TONE_CHORD_MAJOR]
        );
    }

    #[test]
    fn percent_maps_to_g4_and_c5() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Percent]),
            vec![TONE_G4, TONE_C5]
        );
    }

    #[test]
    fn mu_maps_to_f4() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::MU]),
            vec![TONE_F4]
        );
    }

    #[test]
    fn square_root_maps_to_ascend() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::SquareRoot]),
            vec![TONE_ASCEND]
        );
    }

    #[test]
    fn backspace_maps_to_click() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Backspace]),
            vec![TONE_CLICK]
        );
    }

    #[test]
    fn clear_maps_to_click() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Clear]),
            vec![TONE_CLICK]
        );
    }

    #[test]
    fn all_clear_maps_to_descend() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::AllClear]),
            vec![TONE_DESCEND]
        );
    }

    #[test]
    fn memory_events_map_to_e4() {
        let events = [
            VocalEvent::MemoryRecall,
            VocalEvent::MemoryAdd,
            VocalEvent::MemorySubtract,
            VocalEvent::MemoryClear,
        ];
        for event in events {
            assert_eq!(
                events_to_tone_indices(&[event.clone()]),
                vec![TONE_E4],
                "{:?} should map to TONE_E4",
                event
            );
        }
    }

    #[test]
    fn sign_negative_maps_to_g3() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::SignNegative]),
            vec![TONE_G3]
        );
    }

    #[test]
    fn sign_positive_maps_to_c4() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::SignPositive]),
            vec![TONE_C4]
        );
    }

    // --- events_to_tone_indices: Error variants ---

    #[test]
    fn error_divide_by_zero_maps_to_error_tone() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Error(CalcError::DivideByZero)]),
            vec![TONE_ERROR]
        );
    }

    #[test]
    fn error_negative_square_root_maps_to_error_tone() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Error(CalcError::NegativeSquareRoot)]),
            vec![TONE_ERROR]
        );
    }

    #[test]
    fn error_overflow_maps_to_error_tone() {
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Error(CalcError::Overflow)]),
            vec![TONE_ERROR]
        );
    }

    // --- events_to_tone_indices: Result variant ---

    #[test]
    fn result_integer_maps_digits() {
        // format_for_speech(123) => "123" => digits 1,2,3 => C4,D4,E4
        let d = Decimal::from(123);
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Result(d)]),
            vec![TONE_C4, TONE_D4, TONE_E4]
        );
    }

    #[test]
    fn result_single_digit() {
        let d = Decimal::from(5);
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Result(d)]),
            vec![TONE_G4]
        );
    }

    #[test]
    fn result_decimal_number() {
        // format_for_speech(3.14) => "3.14" => digits 3,1,4 => E4,C4,F4
        let d = Decimal::from_str("3.14").unwrap();
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Result(d)]),
            vec![TONE_E4, TONE_C4, TONE_F4]
        );
    }

    #[test]
    fn result_zero() {
        let d = Decimal::ZERO;
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Result(d)]),
            vec![TONE_C3]
        );
    }

    #[test]
    fn result_negative_number_skips_minus_sign() {
        // format_for_speech(-42) => "-42" => digits 4,2 => F4,D4 (minus sign skipped)
        let d = Decimal::from(-42);
        assert_eq!(
            events_to_tone_indices(&[VocalEvent::Result(d)]),
            vec![TONE_F4, TONE_D4]
        );
    }

    // --- events_to_tone_indices: empty input ---

    #[test]
    fn empty_events_produce_empty_indices() {
        assert_eq!(events_to_tone_indices(&[]), Vec::<usize>::new());
    }

    // --- events_to_tone_indices: multiple events in sequence ---

    #[test]
    fn multiple_events_produce_sequential_indices() {
        let events = vec![
            VocalEvent::Digit(1),
            VocalEvent::Operator(BinaryOp::Add),
            VocalEvent::Digit(2),
            VocalEvent::Equals,
        ];
        assert_eq!(
            events_to_tone_indices(&events),
            vec![TONE_C4, TONE_E5, TONE_D4, TONE_CHORD_MAJOR]
        );
    }

    // --- MusicTones::new() ---

    #[test]
    fn music_tones_new_succeeds() {
        let tones = MusicTones::new();
        assert_eq!(tones.count(), 23);
    }

    #[test]
    fn music_tones_count_is_23() {
        let tones = MusicTones::new();
        assert_eq!(tones.count(), 23, "expected 23 tones: 17 notes + major chord + dissonant chord + ascend + descend + click + error");
    }

    #[test]
    fn music_tones_get_sound_returns_some_for_valid_indices() {
        let tones = MusicTones::new();
        for i in 0..23 {
            assert!(tones.get_sound(i).is_some(), "get_sound({}) should return Some", i);
        }
    }

    #[test]
    fn music_tones_get_sound_returns_none_out_of_bounds() {
        let tones = MusicTones::new();
        assert!(tones.get_sound(23).is_none());
        assert!(tones.get_sound(100).is_none());
    }

    #[test]
    fn music_tones_get_wav_bytes_returns_some_for_valid_indices() {
        let tones = MusicTones::new();
        for i in 0..23 {
            assert!(tones.get_wav_bytes(i).is_some(), "get_wav_bytes({}) should return Some", i);
        }
    }

    #[test]
    fn music_tones_get_wav_bytes_returns_none_out_of_bounds() {
        let tones = MusicTones::new();
        assert!(tones.get_wav_bytes(23).is_none());
    }

    #[test]
    fn music_tones_wav_bytes_have_valid_wav_header() {
        let tones = MusicTones::new();
        for i in 0..23 {
            let wav = tones.get_wav_bytes(i).unwrap();
            assert!(wav.len() >= 44, "WAV {} should have at least 44-byte header", i);
            assert_eq!(&wav[0..4], b"RIFF", "WAV {} should start with RIFF", i);
            assert_eq!(&wav[8..12], b"WAVE", "WAV {} should contain WAVE fmt", i);
            assert_eq!(&wav[12..16], b"fmt ", "WAV {} should contain fmt chunk", i);
        }
    }

    // --- Internal synthesis function tests ---

    #[test]
    fn generate_ar7778_tone_produces_correct_sample_count() {
        let samples = generate_ar7778_tone(440.0, 0.20, 44100);
        assert_eq!(samples.len(), (44100.0 * 0.20) as usize);
    }

    #[test]
    fn generate_ar7778_tone_is_normalized() {
        let samples = generate_ar7778_tone(440.0, 0.20, 44100);
        let max_val = samples.iter().map(|s| s.abs()).fold(0.0f64, f64::max);
        assert!((max_val - 1.0).abs() < 1e-6, "tone should be normalized to peak 1.0, got {}", max_val);
    }

    #[test]
    fn generate_tone_wav_starts_with_riff() {
        let wav = generate_tone_wav(440.0, 0.20, 44100);
        assert!(wav.len() > 44);
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[test]
    fn generate_chord_wav_starts_with_riff() {
        let wav = generate_chord_wav(&[261.63, 329.63, 392.00], 0.30, 44100);
        assert!(wav.len() > 44);
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[test]
    fn generate_sweep_wav_starts_with_riff() {
        let wav = generate_sweep_wav(&[261.63, 329.63, 392.00], 0.08, 44100);
        assert!(wav.len() > 44);
        assert_eq!(&wav[0..4], b"RIFF");
    }
}
