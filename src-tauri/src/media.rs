//! Audio codecs run in bundled, local-only FFmpeg processes. The Rust engine
//! receives interleaved floating-point samples at their original sample rate.
use master_core::Audio;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Output as ProcessOutput, Stdio};
use std::thread;
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub year: String,
    pub track: String,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Output {
    pub format: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            format: "wav".into(),
            sample_rate: 44100,
            bit_depth: 24,
        }
    }
}

impl Output {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.format.as_str(), "wav" | "flac" | "alac" | "aac") {
            return Err("Choose WAV, FLAC, Apple Lossless or AAC.".into());
        }
        if ![44100, 48000, 88200, 96000, 176400, 192000].contains(&self.sample_rate)
            || self.format == "aac" && self.sample_rate > 48000
            || ![16, 24].contains(&self.bit_depth)
        {
            return Err("Unsupported sample rate or bit depth for this format.".into());
        }
        Ok(())
    }

    pub fn extension(&self) -> &'static str {
        match self.format.as_str() {
            "alac" | "aac" => "m4a",
            "flac" => "flac",
            _ => "wav",
        }
    }
}

fn tool(name: &str) -> Result<Command, String> {
    let name = format!("faderwake-{name}");
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = exe
        .parent()
        .ok_or("Cannot locate the application directory.")?;
    let installed = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    // Test executables live in a deps directory under the Cargo output folder.
    let tests = directory
        .parent()
        .map(|p| p.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)));
    let local = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!(
            "{name}-{}{}",
            env!("TAURI_ENV_TARGET_TRIPLE"),
            std::env::consts::EXE_SUFFIX
        ));
    let path = [
        Some(installed),
        tests.filter(|_| directory.ends_with("deps")),
        (cfg!(test) || cfg!(debug_assertions)).then_some(local),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_file())
    .ok_or_else(|| {
        format!("The bundled {name} audio tool is missing. Rebuild the app with its codecs.")
    })?;
    let mut command = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .args(["-hide_banner", "-loglevel", "error"])
        .stdin(Stdio::null());
    Ok(command)
}

fn check_output(output: ProcessOutput) -> Result<Vec<u8>, String> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(codec_error(&output.stderr))
    }
}

fn codec_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let message = text.trim();
    if message.is_empty() {
        "The audio codec could not process this file.".into()
    } else {
        message.chars().take(1200).collect()
    }
}

#[derive(Deserialize)]
struct StreamInfo {
    sample_rate: String,
    channels: u16,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}
#[derive(Deserialize, Default)]
struct FormatInfo {
    #[serde(default)]
    tags: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Probe {
    streams: Vec<StreamInfo>,
    #[serde(default)]
    format: FormatInfo,
}

fn probe(path: &Path) -> Result<(u32, u16, Tags), String> {
    let output = tool("ffprobe")?
        .args([
            "-protocol_whitelist",
            "file",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=sample_rate,channels:format_tags:stream_tags",
            "-of",
            "json",
            "-i",
        ])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    let info: Probe = serde_json::from_slice(&check_output(output)?).map_err(|e| e.to_string())?;
    let stream = info
        .streams
        .into_iter()
        .next()
        .ok_or("No audio stream found.")?;
    let rate: u32 = stream
        .sample_rate
        .parse()
        .map_err(|_| "Invalid source sample rate.")?;
    if !(1..=2).contains(&stream.channels) || !(8000..=384000).contains(&rate) {
        return Err("Choose a mono or stereo audio file at 8–384 kHz.".into());
    }
    let mut tags = Tags::default();
    for (key, value) in stream.tags.into_iter().chain(info.format.tags) {
        match key.to_ascii_lowercase().as_str() {
            "title" => tags.title = value,
            "artist" => tags.artist = value,
            "album" => tags.album = value,
            "genre" => tags.genre = value,
            "year" | "date" => tags.year = value,
            "track" | "tracknumber" | "itrk" => tags.track = value,
            "comment" | "description" => tags.comment = value,
            _ => {}
        }
    }
    Ok((rate, stream.channels, tags))
}

fn read_errors(child: &mut Child) -> thread::JoinHandle<Vec<u8>> {
    let mut pipe = child.stderr.take().expect("stderr pipe");
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&buf[..n.min(4096 - kept.len())]);
        }
        kept
    })
}

pub fn load(path: &Path) -> Result<(Audio, Tags), String> {
    load_cancellable(path, || false)
}

