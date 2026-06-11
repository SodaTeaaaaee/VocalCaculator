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

/// Maximum corruption probability for broken mode (50%).
pub const BROKEN_MAX_PROBABILITY: f64 = 0.5;

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

        for i in 0..num_samples {
            let t = i as f64 / sample_rate as f64;
            let signal = (2.0 * PI * harm_freq * t).sin();
            let envelope = (2.0 * PI * env_freq * t).cos();
            samples[i] += signal * envelope * attenuation;
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
