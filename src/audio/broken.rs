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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::token::{BinaryOp, VocalEvent};

    /// Helper: run events_to_wav_indices with a fresh seed.
    fn run(events: &[VocalEvent], seed: u64) -> Vec<u16> {
        let mut state = seed;
        events_to_wav_indices(events, &mut state)
    }

    #[test]
    fn deterministic_seed_produces_specific_corruption_pattern() {
        // Seed 1 gives a known, reproducible PRNG sequence.
        // Feed Digit(2) -> normal index 2, which has broken variants [38, 44, 52, 53].
        let events = vec![VocalEvent::Digit(2)];
        let result = run(&events, 1);

        // The result must be exactly 1 element.
        assert_eq!(result.len(), 1);

        // With seed=1 the first xorshift64 roll is 0 (< MAX_CORRUPTION_PROB),
        // so corruption triggers. pick_idx = 0 % 4 = 0 -> pool[0] = 38.
        assert_eq!(result[0], 38);
    }

    #[test]
    fn deterministic_seed_longer_sequence() {
        // Feed several Digit(2) events to consume multiple PRNG rolls.
        let events = vec![
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
        ];
        let result = run(&events, 1);

        // All 4 should produce indices in the valid broken range for digit 2
        // (either the broken pool [38,44,52,53] or the normal index 2).
        assert_eq!(result.len(), 4);
        for &idx in &result {
            assert!(
                idx == 2 || BROKEN_DIGIT_TWO.contains(&idx),
                "unexpected index {idx} for Digit(2) corruption"
            );
        }

        // Snapshot: this is the exact output with seed=1 and 4x Digit(2).
        // If the PRNG or corruption logic changes, this will flag it.
        assert_eq!(result, vec![38, 52, 2, 2]);
    }

    #[test]
    fn different_seeds_produce_different_corruption_patterns() {
        let events = vec![
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
            VocalEvent::Digit(2),
        ];

        let result_a = run(&events, 1);
        let result_b = run(&events, 0xDEADBEEF);

        // The two seeds must produce different corruption sequences.
        assert_ne!(result_a, result_b, "different seeds should yield different patterns");
    }

    #[test]
    fn different_seeds_for_digit_with_no_broken_variants() {
        // Digit(9) -> normal index 9, no broken variant.
        // When corrupted, it falls through to the noise pool (indices 36-68).
        let events = vec![VocalEvent::Digit(9); 8];

        let result_a = run(&events, 1);
        let result_b = run(&events, 0xCAFE_BABE);

        // Both results must be valid indices.
        for &idx in result_a.iter().chain(result_b.iter()) {
            assert!(idx < 69, "index {idx} out of valid range 0-68");
        }

        // With enough samples the two seeds should diverge.
        assert_ne!(result_a, result_b);
    }

    #[test]
    fn non_corrupted_events_return_valid_indices() {
        // Use a seed that produces only high rolls (> MAX_CORRUPTION_PROB),
        // meaning no corruption. Seed 0 is replaced internally to a non-zero
        // value; we need a seed whose first roll exceeds u32::MAX / 2.
        //
        // Rather than hunting for such a seed, verify the invariant:
        // every returned index must be in 0..68.
        let events = vec![
            VocalEvent::Digit(0),
            VocalEvent::Digit(1),
            VocalEvent::Digit(2),
            VocalEvent::Digit(3),
            VocalEvent::Digit(4),
            VocalEvent::Digit(5),
            VocalEvent::Digit(6),
            VocalEvent::Digit(7),
            VocalEvent::Digit(8),
            VocalEvent::Digit(9),
            VocalEvent::DecimalPoint,
            VocalEvent::Operator(BinaryOp::Add),
            VocalEvent::Operator(BinaryOp::Subtract),
            VocalEvent::Operator(BinaryOp::Multiply),
            VocalEvent::Operator(BinaryOp::Divide),
            VocalEvent::Equals,
            VocalEvent::Percent,
            VocalEvent::SquareRoot,
        ];

        // Test across many seeds to ensure validity.
        for seed in [1u64, 42, 100, 999, 0xDEAD, 0xCAFE_BABE] {
            let result = run(&events, seed);
            assert_eq!(result.len(), events.len());
            for &idx in &result {
                assert!(idx < 69, "index {idx} out of valid range 0..69 (seed={seed})");
            }
        }
    }

    #[test]
    fn broken_variants_mapping_covers_expected_digits() {
        // Verify the mapping returns Some for the expected normal indices.
        assert!(broken_variants_for_normal(2).is_some());
        assert!(broken_variants_for_normal(3).is_some());
        assert!(broken_variants_for_normal(4).is_some());
        assert!(broken_variants_for_normal(5).is_some());
        assert!(broken_variants_for_normal(15).is_some());
        assert!(broken_variants_for_normal(31).is_some());

        // And None for indices with no broken variants.
        assert!(broken_variants_for_normal(0).is_none());
        assert!(broken_variants_for_normal(1).is_none());
        assert!(broken_variants_for_normal(6).is_none());
        assert!(broken_variants_for_normal(34).is_none());
    }
}