use crate::core::speech;
use crate::core::token::VocalEvent;

// Broken WAV file indices (35-68 in the combined registry)
const BROKEN_DIGIT_TWO: &[u16] = &[38, 44, 52, 53];
const BROKEN_DIGIT_THREE: &[u16] = &[40, 63, 66];
const BROKEN_DIGIT_FOUR: &[u16] = &[35, 45, 50, 56];
const BROKEN_DIGIT_FIVE: &[u16] = &[37, 47, 51];
const BROKEN_DIVIDE: &[u16] = &[39];
const BROKEN_SQRT: &[u16] = &[41, 42];

// Noise pool: all other broken files
const NOISE_POOL: &[u16] = &[
    36, 43, 46, 48, 49, 54, 55, 57, 58, 59, 60, 61, 62, 64, 65, 67, 68,
];

/// Maximum corruption probability (50%).
const MAX_CORRUPTION_PROB: u32 = u32::MAX / 2;

fn broken_variants_for_normal(normal_idx: u16) -> Option<&'static [u16]> {
    match normal_idx {
        2 => Some(BROKEN_DIGIT_TWO),
        3 => Some(BROKEN_DIGIT_THREE),
        4 => Some(BROKEN_DIGIT_FOUR),
        5 => Some(BROKEN_DIGIT_FIVE),
        15 => Some(BROKEN_DIVIDE),
        31 => Some(BROKEN_SQRT),
        _ => None,
    }
}

/// Xorshift64 PRNG - fast, no external dependency.
fn xorshift64(state: &mut u64) -> u32 {
    let mut x = *state;
    if x == 0 { x = 0x12345678ABCDEF01; }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    (x >> 32) as u32
}

/// Map events to WAV indices for broken speech mode.
///
/// Uses a PRNG to decide whether to corrupt each sound. Corruption
/// probability is capped at 50%. Returns just the wav_indices (no
/// alt flag needed since randomness replaces round-robin).
pub fn events_to_wav_indices(events: &[VocalEvent], rng_state: &mut u64) -> Vec<u16> {
    let mut result = Vec::new();

    for event in events {
        let normal_indices = speech::event_to_wav_indices(event);
        for &normal_idx in &normal_indices {
            let roll = xorshift64(rng_state);
            let should_corrupt = roll < MAX_CORRUPTION_PROB;

            if should_corrupt {
                if let Some(pool) = broken_variants_for_normal(normal_idx) {
                    let pick_idx = (roll as usize) % pool.len();
                    result.push(pool[pick_idx]);
                } else if normal_idx < 35 {
                    let pick_idx = (roll as usize) % NOISE_POOL.len();
                    result.push(NOISE_POOL[pick_idx]);
                } else {
                    result.push(normal_idx);
                }
            } else {
                result.push(normal_idx);
            }
        }
    }

    result
}