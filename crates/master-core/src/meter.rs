use crate::filters::Biquad;
use crate::{check_cancel, db_gain, Audio};
use serde::Serialize;
use std::collections::VecDeque;
use std::f64::consts::PI;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub lufs: f64,
    pub true_peak_db: f64,
    pub range: f64,
    pub rms_db: f64,
    pub crest_db: f64,
    pub mid_db: f64,
    pub side_db: f64,
    pub correlation: f64,
}

pub fn measure(audio: &Audio) -> Metrics {
    measure_cancellable(audio, || false).unwrap_or(Metrics {
        lufs: f64::NEG_INFINITY,
        true_peak_db: f64::NEG_INFINITY,
        range: 0.,
        rms_db: f64::NEG_INFINITY,
        crest_db: 0.,
        mid_db: f64::NEG_INFINITY,
        side_db: f64::NEG_INFINITY,
        correlation: 1.,
    })
}

pub fn measure_cancellable(audio: &Audio, cancel: impl Fn() -> bool) -> Result<Metrics, String> {
    check_cancel(&cancel)?;
    audio.validate()?;
    let hops = hop_energies(audio, &cancel)?;
    let lufs = integrated(&hops);
    let range = loudness_range(&hops);
    let peak = frame_peaks(audio, &cancel)?.into_iter().fold(0., f32::max);
    let mut sums = StereoEnergy::default();
    for (i, frame) in audio
        .samples
        .chunks_exact(audio.channels as usize)
        .enumerate()
    {
        if i % 8192 == 0 {
            check_cancel(&cancel)?;
        }
        sums.push(frame);
    }
    let count = audio.frames() as f64;
    let rms_db = power_db((sums.ll + sums.rr) / (2. * count));
    let sample_peak = audio.samples.iter().fold(0f32, |p, s| p.max(s.abs()));
    let crest_db = if rms_db.is_finite() {
        20. * (sample_peak as f64).log10() - rms_db
    } else {
        0.
    };
    Ok(Metrics {
        lufs,
        true_peak_db: 20. * (peak as f64).log10(),
        range,
        rms_db,
        crest_db,
        mid_db: power_db((sums.ll + sums.rr + 2. * sums.lr) / (4. * count)),
        side_db: power_db((sums.ll + sums.rr - 2. * sums.lr) / (4. * count)),
        correlation: sums.correlation(),
    })
}

fn power_db(energy: f64) -> f64 {
    10. * energy.max(0.).log10()
}

#[derive(Default)]
struct StereoEnergy {
    ll: f64,
    rr: f64,
    lr: f64,
}
impl StereoEnergy {
    fn push(&mut self, frame: &[f32]) {
        let l = frame[0] as f64;
        let r = *frame.last().unwrap() as f64;
        self.ll += l * l;
        self.rr += r * r;
        self.lr += l * r;
    }
    fn correlation(&self) -> f64 {
        let denominator = (self.ll * self.rr).sqrt();
        if denominator > 1e-12 {
            (self.lr / denominator).clamp(-1., 1.)
        } else {
            1.
        }
    }
}

fn hop_energies(audio: &Audio, cancel: &impl Fn() -> bool) -> Result<Vec<f64>, String> {
    let channels = audio.channels as usize;
    let hop = (audio.sample_rate / 10) as usize;
    let mut weights = [Biquad::k_weight(audio.sample_rate as f64); 2];
    let mut hops = Vec::with_capacity(audio.frames() / hop);
    for samples in audio.samples.chunks_exact(hop * channels) {
        check_cancel(cancel)?;
        let mut sum = 0.0;
        for frame in samples.chunks_exact(channels) {
            for (s, [shelf, high_pass]) in frame.iter().zip(&mut weights) {
                let y = high_pass.sample(shelf.sample(*s as f64));
                sum += y * y;
            }
        }
        hops.push(sum / hop as f64);
    }
    Ok(hops)
}

fn gated_blocks(hops: &[f64], length: usize, relative_power: f64) -> Vec<f64> {
    let absolute = 10.0f64.powf((-70. + 0.691) / 10.);
    let mut blocks: Vec<_> = hops
        .windows(length)
        .map(|window| window.iter().sum::<f64>() / length as f64)
        .filter(|energy| *energy > absolute)
        .collect();
    if !blocks.is_empty() {
        let gate = blocks.iter().sum::<f64>() / blocks.len() as f64 * relative_power;
        blocks.retain(|e| *e > gate);
    }
    blocks
}

