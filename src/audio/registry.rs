use kira::sound::static_sound::StaticSoundData;
use std::io::Cursor;

/// Pair of playable sound data and the raw WAV bytes used to create it.
pub struct SoundEntry {
    pub data: StaticSoundData,
    pub wav_bytes: Vec<u8>,
}

/// Load all embedded WAV assets.
///
/// Returns `(sounds, wav_bytes_vec)` where index correspondence is 1:1.
/// Index 0-34: normal voice assets from `resource/Vocal/Normal/`
/// Index 35-68: broken voice assets from `resource/Vocal/Broken/`
pub fn load_all_sounds() -> Vec<SoundEntry> {
    let mut entries = Vec::with_capacity(69);

    // Normal voice assets (indices 0-34)
    let normal_bytes: &[&[u8]] = &[
        include_bytes!("../../resource/Vocal/Normal/00_0x00_零.wav"),
        include_bytes!("../../resource/Vocal/Normal/01_0x01_一.wav"),
        include_bytes!("../../resource/Vocal/Normal/02_0x02_二.wav"),
        include_bytes!("../../resource/Vocal/Normal/03_0x03_三.wav"),
        include_bytes!("../../resource/Vocal/Normal/04_0x04_四.wav"),
        include_bytes!("../../resource/Vocal/Normal/05_0x05_五.wav"),
        include_bytes!("../../resource/Vocal/Normal/06_0x06_六.wav"),
        include_bytes!("../../resource/Vocal/Normal/07_0x07_七.wav"),
        include_bytes!("../../resource/Vocal/Normal/08_0x08_八.wav"),
        include_bytes!("../../resource/Vocal/Normal/09_0x09_九.wav"),
        include_bytes!("../../resource/Vocal/Normal/10_0x0A_十.wav"),
        include_bytes!("../../resource/Vocal/Normal/11_0x0B_百.wav"),
        include_bytes!("../../resource/Vocal/Normal/12_0x0C_千.wav"),
        include_bytes!("../../resource/Vocal/Normal/13_0x0D_万.wav"),
        include_bytes!("../../resource/Vocal/Normal/14_0x0E_亿.wav"),
        include_bytes!("../../resource/Vocal/Normal/15_0x0F_除以.wav"),
        include_bytes!("../../resource/Vocal/Normal/16_0x10_乘以.wav"),
        include_bytes!("../../resource/Vocal/Normal/17_0x11_减.wav"),
        include_bytes!("../../resource/Vocal/Normal/18_0x12_加.wav"),
        include_bytes!("../../resource/Vocal/Normal/19_0x13_点.wav"),
        include_bytes!("../../resource/Vocal/Normal/20_0x14_百分比.wav"),
        include_bytes!("../../resource/Vocal/Normal/21_0x15_退位.wav"),
        include_bytes!("../../resource/Vocal/Normal/22_0x16_等于.wav"),
        include_bytes!("../../resource/Vocal/Normal/23_0x17_错误.wav"),
        include_bytes!("../../resource/Vocal/Normal/24_0x18_记忆.wav"),
        include_bytes!("../../resource/Vocal/Normal/25_0x19_负.wav"),
        include_bytes!("../../resource/Vocal/Normal/26_0x1A_正.wav"),
        include_bytes!("../../resource/Vocal/Normal/27_0x1B_归零.wav"),
        include_bytes!("../../resource/Vocal/Normal/28_0x1C_清除.wav"),
        include_bytes!("../../resource/Vocal/Normal/29_0x1D_MU.wav"),
        include_bytes!("../../resource/Vocal/Normal/30_0x1E_总和.wav"),
        include_bytes!("../../resource/Vocal/Normal/31_0x1F_平方根.wav"),
        include_bytes!("../../resource/Vocal/Normal/32_0x20_分.wav"),
        include_bytes!("../../resource/Vocal/Normal/33_0x21_上午.wav"),
        include_bytes!("../../resource/Vocal/Normal/34_0x22_下午.wav"),
    ];

    // Broken voice assets (indices 35-68)
    let broken_bytes: &[&[u8]] = &[
        include_bytes!("../../resource/Vocal/Broken/35_0x4D_四♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/36_0x4F_卟卟卟.wav"),
        include_bytes!("../../resource/Vocal/Broken/37_0x50_五♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/38_0x52_二♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/39_0x54_除♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/40_0x55_三♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/41_0x58_方根.wav"),
        include_bytes!("../../resource/Vocal/Broken/42_0x60_平方根♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/43_0x62_卟卟卟.wav"),
        include_bytes!("../../resource/Vocal/Broken/44_0x65_二♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/45_0x73_四♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/46_0x75_卟卟.wav"),
        include_bytes!("../../resource/Vocal/Broken/47_0x76_五♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/48_0x7D_卟卟卟.wav"),
        include_bytes!("../../resource/Vocal/Broken/49_0x83_呃.wav"),
        include_bytes!("../../resource/Vocal/Broken/50_0x86_四♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/51_0x89_五♂(zi).wav"),
        include_bytes!("../../resource/Vocal/Broken/52_0x8B_二♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/53_0xB1_二♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/54_0xBA_哧.wav"),
        include_bytes!("../../resource/Vocal/Broken/55_0xC2_eng.wav"),
        include_bytes!("../../resource/Vocal/Broken/56_0xC7_四♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/57_0xCD_eng.wav"),
        include_bytes!("../../resource/Vocal/Broken/58_0xDC_噗.wav"),
        include_bytes!("../../resource/Vocal/Broken/59_0xDD_U♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/60_0xDE_啊↑.wav"),
        include_bytes!("../../resource/Vocal/Broken/61_0xE4_U♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/62_0xE7_于.wav"),
        include_bytes!("../../resource/Vocal/Broken/63_0xEB_三♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/64_0xEE_啊→.wav"),
        include_bytes!("../../resource/Vocal/Broken/65_0xEF_卟卟卟.wav"),
        include_bytes!("../../resource/Vocal/Broken/66_0xF5_三♂.wav"),
        include_bytes!("../../resource/Vocal/Broken/67_0xF8_卟零.wav"),
        include_bytes!("../../resource/Vocal/Broken/68_0xFF_卟零♂.wav"),
    ];

    for bytes in normal_bytes.iter().chain(broken_bytes.iter()) {
        let converted = super::convert::float_wav_to_pcm24(bytes);
        let wav_bytes: Vec<u8> = match converted {
            Some(ref v) => v.as_slice(),
            None => *bytes,
        }
        .to_vec();
        match StaticSoundData::from_cursor(Cursor::new(wav_bytes.clone())) {
            Ok(sound) => entries.push(SoundEntry {
                data: sound,
                wav_bytes,
            }),
            Err(e) => {
                log::warn!("Failed to load embedded WAV: {e}");
            }
        }
    }

    log::info!("Loaded {} sound assets", entries.len());
    entries
}
