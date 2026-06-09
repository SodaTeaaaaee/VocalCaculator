use crate::core::speech;
use crate::core::token::VocalEvent;

/// Map events to WAV indices for normal speech mode.
pub fn events_to_wav_indices(events: &[VocalEvent]) -> Vec<u16> {
    let mut result = Vec::new();
    for event in events {
        result.extend(speech::event_to_wav_indices(event));
    }
    result
}