fn integrated(hops: &[f64]) -> f64 {
    let blocks = gated_blocks(hops, 4, 0.1);
    if blocks.is_empty() {
        return f64::NEG_INFINITY;
    }
    -0.691 + power_db(blocks.iter().sum::<f64>() / blocks.len() as f64)
}

fn loudness_range(hops: &[f64]) -> f64 {
    let mut blocks = gated_blocks(hops, 30, 0.01);
    if blocks.len() < 2 {
        return 0.;
    }
    blocks.sort_unstable_by(f64::total_cmp);
    let last = (blocks.len() - 1) as f64;
    power_db(blocks[(last * 0.95) as usize] / blocks[(last * 0.10) as usize])
}

pub(crate) fn loudness(audio: &Audio, cancel: &impl Fn() -> bool) -> Result<f64, String> {
    Ok(integrated(&hop_energies(audio, cancel)?))
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveValues {
    pub momentary: f64,
    pub short_term: f64,
    pub correlation: f64,
}

/// Post-audition K-weighted playback meter. All storage is allocated before playback.
pub struct LiveMeter {
    weights: [[Biquad; 2]; 2],
    hop_length: usize,
    channels: usize,
    frames: usize,
    current: f64,
    stereo: StereoEnergy,
    energy: [f64; 30],
    cursor: usize,
    count: usize,
    values: LiveValues,
}

impl LiveMeter {
    pub fn new(rate: u32, channels: u16) -> Self {
        Self {
            weights: [Biquad::k_weight(rate as f64); 2],
            hop_length: (rate as usize / 10).max(1),
            channels: usize::from(channels.clamp(1, 2)),
            frames: 0,
            current: 0.,
            stereo: StereoEnergy::default(),
            energy: [0.; 30],
            cursor: 0,
            count: 0,
            values: LiveValues {
                momentary: f64::NEG_INFINITY,
                short_term: f64::NEG_INFINITY,
                correlation: 1.,
            },
        }
    }
    pub fn reset(&mut self) {
        for channel in &mut self.weights {
            for filter in channel {
                filter.reset();
            }
        }
        self.frames = 0;
        self.count = 0;
        self.cursor = 0;
        self.current = 0.;
        self.energy.fill(0.);
        self.stereo = StereoEnergy::default();
        self.values = LiveValues {
            momentary: f64::NEG_INFINITY,
            short_term: f64::NEG_INFINITY,
            correlation: 1.,
        };
    }
    pub fn values(&self) -> LiveValues {
        self.values
    }
    pub fn push(&mut self, frame: &[f32]) {
        if frame.len() < self.channels {
            return;
        }
        let frame = &frame[..self.channels];
        self.stereo.push(frame);
        for (&value, [shelf, hp]) in frame.iter().zip(&mut self.weights) {
            let y = hp.sample(shelf.sample(value as f64));
            self.current += y * y;
        }
        self.frames += 1;
        if self.frames == self.hop_length {
            self.energy[self.cursor] = self.current / self.frames as f64;
            self.cursor = (self.cursor + 1) % self.energy.len();
            self.count = (self.count + 1).min(self.energy.len());
            let moment = self.count.min(4);
            let energy = (1..=moment)
                .map(|i| self.energy[(self.cursor + self.energy.len() - i) % self.energy.len()])
                .sum::<f64>();
            self.values.momentary = -0.691 + power_db(energy / moment as f64);
            self.values.short_term =
                -0.691 + power_db(self.energy.iter().sum::<f64>() / self.count as f64);
            self.values.correlation =
                0.7 * self.values.correlation + 0.3 * self.stereo.correlation();
            self.current = 0.;
            self.frames = 0;
            self.stereo = StereoEnergy::default();
        }
    }
}

// Three fractional phases supplement the original samples, measuring at 4x.
// The window rolls off discontinuities at the FIR edges. Zero-pad track boundaries.
pub(crate) const TAPS: usize = 32;
pub(crate) fn interpolator() -> &'static [[f32; TAPS]; 3] {
    static PHASES: OnceLock<[[f32; TAPS]; 3]> = OnceLock::new();
    PHASES.get_or_init(|| {
        std::array::from_fn(|phase| {
            let mut coeff = [0.0; TAPS];
            for (tap, c) in coeff.iter_mut().enumerate() {
                let x = (tap as f64 - (TAPS / 2 - 1) as f64) - (phase + 1) as f64 / 4.;
                let window = 0.42
                    + 0.5 * (PI * x / (TAPS / 2) as f64).cos()
                    + 0.08 * (2. * PI * x / (TAPS / 2) as f64).cos();
                *c = ((PI * x).sin() / (PI * x) * window) as f32;
            }
            let norm = coeff.iter().sum::<f32>();
            for c in &mut coeff {
                *c /= norm;
            }
            coeff
        })
    })
}

