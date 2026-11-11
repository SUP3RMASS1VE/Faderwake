use crate::{check_cancel, Audio};
use serde::Serialize;
use std::f32::consts::TAU;
use std::sync::OnceLock;

const SIZE: usize = 2048;
struct Plan {
    window: [f32; SIZE],
    twiddle: [[f32; 2]; SIZE / 2],
    order: [usize; SIZE],
}
fn plan() -> &'static Plan {
    static PLAN: OnceLock<Plan> = OnceLock::new();
    PLAN.get_or_init(|| Plan {
        window: std::array::from_fn(|i| 0.5 - 0.5 * (TAU * i as f32 / SIZE as f32).cos()),
        twiddle: std::array::from_fn(|i| {
            let (s, c) = (-TAU * i as f32 / SIZE as f32).sin_cos();
            [c, s]
        }),
        order: std::array::from_fn(|i| i.reverse_bits() >> (usize::BITS - SIZE.ilog2())),
    })
}

fn power(audio: &Audio, start: isize, average_channels: bool) -> [f32; SIZE / 2 + 1] {
    let plan = plan();
    let mut power = [0f32; SIZE / 2 + 1];
    let mut work = [[0f32; 2]; SIZE];
    let channels = audio.channels as usize;
    for ch in 0..channels {
        for i in 0..SIZE {
            let index = start + i as isize;
            let value = if index >= 0 && index < audio.frames() as isize {
                audio.samples[index as usize * channels + ch]
            } else {
                0.
            };
            work[plan.order[i]] = [value * plan.window[i], 0.];
        }
        let mut length = 2;
        while length <= SIZE {
            let half = length / 2;
            let step = SIZE / length;
            for stage in work.chunks_exact_mut(length) {
                let (first, second) = stage.split_at_mut(half);
                for (j, (a, b)) in first.iter_mut().zip(second).enumerate() {
                    let [wr, wi] = plan.twiddle[j * step];
                    let re = wr * b[0] - wi * b[1];
                    let im = wr * b[1] + wi * b[0];
                    *b = [a[0] - re, a[1] - im];
                    a[0] += re;
                    a[1] += im;
                }
            }
            length *= 2;
        }
        for (band, [re, im]) in power.iter_mut().zip(work) {
            let p = re * re + im * im;
            if average_channels {
                *band += p / channels as f32;
            } else {
                *band = band.max(p);
            }
        }
    }
    power
}

fn ranges(rate: u32, bands: usize, low: f64) -> Vec<std::ops::RangeInclusive<usize>> {
    let high = 20000f64.min(rate as f64 / 2.);
    let edge = |i: usize| {
        let hz = low * (high / low).powf(i as f64 / bands as f64);
        ((hz * SIZE as f64 / rate as f64) as usize).clamp(1, SIZE / 2)
    };
    (0..bands).map(|i| edge(i)..=edge(i + 1)).collect()
}

fn level(power: &[f32]) -> f32 {
    let max = power.iter().copied().fold(0f32, f32::max);
    let amplitude = max.sqrt() * 4. / SIZE as f32;
    ((20. * amplitude.max(1e-4).log10() + 80.) / 80.).clamp(0., 1.)
}

/// Log-frequency band levels of the audio ending at the cursor, normalized from −80…0 dBFS.
pub fn spectrum(audio: &Audio, position: usize, bins: usize) -> Vec<f32> {
    if bins == 0 || audio.samples.is_empty() {
        return vec![];
    }
    let power = power(
        audio,
        position.min(audio.frames()) as isize - SIZE as isize,
        true,
    );
    ranges(audio.sample_rate, bins, 20.)
        .into_iter()
        .map(|r| level(&power[r]))
        .collect()
}

#[derive(Debug, Serialize)]
pub struct Spectrogram {
    pub width: usize,
    pub height: usize,
    pub levels: Vec<u8>,
}

/// Offline 2,048-point Hann STFT. Rows go from high frequencies to low frequencies.
pub fn spectrogram(audio: &Audio, cancel: impl Fn() -> bool) -> Result<Spectrogram, String> {
    audio.validate()?;
    let width = ((audio.duration() * 24.) as usize).clamp(1, 1600);
    let height = 80;
    let rows = ranges(audio.sample_rate, height, 40.);
    let mut levels = vec![0; width * height];
    for x in 0..width {
        check_cancel(&cancel)?;
        let center = (x as u64 * audio.frames() as u64 / width as u64) as isize;
        let power = power(audio, center - (SIZE / 2) as isize, false);
        for (y, r) in rows.iter().rev().enumerate() {
            levels[y * width + x] = (255. * level(&power[r.clone()])).round() as u8;
        }
    }
    Ok(Spectrogram {
        width,
        height,
        levels,
    })
}
