use crate::{Compressor, DeEsser, Settings, Shape};
use std::f64::consts::PI;

#[derive(Clone, Copy)]
pub(crate) struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn normalized(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn same_response(&self, other: &Self) -> bool {
        [self.b0, self.b1, self.b2, self.a1, self.a2]
            == [other.b0, other.b1, other.b2, other.a1, other.a2]
    }
    pub(crate) fn reset(&mut self) {
        self.z1 = 0.;
        self.z2 = 0.;
    }
    pub(crate) fn sample(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn new(shape: Shape, hz: f64, gain: f64, q: f64, rate: f64) -> Self {
        let w = 2.0 * PI * hz.min(rate * 0.45) / rate;
        let (sin, cos) = w.sin_cos();
        let a = 10.0f64.powf(gain / 40.0);
        let alpha = sin / (2.0 * q);
        let beta = 2.0 * a.sqrt() * alpha;
        let (b, d) = match shape {
            Shape::Bell => (
                [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
                [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
            ),
            Shape::HighPass => (
                [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            Shape::LowPass => (
                [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            Shape::Notch => (
                [1.0, -2.0 * cos, 1.0],
                [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
            ),
            Shape::LowShelf => (
                [
                    a * ((a + 1.0) - (a - 1.0) * cos + beta),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - beta),
                ],
                [
                    (a + 1.0) + (a - 1.0) * cos + beta,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - beta,
                ],
            ),
            Shape::HighShelf => (
                [
                    a * ((a + 1.0) + (a - 1.0) * cos + beta),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - beta),
                ],
                [
                    (a + 1.0) - (a - 1.0) * cos + beta,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - beta,
                ],
            ),
        };
        Self::normalized(b, d)
    }

    pub(crate) fn k_weight(rate: f64) -> [Self; 2] {
        let k = (PI * 1681.974450955533 / rate).tan();
        let q = 0.7071752369554196;
        let vh = 10.0f64.powf(3.999843853973347 / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let kh = (PI * 38.13547087602444 / rate).tan();
        let qh = 0.5003270373238773;
        let ah = 1.0 + kh / qh + kh * kh;
        [
            Self::normalized(
                [
                    vh + vb * k / q + k * k,
                    2.0 * (k * k - vh),
                    vh - vb * k / q + k * k,
                ],
                [
                    1.0 + k / q + k * k,
                    2.0 * (k * k - 1.0),
                    1.0 - k / q + k * k,
                ],
            ),
            Self::normalized(
                [ah, -2.0 * ah, ah],
                [ah, 2.0 * (kh * kh - 1.0), 1.0 - kh / qh + kh * kh],
            ),
        ]
    }
}

/// Stateful EQ, glue compressor, and stereo stage. Create once per settings change.
pub struct ToneChain {
    rate: f64,
    channels: u16,
    filters: [Vec<Biquad>; 2],
    input_gain: f64,
    width: f64,
    glue: bool,
    compressor: Compressor,
    de_esser: Option<BandReducer>,
    high_cleaner: Option<BandReducer>,
    center_bass: bool,
    side_filter: Biquad,
    reduction_db: f64,
    attack: f64,
    release: f64,
}

impl ToneChain {
    pub fn new(s: &Settings, rate: u32, channels: u16) -> Self {
        let rate = rate as f64;
        let mut filters = Vec::new();
        let mut add = |shape, hz, gain, q| filters.push(Biquad::new(shape, hz, gain, q, rate));
        if s.clean_low_end {
            add(Shape::HighPass, 30., 0., 0.707);
        }
        for b in s.bands.iter().filter(|_| !s.eq_bypassed) {
            let passive_gain =
                matches!(b.shape, Shape::Bell | Shape::HighShelf | Shape::LowShelf) && b.gain == 0.;
            if b.enabled && !passive_gain {
                add(b.shape, b.frequency, b.gain, b.q);
            }
        }
        if s.cut_mud {
            add(Shape::Bell, 250., -3., 1.5);
        }
        if s.tame_harsh {
            add(Shape::Bell, 4000., -2., 2.);
            add(Shape::Bell, 6000., -1.5, 1.5);
        }
        if s.add_air {
            add(Shape::HighShelf, 12000., 2.5, 0.707);
        }
        if s.smooth_highs {
            add(Shape::LowPass, 16000., 0., 0.707);
        }
        Self {
            rate,
            channels,
            filters: [filters.clone(), filters],
            input_gain: 10.0f64.powf(s.input_gain_db / 20.0),
            width: s.width / 100.0,
            glue: s.glue,
            compressor: s.compressor,
            de_esser: s.de_esser.map(|d| BandReducer::new(d, rate)),
            high_cleaner: s.smooth_highs.then(|| {
                BandReducer::new(
                    DeEsser {
                        frequency: 10500.,
                        threshold: -36.,
                        reduction: 8.,
                    },
                    rate,
                )
            }),
            center_bass: s.center_bass,
            side_filter: Biquad::new(Shape::HighPass, 120., 0., 0.7, rate),
            reduction_db: 0.,
            attack: (-1. / (rate * s.compressor.attack / 1000.)).exp(),
            release: (-1. / (rate * s.compressor.release / 1000.)).exp(),
        }
    }

    /// Continue compatible filter histories when changing controls during playback.
    /// The compressor/reducer envelopes survive parameter changes to prevent gain jumps.
    pub fn inherit_dynamics(&mut self, previous: &Self) {
        if self.rate != previous.rate || self.channels != previous.channels {
            return;
        }
        let same_filters = self.input_gain == previous.input_gain
            && self.filters[0].len() == previous.filters[0].len()
            && self.filters[0]
                .iter()
                .zip(&previous.filters[0])
                .all(|(a, b)| a.same_response(b));
        if same_filters {
            for (new, old) in self.filters.iter_mut().zip(&previous.filters) {
                new.copy_from_slice(old);
            }
        }
        if self.glue && previous.glue {
            self.reduction_db = previous.reduction_db;
        }
        if let (Some(now), Some(old)) = (&mut self.high_cleaner, &previous.high_cleaner) {
            now.reduction_db = old.reduction_db;
            if same_filters {
                now.high_pass = old.high_pass;
            }
        }
        let same_cleaner = self.high_cleaner.is_some() == previous.high_cleaner.is_some();
        if let (Some(now), Some(old)) = (&mut self.de_esser, &previous.de_esser) {
            now.reduction_db = old.reduction_db;
            if same_filters && same_cleaner && now.settings.frequency == old.settings.frequency {
                now.high_pass = old.high_pass;
            }
        }
        let same_de_esser = self.de_esser.as_ref().map(|d| d.settings)
            == previous.de_esser.as_ref().map(|d| d.settings);
        if same_filters
            && same_cleaner
            && same_de_esser
            && self.glue == previous.glue
            && self.compressor == previous.compressor
            && self.width == previous.width
            && self.center_bass
            && previous.center_bass
        {
            self.side_filter = previous.side_filter;
        }
    }

    pub fn process_frame(&mut self, frame: &mut [f32]) {
        debug_assert!((1..=2).contains(&frame.len()));
        let mut pair = [0.0; 2];
        for (ch, value) in frame.iter().enumerate() {
            let mut x = *value as f64 * self.input_gain;
            for filter in &mut self.filters[ch] {
                x = filter.sample(x);
            }
            pair[ch] = x;
        }
        if let Some(cleaner) = &mut self.high_cleaner {
            cleaner.process(&mut pair[..frame.len()], true);
        }
        if let Some(reducer) = &mut self.de_esser {
            reducer.process(&mut pair[..frame.len()], false);
        }
        if self.glue {
            let peak = pair[0].abs().max(pair[1].abs()).max(1e-12);
            let over = 20.0 * peak.log10() - self.compressor.threshold;
            let slope = 1.0 - 1.0 / self.compressor.ratio;
            let target = if over < -5.0 {
                0.0
            } else if over > 5.0 {
                over * slope
            } else {
                (over + 5.).powi(2) * slope / 20.0
            };
            let coeff = if target > self.reduction_db {
                self.attack
            } else {
                self.release
            };
            self.reduction_db = coeff * self.reduction_db + (1.0 - coeff) * target;
            let gain = 10.0f64.powf((self.compressor.makeup - self.reduction_db) / 20.0);
            pair[0] *= gain;
            pair[1] *= gain;
        }
        if frame.len() == 2 && (self.width != 1.0 || self.center_bass) {
            let mid = (pair[0] + pair[1]) * 0.5;
            let mut side = (pair[0] - pair[1]) * 0.5 * self.width;
            if self.center_bass {
                side = self.side_filter.sample(side);
            }
            pair = [mid + side, mid - side];
        }
        for (sample, out) in frame.iter_mut().zip(pair) {
            *sample = out as f32;
        }
    }
}

struct BandReducer {
    settings: DeEsser,
    high_pass: [Biquad; 2],
    attack: f64,
    release: f64,
    reduction_db: f64,
}

impl BandReducer {
    fn new(settings: DeEsser, rate: f64) -> Self {
        Self {
            settings,
            high_pass: [Biquad::new(Shape::HighPass, settings.frequency, 0., 0.707, rate); 2],
            attack: (-1. / (rate * 0.001)).exp(),
            release: (-1. / (rate * 0.06)).exp(),
            reduction_db: 0.,
        }
    }

    fn process(&mut self, samples: &mut [f64], side_boost: bool) {
        let mut highs = [0f64; 2];
        for (i, &x) in samples.iter().enumerate() {
            highs[i] = self.high_pass[i].sample(x);
        }
        let level_db = 20. * highs[0].abs().max(highs[1].abs()).max(1e-12).log10();
        let target =
            ((level_db - self.settings.threshold) * 0.75).clamp(0., self.settings.reduction);
        let coeff = if target > self.reduction_db {
            self.attack
        } else {
            self.release
        };
        self.reduction_db = coeff * self.reduction_db + (1. - coeff) * target;
        let amount = 1. - 10f64.powf(-self.reduction_db / 20.);
        if side_boost && samples.len() == 2 {
            let mid = (highs[0] + highs[1]) * 0.5 * amount;
            let side = (highs[0] - highs[1]) * 0.5 * (amount * 1.35 + 0.12).min(0.85);
            samples[0] -= mid + side;
            samples[1] -= mid - side;
        } else {
            for (sample, high) in samples.iter_mut().zip(highs) {
                *sample -= high * amount;
            }
        }
    }
}