pub(crate) fn frame_peaks(audio: &Audio, cancel: &impl Fn() -> bool) -> Result<Vec<f32>, String> {
    let channels = audio.channels as usize;
    let frames = audio.frames();
    let mut peaks = vec![0.0f32; frames];
    let filters = interpolator();
    let mut padded = vec![0.0f32; frames + TAPS];
    for ch in 0..channels {
        for (dest, frame) in padded[TAPS / 2 - 1..]
            .iter_mut()
            .zip(audio.samples.chunks_exact(channels))
        {
            *dest = frame[ch];
        }
        for (i, peak) in peaks.iter_mut().enumerate() {
            if i % 8192 == 0 {
                check_cancel(cancel)?;
            }
            let window = &padded[i..i + TAPS];
            *peak = peak.max(window[TAPS / 2 - 1].abs());
            for filter in filters {
                // Independent partial sums enable SIMD without reassociating the filter.
                let mut sums = [0.0f32; 4];
                for (x, h) in window
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(filter.as_chunks::<4>().0)
                {
                    for j in 0..4 {
                        sums[j] += x[j] * h[j];
                    }
                }
                *peak = peak.max(sums.iter().sum::<f32>().abs());
            }
        }
    }
    Ok(peaks)
}

pub(crate) fn limit(
    audio: &mut Audio,
    source_peaks: &[f32],
    scale: f32,
    ceiling: f32,
    cancel: &impl Fn() -> bool,
) -> Result<(), String> {
    let lookahead = ((audio.sample_rate as f64 * 0.005) as usize).max(1);
    let release = (-1.0 / (audio.sample_rate as f64 * 0.05)).exp() as f32;
    let safety = ceiling * db_gain(-0.15);
    let mut queue = VecDeque::<usize>::with_capacity(lookahead + 1);
    let mut next = 0;
    let mut history = vec![1.0f32; lookahead];
    let mut sum = lookahead as f64;
    let mut level = 1.0f32;
    for (i, frame) in audio
        .samples
        .chunks_exact_mut(audio.channels as usize)
        .enumerate()
    {
        if i % 8192 == 0 {
            check_cancel(cancel)?;
        }
        while queue.front().is_some_and(|&front| front < i) {
            queue.pop_front();
        }
        while next < source_peaks.len() && next <= i + lookahead {
            while queue
                .back()
                .is_some_and(|&back| source_peaks[back] <= source_peaks[next])
            {
                queue.pop_back();
            }
            queue.push_back(next);
            next += 1;
        }
        let future_peak = source_peaks[*queue.front().unwrap()] * scale;
        let target = safety / future_peak.max(safety);
        let old = std::mem::replace(&mut history[i % lookahead], target);
        sum += target as f64 - old as f64;
        let smooth_target = (sum / lookahead as f64) as f32;
        level = smooth_target
            .min(safety / (source_peaks[i] * scale).max(safety))
            .min(release * level + (1. - release));
        for v in frame {
            *v *= level;
        }
    }
    // Gain modulation can introduce new intersample peaks. Verify the rendered
    // signal and apply a constant correction, preserving the envelope.
    let peak = frame_peaks(audio, cancel)?.into_iter().fold(0.0, f32::max);
    if peak > ceiling {
        let correction = safety / peak;
        for sample in &mut audio.samples {
            *sample *= correction;
        }
    }
    Ok(())
}
