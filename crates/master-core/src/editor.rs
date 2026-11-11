use crate::{db_gain, resample, Audio};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edit {
    FadeIn,
    FadeOut,
    Trim,
    Cut,
    Silence,
    Normalize,
    Reverse,
    Gain,
    Paste,
}

fn valid_range(audio: &Audio, range: &Range<usize>, allow_empty: bool) -> Result<(), String> {
    if range.start > range.end || range.end > audio.frames() || (!allow_empty && range.is_empty()) {
        return Err("Select a valid part of the waveform.".into());
    }
    Ok(())
}

pub fn copy_region(audio: &Audio, range: Range<usize>) -> Result<Audio, String> {
    valid_range(audio, &range, false)?;
    let channels = audio.channels as usize;
    Ok(Audio {
        sample_rate: audio.sample_rate,
        channels: audio.channels,
        samples: audio.samples[range.start * channels..range.end * channels].to_vec(),
    })
}

/// Nearest sign change on the first channel within five milliseconds of a selection boundary.
pub fn snap_frame(audio: &Audio, frame: usize) -> usize {
    let count = audio.frames();
    let target = frame.min(count);
    if target == 0 || target == count {
        return target;
    }
    let radius = audio.sample_rate as usize / 200;
    let stride = audio.channels as usize;
    (target.saturating_sub(radius).max(1)..=(target + radius).min(count - 1))
        .filter(|&i| {
            let a = audio.samples[(i - 1) * stride];
            let b = audio.samples[i * stride];
            (a <= 0. && b >= 0.) || (a >= 0. && b <= 0.)
        })
        .min_by_key(|i| i.abs_diff(target))
        .unwrap_or(target)
}

/// Exact signed min/max bins per channel over the half-open visible frame range.
pub fn channel_peaks(audio: &Audio, range: Range<usize>, bins: usize) -> Vec<Vec<[f32; 2]>> {
    if valid_range(audio, &range, false).is_err() || bins == 0 {
        return vec![];
    }
    let channels = audio.channels as usize;
    let count = bins.min(range.len());
    let mut output = vec![Vec::with_capacity(count); channels];
    for i in 0..count {
        let start = range.start + (i as u64 * range.len() as u64 / count as u64) as usize;
        let end = range.start + ((i as u64 + 1) * range.len() as u64 / count as u64) as usize;
        let mut peaks = [[f32::INFINITY, f32::NEG_INFINITY]; 2];
        for frame in audio.samples[start * channels..end * channels].chunks_exact(channels) {
            for (peak, x) in peaks.iter_mut().zip(frame) {
                peak[0] = peak[0].min(*x);
                peak[1] = peak[1].max(*x);
            }
        }
        for (channel, result) in output.iter_mut().zip(peaks) {
            channel.push(result);
        }
    }
    output
}

pub fn apply_edit(
    source: &Audio,
    edit: Edit,
    selection: Option<Range<usize>>,
    fade_seconds: f64,
    gain_db: f64,
    clipboard: Option<&Audio>,
) -> Result<Audio, String> {
    source.validate()?;
    let has_selection = selection.is_some();
    if !has_selection && matches!(edit, Edit::Trim | Edit::Cut | Edit::Silence | Edit::Paste) {
        return Err("Select audio or a paste position on the waveform.".into());
    }
    let range = selection.unwrap_or(0..source.frames());
    valid_range(source, &range, edit == Edit::Paste)?;
    if edit == Edit::Trim {
        return copy_region(source, range);
    }
    if edit == Edit::Cut && range.len() == source.frames() {
        return Err("Cut would remove the entire track. Select a smaller region.".into());
    }
    let channels = source.channels as usize;
    let samples = range.start * channels..range.end * channels;
    if matches!(edit, Edit::Paste | Edit::Cut) {
        let insert = if edit == Edit::Paste {
            let copied = clipboard.ok_or("Copy audio before pasting.")?;
            let converted = resample(copied, source.sample_rate)?;
            if converted.channels == source.channels {
                converted.samples
            } else if source.channels == 1 {
                converted
                    .samples
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[l, r]| (l + r) * 0.5)
                    .collect()
            } else {
                converted.samples.into_iter().flat_map(|v| [v, v]).collect()
            }
        } else {
            vec![]
        };
        let length = source.samples.len() - samples.len() + insert.len();
        if length / channels > 100_000_000 {
            return Err("The edited track exceeds the 100 million frame limit.".into());
        }
        let mut output = Vec::with_capacity(length);
        output.extend_from_slice(&source.samples[..samples.start]);
        output.extend_from_slice(&insert);
        output.extend_from_slice(&source.samples[samples.end..]);
        return Ok(Audio {
            sample_rate: source.sample_rate,
            channels: source.channels,
            samples: output,
        });
    }

    let mut audio = source.clone();
    let region = &mut audio.samples[samples];
    match edit {
        Edit::Silence => region.fill(0.),
        Edit::Gain => {
            if !(-24.0..=24.0).contains(&gain_db) {
                return Err("Gain must be between -24 and +24 dB.".into());
            }
            let gain = db_gain(gain_db);
            for x in region {
                *x *= gain;
            }
        }
        Edit::Normalize => {
            let peak = region.iter().fold(0f32, |p, s| p.max(s.abs()));
            if peak > 0. {
                let gain = db_gain(-0.3) as f64 / peak as f64;
                for x in region {
                    *x = (*x as f64 * gain) as f32;
                }
            }
        }
        Edit::Reverse => {
            let frames = region.len() / channels;
            for i in 0..frames / 2 {
                for ch in 0..channels {
                    region.swap(i * channels + ch, (frames - 1 - i) * channels + ch);
                }
            }
        }
        Edit::FadeIn | Edit::FadeOut => {
            if !has_selection && !(0.01..=10.0).contains(&fade_seconds) {
                return Err("Fade duration must be between 0.01 and 10 seconds.".into());
            }
            let count = if has_selection {
                range.len()
            } else {
                ((fade_seconds * source.sample_rate as f64) as usize).clamp(1, range.len())
            };
            for i in 0..count {
                let gain = (std::f64::consts::FRAC_PI_2 * i as f64
                    / count.saturating_sub(1).max(1) as f64)
                    .sin() as f32;
                let frame = if edit == Edit::FadeIn {
                    i
                } else {
                    range.len() - 1 - i
                };
                for x in &mut region[frame * channels..(frame + 1) * channels] {
                    *x *= gain;
                }
            }
        }
        Edit::Paste | Edit::Cut | Edit::Trim => unreachable!(),
    }
    Ok(audio)
}