pub fn load_cancellable(
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<(Audio, Tags), String> {
    let (sample_rate, channels, tags) = probe(path)?;
    let mut cmd = tool("ffmpeg")?;
    let mut child = cmd
        .args(["-nostdin", "-protocol_whitelist", "file", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-c:a",
            "pcm_f32le",
            "-f",
            "f32le",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let errors = read_errors(&mut child);
    let mut reader = child.stdout.take().expect("PCM pipe");
    let audio = (|| -> Result<Audio, String> {
        let mut samples = Vec::new();
        let mut buf = [0u8; 65536];
        let mut rest = 0;
        loop {
            if cancelled() {
                return Err("Import cancelled.".into());
            }
            let n = reader.read(&mut buf[rest..]).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let size = rest + n;
            let (words, tail) = buf[..size].as_chunks::<4>();
            if samples.len() + words.len() > 100_000_000 * usize::from(channels) {
                return Err("This file exceeds the 100 million frame editing limit.".into());
            }
            for word in words {
                let value = f32::from_le_bytes(*word);
                if !value.is_finite() {
                    return Err("The track contains invalid samples.".into());
                }
                samples.push(value);
            }
            rest = tail.len();
            buf.copy_within(size - rest..size, 0);
        }
        if rest != 0 || samples.is_empty() || !samples.len().is_multiple_of(usize::from(channels)) {
            return Err("The audio stream is empty or incomplete.".into());
        }
        Ok(Audio {
            sample_rate,
            channels,
            samples,
        })
    })();
    drop(reader);
    if audio.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = errors.join().unwrap_or_default();
    if !status.success() && !stderr.is_empty() && !cancelled() {
        return Err(codec_error(&stderr));
    }
    let audio = audio?;
    if !status.success() {
        return Err(codec_error(&stderr));
    }
    Ok((audio, tags))
}

fn encoder_options(output: &Output, channels: u16) -> Vec<OsString> {
    let sixteen = output.bit_depth == 16;
    let (codec, container) = match output.format.as_str() {
        "flac" => ("flac", "flac"),
        "alac" => ("alac", "ipod"),
        "aac" => ("aac", "ipod"),
        _ if sixteen => ("pcm_s16le", "wav"),
        _ => ("pcm_s24le", "wav"),
    };
    let mut args = vec!["-c:a", codec, "-f", container];
    match codec {
        "aac" => args.extend(["-b:a", if channels == 1 { "128k" } else { "256k" }]),
        "alac" => args.extend(["-sample_fmt", if sixteen { "s16p" } else { "s32p" }]),
        "flac" => args.extend(["-sample_fmt", if sixteen { "s16" } else { "s32" }]),
        _ => {}
    }
    args.into_iter().map(OsString::from).collect()
}

#[cfg(test)]
pub fn encode(
    path: &Path,
    audio: &Audio,
    tags: &Tags,
    output: &Output,
    overwrite: bool,
) -> Result<(), String> {
    encode_cancellable(path, audio, tags, output, overwrite, || false)
}

pub fn encode_cancellable(
    path: &Path,
    audio: &Audio,
    tags: &Tags,
    output: &Output,
    overwrite: bool,
    cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    output.validate()?;
    audio.validate()?;
    if cancelled() {
        return Err("Export cancelled.".into());
    }
    let converted;
    let audio = if audio.sample_rate == output.sample_rate {
        audio
    } else {
        converted = master_core::resample_cancellable(audio, output.sample_rate, &cancelled)?;
        &converted
    };
    let parent = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = NamedTempFile::new_in(parent)
        .map_err(|e| e.to_string())?
        .into_temp_path();
    let mut cmd = tool("ffmpeg")?;
    cmd.args([
        "-nostdin",
        "-y",
        "-f",
        "f32le",
        "-ar",
        &audio.sample_rate.to_string(),
        "-ac",
        &audio.channels.to_string(),
        "-i",
        "pipe:0",
        "-map_metadata",
        "-1",
        "-ar",
        &output.sample_rate.to_string(),
    ]);
    for (name, value) in [
        ("title", &tags.title),
        ("artist", &tags.artist),
        ("album", &tags.album),
        ("genre", &tags.genre),
        ("date", &tags.year),
        ("track", &tags.track),
        ("comment", &tags.comment),
    ] {
        if !value.is_empty() {
            cmd.arg("-metadata").arg(format!("{name}={value}"));
        }
    }
    cmd.args(encoder_options(output, audio.channels))
        .arg(&*temp);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let errors = read_errors(&mut child);
    let mut writer = child.stdin.take().expect("encoder input pipe");
    let result = (|| -> Result<(), String> {
        let mut bytes = Vec::with_capacity(65536);
        let mut noise = fastrand::Rng::new();
        for chunk in audio.samples.chunks(16384) {
            if cancelled() {
                return Err("Export cancelled.".into());
            }
            bytes.clear();
            if output.format == "wav" {
                let scale = f64::from(1u32 << (output.bit_depth - 1));
                for &sample in chunk {
                    let dither = if output.bit_depth == 16 {
                        noise.f64() - noise.f64()
                    } else {
                        0.0
                    };
                    let value = (f64::from(sample) * scale + dither)
                        .round()
                        .clamp(-scale, scale - 1.0);
                    bytes.extend_from_slice(&((value / scale) as f32).to_le_bytes());
                }
            } else {
                bytes.extend(chunk.iter().flat_map(|sample| sample.to_le_bytes()));
            }
            writer.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    drop(writer);
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = errors.join().unwrap_or_default();
    if cancelled() {
        return Err("Export cancelled.".into());
    }
    if !status.success() {
        return Err(codec_error(&stderr));
    }
    result?;
    if overwrite {
        temp.persist(path)
    } else {
        temp.persist_noclobber(path)
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(rate: u32, frames: usize) -> Audio {
        Audio {
            sample_rate: rate,
            channels: 2,
            samples: (0..frames * 2)
                .map(|i| {
                    let phase = (i / 2) as f32 * 997. * std::f32::consts::TAU / rate as f32;
                    phase.sin() * if i % 2 == 0 { 0.55 } else { 0.32 }
                })
                .collect(),
        }
    }

    fn recording_tags() -> Tags {
        Tags {
            title: "Half Light — test".into(),
            artist: "Studio".into(),
            album: "After Hours".into(),
            genre: "Electronic".into(),
            year: "2026".into(),
            track: "3".into(),
            comment: "Codec round trip".into(),
        }
    }

    #[test]
    fn all_formats_keep_rate_depth_and_tags() {
        let dir = tempfile::tempdir().unwrap();
        let tags = recording_tags();
        for format in ["wav", "flac", "alac", "aac"] {
            for rate in [44100, 48000, 88200, 96000, 176400, 192000] {
                for bits in [16, 24] {
                    let output = Output {
                        format: format.into(),
                        sample_rate: rate,
                        bit_depth: bits,
                    };
                    if output.validate().is_err() {
                        assert!(format == "aac" && rate > 48000);
                        continue;
                    }
                    if format == "aac" && bits == 16 {
                        continue;
                    }
                    let path = dir.path().join(format!(
                        "master-{}-{}-{}.{}",
                        format,
                        rate,
                        bits,
                        output.extension()
                    ));
                    let source = example(
                        rate,
                        if format == "aac" {
                            rate as usize / 4
                        } else {
                            1579
                        },
                    );
                    encode(&path, &source, &tags, &output, false).unwrap();
                    let (decoded, actual_tags) = load(&path).unwrap();
                    assert_eq!(decoded.sample_rate, rate, "{format} at {rate}");
                    assert_eq!(tags, actual_tags, "{format} metadata at {rate}");
                    assert_eq!(decoded.channels, 2);
                    if format == "aac" {
                        assert!(decoded.frames().abs_diff(source.frames()) <= 1024);
                        continue;
                    }
                    let prop = tool("ffprobe")
                        .unwrap()
                        .args([
                            "-select_streams",
                            "a:0",
                            "-show_entries",
                            "stream=bits_per_raw_sample,bits_per_sample",
                            "-of",
                            "json",
                            "-i",
                        ])
                        .arg(&path)
                        .output()
                        .unwrap();
                    let val: serde_json::Value =
                        serde_json::from_slice(&check_output(prop).unwrap()).unwrap();
                    let st = &val["streams"][0];
                    assert!(
                        st["bits_per_sample"].as_u64() == Some(u64::from(bits))
                            || st["bits_per_raw_sample"].as_str() == Some(&bits.to_string()),
                        "{format} {bits}: {st}"
                    );
                    assert_eq!(decoded.frames(), source.frames(), "{format} at {rate}");
                    // Triangular dither plus rounding stays within 1.5 output LSBs.
                    let tolerance = if bits == 16 { 1.5 / 32768.0 } else { 1.0e-6 };
                    assert!(
                        decoded
                            .samples
                            .iter()
                            .zip(source.samples.iter())
                            .all(|(a, b)| (a - b).abs() <= tolerance),
                        "{format} {bits} samples"
                    );
                }
            }
        }
    }

    #[test]
    fn sixteen_bit_wav_dithers_quiet_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quiet.wav");
        let mut source = example(44100, 4096);
        source.samples.fill(0.2 / 32768.0);
        let output = Output {
            bit_depth: 16,
            ..Output::default()
        };
        encode(&path, &source, &Tags::default(), &output, true).unwrap();
        let samples = load(&path).unwrap().0.samples;
        assert!(samples.contains(&(1.0 / 32768.0)));
        assert!(samples.contains(&(-1.0 / 32768.0)));
        assert!(samples.contains(&0.0));
    }

    #[test]
    fn short_flac_and_collision_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one.flac");
        let tags = recording_tags();
        let output = Output {
            format: "flac".into(),
            sample_rate: 48000,
            bit_depth: 24,
        };
        for n in [1, 32, 480, 4096, 4607, 4608, 4800] {
            let src = example(48000, n);
            encode(&path, &src, &tags, &output, true).unwrap();
            assert_eq!(load(&path).unwrap().0.frames(), n);
        }
        let before = std::fs::read(&path).unwrap();
        assert!(encode(&path, &example(48000, 8), &tags, &output, false).is_err());
        assert!(
            encode_cancellable(&path, &example(48000, 8), &tags, &output, true, || true).is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
