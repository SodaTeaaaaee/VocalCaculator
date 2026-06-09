pub mod broken;
pub mod music;
pub mod normal;
pub mod registry;

use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, DefaultBackend, Decibels, Tween};
use music::MusicTones;

use crate::core::token::VocalEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    Normal,
    Broken,
    Music,
    Silent,
}

impl AudioMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Broken => "Broken",
            Self::Music => "Music",
            Self::Silent => "Silent",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Broken,
            Self::Broken => Self::Music,
            Self::Music => Self::Silent,
            Self::Silent => Self::Normal,
        }
    }
}

fn slider_to_decibels(slider: f64) -> Decibels {
    let slider = slider.clamp(0.0, 1.0);
    if slider <= 0.0 {
        Decibels::SILENCE
    } else {
        Decibels((slider - 1.0) as f32 * 60.0)
    }
}

pub struct VocalAudio {
    main_track: TrackHandle,
    sounds: Vec<StaticSoundData>,
    wav_bytes: Vec<Vec<u8>>,
    music_tones: MusicTones,
    mode: AudioMode,
    current_handle: Option<StaticSoundHandle>,
    rng_state: u64,
}

impl VocalAudio {
    pub fn new() -> Option<Self> {
        let mut manager = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("AudioManager init failed: {e}");
                return None;
            }
        };
        let mut main_track = match manager.add_sub_track(TrackBuilder::default()) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("Failed to create audio sub-track: {e}");
                return None;
            }
        };
        main_track.set_volume(slider_to_decibels(0.8), Tween::default());
        let entries = registry::load_all_sounds();
        let n = entries.len();
        let mut sounds = Vec::with_capacity(n);
        let mut wav_bytes = Vec::with_capacity(n);
        for entry in entries {
            sounds.push(entry.data);
            wav_bytes.push(entry.wav_bytes);
        }
        let music_tones = MusicTones::new();
        log::info!(
            "Audio: loaded {} voice sounds, {} music tones",
            n,
            music_tones.count()
        );
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Some(Self {
            main_track,
            sounds,
            wav_bytes,
            music_tones,
            mode: AudioMode::Normal,
            current_handle: None,
            rng_state: seed | 1,
        })
    }

    pub fn sound_count(&self) -> usize {
        self.sounds.len()
    }

    pub fn mode(&self) -> AudioMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: AudioMode) {
        self.mode = mode;
    }

    pub fn cycle_mode(&mut self) {
        self.set_mode(self.mode.next());
    }

    pub fn set_volume(&mut self, slider: f64) {
        self.main_track
            .set_volume(slider_to_decibels(slider), Tween::default());
    }

    pub fn play_events(&mut self, events: &[VocalEvent]) {
        if self.mode == AudioMode::Silent {
            return;
        }
        if let Some(ref mut handle) = self.current_handle {
            handle.stop(Tween::default());
        }
        self.current_handle = None;
        match self.mode {
            AudioMode::Normal => {
                let indices = normal::events_to_wav_indices(events);
                self.play_voice_sequence(&indices);
            }
            AudioMode::Broken => {
                let indices = broken::events_to_wav_indices(events, &mut self.rng_state);
                self.play_voice_sequence(&indices);
            }
            AudioMode::Music => {
                let tone_indices = music::events_to_tone_indices(events);
                self.play_tone_sequence(&tone_indices);
            }
            AudioMode::Silent => {}
        }
    }

    fn play_voice_sequence(&mut self, wav_indices: &[u16]) {
        if wav_indices.is_empty() {
            return;
        }
        if wav_indices.len() == 1 {
            self.play_sound(wav_indices[0]);
            return;
        }
        let buffers: Vec<&[u8]> = wav_indices
            .iter()
            .filter_map(|&idx| self.wav_bytes.get(idx as usize).map(|v| v.as_slice()))
            .collect();
        match convert::concat_wav_buffers(&buffers) {
            Some(combined) => {
                match StaticSoundData::from_cursor(std::io::Cursor::new(combined)) {
                    Ok(data) => match self.main_track.play(data) {
                        Ok(handle) => {
                            self.current_handle = Some(handle);
                        }
                        Err(e) => log::warn!("Audio concat play error: {e}"),
                    },
                    Err(e) => log::warn!("Concat sound data error: {e}"),
                }
            }
            None => {
                if let Some(&idx) = wav_indices.first() {
                    self.play_sound(idx);
                }
            }
        }
    }

    fn play_tone_sequence(&mut self, tone_indices: &[usize]) {
        if tone_indices.is_empty() {
            return;
        }
        if tone_indices.len() == 1 {
            if let Some(data) = self.music_tones.get_sound(tone_indices[0]) {
                match self.main_track.play(data.clone()) {
                    Ok(handle) => {
                        self.current_handle = Some(handle);
                    }
                    Err(e) => log::warn!("Music play error: {e}"),
                }
            }
            return;
        }
        let buffers: Vec<&[u8]> = tone_indices
            .iter()
            .filter_map(|&idx| self.music_tones.get_wav_bytes(idx))
            .collect();
        match convert::concat_wav_buffers(&buffers) {
            Some(combined) => {
                match StaticSoundData::from_cursor(std::io::Cursor::new(combined)) {
                    Ok(data) => match self.main_track.play(data) {
                        Ok(handle) => {
                            self.current_handle = Some(handle);
                        }
                        Err(e) => log::warn!("Music concat play error: {e}"),
                    },
                    Err(e) => log::warn!("Music concat data error: {e}"),
                }
            }
            None => {
                if let Some(&idx) = tone_indices.first() {
                    if let Some(data) = self.music_tones.get_sound(idx) {
                        if let Ok(handle) = self.main_track.play(data.clone()) {
                            self.current_handle = Some(handle);
                        }
                    }
                }
            }
        }
    }

    fn play_sound(&mut self, index: u16) {
        let idx = index as usize;
        if idx >= self.sounds.len() {
            return;
        }
        match self.main_track.play(self.sounds[idx].clone()) {
            Ok(handle) => {
                self.current_handle = Some(handle);
            }
            Err(e) => {
                log::warn!("Audio play error: {e}");
            }
        }
    }
}

pub mod convert;