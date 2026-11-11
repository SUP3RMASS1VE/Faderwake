use crate::Audio;
use std::io::{BufWriter, Write};
use std::path::Path;
use tempfile::NamedTempFile;

struct Format {
    channels: u16,
    rate: u32,
    bytes: usize,
    float: bool,
}

fn word(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().unwrap())
}
fn dword(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

fn format(data: &[u8]) -> Result<Format, String> {
    if data.len() < 16 {
        return Err("Incomplete WAV format chunk.".into());
    }
    let channels = word(data, 2);
    let rate = dword(data, 4);
    let bits = word(data, 14);
    let mut encoding = word(data, 0);
    if encoding == 0xfffe {
        // KSDATAFORMAT_SUBTYPE_PCM / IEEE_FLOAT, with valid bits left-aligned.
        const GUID_SUFFIX: [u8; 14] = [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71];
        if data.len() < 40 || word(data, 16) < 22 || data[26..40] != GUID_SUFFIX {
            return Err("Unsupported extensible WAV format.".into());
        }
        encoding = word(data, 24);
    }
    let supported = match encoding {
        1 => matches!(bits, 8 | 16 | 24 | 32),
        3 => matches!(bits, 32 | 64),
        _ => false,
    };
    if !supported {
        return Err("Open an uncompressed PCM or floating-point WAV file.".into());
    }
    if !(1..=2).contains(&channels) || !(8_000..=384_000).contains(&rate) {
        return Err("Open a mono or stereo WAV file at 8–384 kHz.".into());
    }
    let bytes = (bits / 8) as usize;
    if word(data, 12) as usize != bytes * channels as usize {
        return Err("Invalid WAV frame alignment.".into());
    }
    Ok(Format {
        channels,
        rate,
        bytes,
        float: encoding == 3,
    })
}

pub fn decode_wav(bytes: &[u8]) -> Result<Audio, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("This file is not a little-endian RIFF WAV.".into());
    }
    let end = (dword(bytes, 4) as usize)
        .checked_add(8)
        .ok_or("Invalid WAV size.")?;
    if end > bytes.len() || end < 12 {
        return Err("The WAV header reports an incomplete file.".into());
    }
    let mut position = 12;
    let mut fmt = None;
    let mut data = None;
    while end - position >= 8 {
        let size = dword(bytes, position + 4) as usize;
        let start = position + 8;
        let stop = start
            .checked_add(size)
            .filter(|n| *n <= end)
            .ok_or("Invalid WAV chunk length.")?;
        match &bytes[position..position + 4] {
            b"fmt " => fmt = Some(format(&bytes[start..stop])?),
            b"data" if data.is_none() => data = Some(&bytes[start..stop]),
            _ => {}
        }
        // Some encoders omit the optional final padding byte.
        position = (stop + size % 2).min(end);
    }
    let fmt = fmt.ok_or("Missing WAV format.")?;
    let data = data.ok_or("Missing WAV audio data.")?;
    let frame = fmt.bytes * fmt.channels as usize;
    if data.len() % frame != 0 || data.is_empty() {
        return Err("The WAV file has empty or partial frames.".into());
    }
    if data.len() / frame > 100_000_000 {
        return Err("This track exceeds the 100 million frame limit.".into());
    }
    let mut samples = Vec::with_capacity(data.len() / fmt.bytes);
    for b in data.chunks_exact(fmt.bytes) {
        let value = match (fmt.float, fmt.bytes) {
            (false, 1) => (b[0] as f32 - 128.) / 128.,
            (false, 2) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.,
            (false, 3) => i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32 / 2147483648.,
            (false, 4) => i32::from_le_bytes(b.try_into().unwrap()) as f32 / 2147483648.,
            (true, 4) => f32::from_le_bytes(b.try_into().unwrap()),
            (true, 8) => f64::from_le_bytes(b.try_into().unwrap()) as f32,
            _ => unreachable!(),
        };
        if !value.is_finite() {
            return Err("The WAV contains non-finite samples.".into());
        }
        samples.push(value);
    }
    Ok(Audio {
        sample_rate: fmt.rate,
        channels: fmt.channels,
        samples,
    })
}

pub fn write_wav(path: &Path, audio: &Audio) -> Result<(), String> {
    audio.validate()?;
    let data_bytes = audio
        .samples
        .len()
        .checked_mul(3)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("Audio exceeds the WAV size limit.")?;
    let riff_bytes = data_bytes
        .checked_add(36 + data_bytes % 2)
        .ok_or("Audio exceeds the WAV size limit.")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut w = BufWriter::new(file.as_file_mut());
        let result = (|| -> std::io::Result<()> {
            w.write_all(b"RIFF")?;
            w.write_all(&riff_bytes.to_le_bytes())?;
            w.write_all(b"WAVEfmt ")?;
            w.write_all(&16u32.to_le_bytes())?;
            w.write_all(&1u16.to_le_bytes())?;
            w.write_all(&audio.channels.to_le_bytes())?;
            w.write_all(&audio.sample_rate.to_le_bytes())?;
            let alignment = audio.channels * 3;
            w.write_all(&(audio.sample_rate * alignment as u32).to_le_bytes())?;
            w.write_all(&alignment.to_le_bytes())?;
            w.write_all(&24u16.to_le_bytes())?;
            w.write_all(b"data")?;
            w.write_all(&data_bytes.to_le_bytes())?;
            for &sample in &audio.samples {
                let quantized = (sample.clamp(-1., 1.) * 8388608.).round() as i32;
                w.write_all(&quantized.clamp(-8388608, 8388607).to_le_bytes()[..3])?;
            }
            if data_bytes % 2 != 0 {
                w.write_all(&[0])?;
            }
            w.flush()
        })();
        result.map_err(|e| e.to_string())?;
    }
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.error.to_string())?;
    Ok(())
}
