//! Local mono/stereo mastering at the source sample rate.
mod editor;
mod filters;
pub use editor::{apply_edit, channel_peaks, copy_region, snap_frame, Edit};
mod live_limiter;
mod meter;
pub use live_limiter::LiveLimiter;
mod resample;
mod settings;
mod spectrum;
pub use resample::{resample, resample_cancellable, SincInterpolator};
pub use spectrum::{spectrogram, spectrum, Spectrogram};
mod wav;

pub use filters::ToneChain;
pub use meter::{measure, measure_cancellable, LiveMeter, LiveValues, Metrics};
pub use settings::{Band, Compressor, DeEsser, Settings, Shape, PRESETS};
pub use wav::{decode_wav, write_wav};

#[derive(Clone, Debug)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved, normalized floating-point PCM.
    pub samples: Vec<f32>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }

    /// Verifies the mono/stereo PCM buffer is finite, contains whole frames,
    /// and fits the 100 million frame ceiling used by import and editing.
    pub fn validate(&self) -> Result<(), String> {
        if self.frames() > 100_000_000 {
            return Err("This track exceeds the 100 million frame limit.".into());
        }
        if !(1..=2).contains(&self.channels)
            || !(8_000..=384_000).contains(&self.sample_rate)
            || self.samples.is_empty()
            || !self.samples.len().is_multiple_of(self.channels as usize)
            || self.samples.iter().any(|v| !v.is_finite())
        {
            return Err("Expected finite mono or stereo audio at 8–384 kHz.".into());
        }
        Ok(())
    }
}

pub fn waveform(audio: &Audio, bins: usize) -> Vec<f32> {
    let count = bins.min(audio.frames());
    if count == 0 {
        return vec![];
    }
    let channels = audio.channels as usize;
    (0..count)
        .map(|bin| {
            let first = (bin as u64 * audio.frames() as u64 / count as u64) as usize;
            let last = ((bin as u64 + 1) * audio.frames() as u64 / count as u64) as usize;
            audio.samples[first * channels..last * channels]
                .iter()
                .fold(0.0f32, |p, s| p.max(s.abs()))
        })
        .collect()
}

pub fn audition_gains(original: Metrics, master: Metrics) -> (f32, f32) {
    let bypass_db = if original.lufs.is_finite() && master.lufs.is_finite() {
        master.lufs - original.lufs
    } else {
        0.
    };
    let peak = master.true_peak_db.max(original.true_peak_db + bypass_db);
    let monitor_db = if peak.is_finite() {
        (-1. - peak).min(0.)
    } else {
        0.
    };
    (db_gain(bypass_db + monitor_db), db_gain(monitor_db))
}

fn db_gain(db: f64) -> f32 {
    10.0_f64.powf(db / 20.0) as f32
}

fn check_cancel(cancel: &impl Fn() -> bool) -> Result<(), String> {
    if cancel() {
        Err("Render cancelled.".into())
    } else {
        Ok(())
    }
}

pub fn render(source: &Audio, settings: &Settings) -> Result<Audio, String> {
    render_cancellable(source, settings, || false)
}

pub fn render_cancellable(
    source: &Audio,
    settings: &Settings,
    cancel: impl Fn() -> bool,
) -> Result<Audio, String> {
    render_with_gain(source, settings, cancel).map(|(audio, _)| audio)
}

pub fn render_with_gain(
    source: &Audio,
    settings: &Settings,
    cancel: impl Fn() -> bool,
) -> Result<(Audio, f64), String> {
    check_cancel(&cancel)?;
    source.validate()?;
    settings.validate()?;
    let mut chain = ToneChain::new(settings, source.sample_rate, source.channels);
    let mut processed = source.clone();
    for (i, frame) in processed
        .samples
        .chunks_exact_mut(source.channels as usize)
        .enumerate()
    {
        if i % 8192 == 0 {
            check_cancel(&cancel)?;
        }
        chain.process_frame(frame);
    }

    let loudness = if settings.normalize {
        meter::loudness(&processed, &cancel)?
    } else {
        f64::NEG_INFINITY
    };
    let normalize = loudness.is_finite() && loudness > -70.0;
    let mut gain_db = if normalize {
        settings.target_lufs - loudness
    } else {
        0.0
    };
    let ceiling = db_gain(settings.ceiling_db);
    let input_peaks = if settings.limit {
        meter::frame_peaks(&processed, &cancel)?
    } else {
        vec![]
    };
    let mut best = None;
    let mut best_error = f64::INFINITY;
    for _ in 0..12 {
        check_cancel(&cancel)?;
        let mut output = processed.clone();
        let gain = db_gain(gain_db);
        for v in &mut output.samples {
            *v *= gain;
        }
        if !settings.limit
            || input_peaks
                .iter()
                .all(|p| p * gain <= ceiling * db_gain(-0.15))
        {
            return Ok((output, gain_db));
        }
        meter::limit(&mut output, &input_peaks, gain, ceiling, &cancel)?;
        if !normalize {
            return Ok((output, gain_db));
        }
        let error = settings.target_lufs - meter::loudness(&output, &cancel)?;
        if error.abs() < 0.08 {
            return Ok((output, gain_db));
        }
        // Some targets are unreachable at this ceiling. Keep the closest safe pass.
        if !error.is_finite() || error.abs() >= best_error - 0.001 {
            return Ok(best.unwrap_or((output, gain_db)));
        }
        best_error = error.abs();
        best = Some((output, gain_db));
        gain_db += error.clamp(-6.0, 6.0);
    }
    Ok(best.unwrap_or((processed, 0.0)))
}
