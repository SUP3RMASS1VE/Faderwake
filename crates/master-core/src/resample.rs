//! Polyphase windowed-sinc conversion for device playback, export and clipboard audio.
use crate::{check_cancel, Audio};
use std::f64::consts::PI;

pub struct SincInterpolator {
    pub rates: (u32, u32),
    half: isize,
    phases: usize,
    coefficients: Vec<f32>,
}

impl SincInterpolator {
    pub fn new(source: u32, output: u32) -> Self {
        Self::with_quality(source, output, 16., 256)
    }

    fn with_quality(source: u32, output: u32, radius: f64, phases: usize) -> Self {
        let ratio = (f64::from(output) / f64::from(source)).min(1.0);
        let cutoff = 0.97 * ratio;
        let half = (radius / ratio).ceil() as isize;
        let taps = (half * 2) as usize;
        let mut coefficients = Vec::with_capacity(phases * taps);
        for phase in 0..phases {
            let start = coefficients.len();
            let mut sum = 0.0;
            for tap in 0..taps {
                let distance = tap as f64 - half as f64 + 1.0 - phase as f64 / phases as f64;
                let theta = PI * distance / half as f64;
                let window = 0.42 + 0.5 * theta.cos() + 0.08 * (theta * 2.0).cos();
                let w = distance * PI * cutoff;
                let sinc = if w.abs() < 1e-10 { 1.0 } else { w.sin() / w };
                let weight = window * sinc * cutoff;
                coefficients.push(weight as f32);
                sum += weight;
            }
            for weight in &mut coefficients[start..] {
                *weight /= sum as f32;
            }
        }
        Self {
            rates: (source, output),
            half,
            phases,
            coefficients,
        }
    }

    pub fn sample(&self, audio: &Audio, position: f64) -> [f32; 2] {
        if !position.is_finite() || position < 0. || position >= audio.frames() as f64 {
            return [0.; 2];
        }
        let mut base = position as isize;
        let fraction = position - base as f64;
        let taps = (self.half * 2) as usize;
        let mut phase = (fraction * self.phases as f64).round() as usize;
        if phase == self.phases {
            base += 1;
            phase = 0;
        }
        let weights = &self.coefficients[phase * taps..(phase + 1) * taps];
        let channels = usize::from(audio.channels);
        let first = base - self.half + 1;
        let frames = audio.frames() as isize;
        let mut pair = [0.; 2];
        let first_weight = (-first).clamp(0, taps as isize) as usize;
        let last_weight = (frames - first).clamp(0, taps as isize) as usize;
        let head = first.max(0) as usize * channels;
        let tail = head + (last_weight - first_weight) * channels;
        let samples = &audio.samples[head..tail];
        if channels == 1 {
            pair[0] = weights[first_weight..last_weight]
                .iter()
                .zip(samples)
                .map(|(w, x)| w * x)
                .sum();
            pair[1] = pair[0];
        } else {
            for (&w, frame) in weights[first_weight..last_weight]
                .iter()
                .zip(samples.as_chunks::<2>().0)
            {
                pair[0] += w * frame[0];
                pair[1] += w * frame[1];
            }
        }
        pair
    }
}

pub fn resample(source: &Audio, rate: u32) -> Result<Audio, String> {
    resample_cancellable(source, rate, || false)
}

pub fn resample_cancellable(
    source: &Audio,
    rate: u32,
    cancel: impl Fn() -> bool,
) -> Result<Audio, String> {
    check_cancel(&cancel)?;
    source.validate()?;
    if !(8_000..=384_000).contains(&rate) {
        return Err("Choose a sample rate between 8 and 384 kHz.".into());
    }
    if rate == source.sample_rate {
        return Ok(source.clone());
    }
    let dest_len = ((source.frames() as u64 * rate as u64 + source.sample_rate as u64 / 2)
        / source.sample_rate as u64)
        .max(1);
    if dest_len > 100_000_000 {
        return Err("Converted audio exceeds the 100 million frame limit.".into());
    }
    let mut samples = Vec::with_capacity(dest_len as usize * source.channels as usize);
    let step = source.sample_rate as f64 / rate as f64;
    // Common studio rates have an exact repeating fractional phase (e.g. 160
    // phases for 44.1→48 kHz). Wider filters keep the offline transition narrow.
    let (mut a, mut b) = (source.sample_rate, rate);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let phases = (rate / a).min(2048) as usize;
    let kernel = SincInterpolator::with_quality(source.sample_rate, rate, 64., phases);
    for i in 0..dest_len {
        if i % 4096 == 0 {
            check_cancel(&cancel)?;
        }
        let frame = kernel.sample(source, i as f64 * step);
        samples.extend_from_slice(&frame[..source.channels as usize]);
    }
    Ok(Audio {
        sample_rate: rate,
        channels: source.channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsampling_suppresses_ultrasonic_foldback() {
        let kernel = SincInterpolator::new(96_000, 48_000);
        let amplitude = |frequency: f64| {
            let audio = Audio {
                sample_rate: 96_000,
                channels: 1,
                samples: (0..4096)
                    .map(|i| (2. * PI * frequency * i as f64 / 96_000.).sin() as f32)
                    .collect(),
            };
            (64..2048)
                .step_by(2)
                .map(|i| kernel.sample(&audio, i as f64)[0].abs())
                .fold(0., f32::max)
        };
        assert!(amplitude(6000.) > 0.99);
        assert!(amplitude(36000.) < 0.001);
    }
}
