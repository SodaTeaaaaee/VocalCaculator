/// Convert 32-bit IEEE float WAV to 24-bit PCM WAV with TPDF dithering.
/// Returns None if the input is not a valid IEEE float WAV.
pub fn float_wav_to_pcm24(input: &[u8]) -> Option<Vec<u8>> {
    if input.len() < 44 || &input[0..4] != b"RIFF" || &input[8..12] != b"WAVE" {
        return None;
    }

    let mut audio_format: u16 = 0;
    let mut channels: u16 = 1;
    let mut sample_rate: u32 = 44100;
    let mut data_start: usize = 0;
    let mut data_size: usize = 0;

    let mut pos = 12usize;
    while pos + 8 <= input.len() {
        let chunk_id = &input[pos..pos + 4];
        let cs = u32::from_le_bytes([
            input[pos + 4],
            input[pos + 5],
            input[pos + 6],
            input[pos + 7],
        ]) as usize;
        let cstart = pos + 8;
        if chunk_id == b"fmt " && cstart + 16 <= input.len() {
            audio_format = u16::from_le_bytes([input[cstart], input[cstart + 1]]);
            channels = u16::from_le_bytes([input[cstart + 2], input[cstart + 3]]);
            sample_rate = u32::from_le_bytes([
                input[cstart + 4],
                input[cstart + 5],
                input[cstart + 6],
                input[cstart + 7],
            ]);
        } else if chunk_id == b"data" {
            data_start = cstart;
            data_size = cs;
        }
        pos = pos.saturating_add(8).saturating_add(cs);
        if cs == 0 {
            break;
        }
    }

    if audio_format != 3
        || data_start == 0
        || data_size == 0
        || data_start + data_size > input.len()
    {
        return None;
    }

    let float_data = &input[data_start..data_start + data_size];
    let num_samples = float_data.len() / 4;
    let mut pcm = Vec::with_capacity(num_samples * 3);
    let mut rng: u64 = 0x12345678ABCDEF01;

    for i in 0..num_samples {
        let o = i * 4;
        let f = f32::from_le_bytes([
            float_data[o],
            float_data[o + 1],
            float_data[o + 2],
            float_data[o + 3],
        ]);

        // TPDF dither (triangular probability density function)
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r1 = ((rng >> 33) as f64) / ((1u64 << 31) as f64);
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r2 = ((rng >> 33) as f64) / ((1u64 << 31) as f64);
        let dither = (r1 + r2 - 1.0) * (2.0 / 16777215.0);

        let val = ((f as f64 + dither) * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
        pcm.push(val as u8);
        pcm.push((val >> 8) as u8);
        pcm.push((val >> 16) as u8);
    }

    let nd = pcm.len() as u32;
    let br = sample_rate * channels as u32 * 3;
    let ba = channels * 3;

    let mut out = Vec::with_capacity(44 + nd as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + nd).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&br.to_le_bytes());
    out.extend_from_slice(&ba.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&nd.to_le_bytes());
    out.extend_from_slice(&pcm);

    Some(out)
}

/// WAV format information extracted from a header.
pub(crate) struct WavInfo {
    pub audio_format: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub data_offset: usize,
    pub data_size: usize,
}

/// Parse a WAV header and return format info + data location.
pub(crate) fn parse_wav_info(data: &[u8]) -> Option<WavInfo> {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }

    let mut info = WavInfo {
        audio_format: 0,
        channels: 1,
        sample_rate: 44100,
        bits_per_sample: 16,
        data_offset: 0,
        data_size: 0,
    };

    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let cs = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
        let cstart = pos + 8;
        if chunk_id == b"fmt " && cstart + 16 <= data.len() {
            info.audio_format = u16::from_le_bytes([data[cstart], data[cstart + 1]]);
            info.channels = u16::from_le_bytes([data[cstart + 2], data[cstart + 3]]);
            info.sample_rate = u32::from_le_bytes([
                data[cstart + 4],
                data[cstart + 5],
                data[cstart + 6],
                data[cstart + 7],
            ]);
            info.bits_per_sample = u16::from_le_bytes([data[cstart + 14], data[cstart + 15]]);
        } else if chunk_id == b"data" {
            info.data_offset = cstart;
            info.data_size = cs;
        }
        pos = pos.saturating_add(8).saturating_add(cs);
        if cs == 0 {
            break;
        }
    }

    if info.data_offset == 0 || info.data_size == 0 || info.data_offset + info.data_size > data.len() {
        return None;
    }
    Some(info)
}

/// Concatenate multiple WAV buffers into a single WAV file.
///
/// All input buffers must share the same sample format (channels, sample rate, bit depth).
/// Returns None if inputs are empty or have mismatched formats.
pub fn concat_wav_buffers(buffers: &[&[u8]]) -> Option<Vec<u8>> {
    if buffers.is_empty() {
        return None;
    }

    let infos: Vec<WavInfo> = buffers.iter()
        .map(|b| parse_wav_info(b))
        .collect::<Option<Vec<_>>>()?;

    let first = &infos[0];
    for info in &infos[1..] {
        if info.channels != first.channels
            || info.sample_rate != first.sample_rate
            || info.bits_per_sample != first.bits_per_sample
        {
            log::warn!("WAV format mismatch during concatenation");
            return None;
        }
    }

    let total_data_size: usize = infos.iter().map(|i| i.data_size).sum();
    let bytes_per_sample = first.bits_per_sample / 8;
    let block_align = first.channels * bytes_per_sample;
    let byte_rate = first.sample_rate * block_align as u32;
    let file_size = 36 + total_data_size as u32;

    let mut out = Vec::with_capacity(44 + total_data_size);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&first.audio_format.to_le_bytes());
    out.extend_from_slice(&first.channels.to_le_bytes());
    out.extend_from_slice(&first.sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&first.bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(total_data_size as u32).to_le_bytes());

    for (i, info) in infos.iter().enumerate() {
        let end = info.data_offset + info.data_size;
        if end <= buffers[i].len() {
            out.extend_from_slice(&buffers[i][info.data_offset..end]);
        }
    }

    Some(out)
}

