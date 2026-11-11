use crate::{
    db_gain,
    meter::{interpolator, TAPS},
};
use std::collections::VecDeque;

/// Stereo-linked true-peak limiter for continuous playback. The principal stage
/// follows the offline lookahead envelope. A second short stage checks peaks
/// created by modulation. Callers prime/drain its known latency at boundaries.
pub struct LiveLimiter {
    main: Stage,
    guard: Stage,
}

impl LiveLimiter {
    pub fn new(rate: u32, ceiling_db: f64) -> Self {
        let ceiling = db_gain(ceiling_db.clamp(-12., 0.));
        let _ = interpolator();
        Self {
            main: Stage::new(
                rate,
                0.005,
                ceiling * db_gain(-0.15),
                ceiling * db_gain(-0.15),
            ),
            guard: Stage::new(rate, 0.001, ceiling, ceiling * db_gain(-0.15)),
        }
    }
    pub fn latency(&self) -> usize {
        self.main.delay.len() + self.guard.delay.len() - 2
    }
    pub fn reset(&mut self) {
        self.main.reset();
        self.guard.reset();
    }
    pub fn push(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let main = self.main.push(frame);
        self.guard.push(main)
    }
}

struct Stage {
    delay: Vec<[f32; 2]>,
    window: [[f32; TAPS * 2]; 2],
    peaks: VecDeque<(usize, f32)>,
    frame_peaks: Vec<f32>,
    targets: Vec<f32>,
    sum: f64,
    frames: usize,
    release: f32,
    level: f32,
    threshold: f32,
    safety: f32,
}

impl Stage {
    fn new(rate: u32, seconds: f64, threshold: f32, safety: f32) -> Self {
        let lookahead = (rate as f64 * seconds).max(1.) as usize;
        Self {
            delay: vec![[0.; 2]; TAPS / 2 + lookahead + 1],
            window: [[0.; TAPS * 2]; 2],
            peaks: VecDeque::with_capacity(lookahead + 2),
            frame_peaks: vec![0.; lookahead + 1],
            targets: vec![1.; lookahead],
            sum: lookahead as f64,
            frames: 0,
            release: (-1. / (rate as f64 * 0.05)).exp() as f32,
            level: 1.,
            threshold,
            safety,
        }
    }
    fn reset(&mut self) {
        self.delay.fill([0.; 2]);
        self.window = [[0.; TAPS * 2]; 2];
        self.peaks.clear();
        self.frame_peaks.fill(0.);
        self.targets.fill(1.);
        self.sum = self.targets.len() as f64;
        self.frames = 0;
        self.level = 1.;
    }

    fn target(&self, peak: f32) -> f32 {
        if peak > self.threshold {
            self.safety / peak
        } else {
            1.
        }
    }

    fn push(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let n = self.frames;
        self.frames += 1;
        let delay_slot = n % self.delay.len();
        self.delay[delay_slot] = frame;
        let cursor = n % TAPS;
        let start = (cursor + 1) % TAPS;
        let mut peak = 0f32;
        for (ch, history) in self.window.iter_mut().enumerate() {
            history[cursor] = frame[ch];
            history[cursor + TAPS] = frame[ch];
            let sample = &history[start..start + TAPS];
            peak = peak.max(sample[TAPS / 2 - 1].abs());
            for filter in interpolator() {
                let mut accum = [0f32; 4];
                for (xs, hs) in sample
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(filter.as_chunks::<4>().0)
                {
                    for j in 0..4 {
                        accum[j] += xs[j] * hs[j];
                    }
                }
                peak = peak.max(accum.into_iter().sum::<f32>().abs());
            }
        }
        if n < TAPS / 2 {
            return [0.; 2];
        }
        let i = n - TAPS / 2;
        let lookahead = self.targets.len();
        let peak_slot = i % self.frame_peaks.len();
        self.frame_peaks[peak_slot] = peak;
        let output_index = i.saturating_sub(lookahead);
        while self.peaks.front().is_some_and(|&(t, _)| t < output_index) {
            self.peaks.pop_front();
        }
        while self.peaks.back().is_some_and(|&(_, p)| p <= peak) {
            self.peaks.pop_back();
        }
        self.peaks.push_back((i, peak));
        if i < lookahead {
            return [0.; 2];
        }
        let target = self.target(self.peaks.front().unwrap().1);
        let old = std::mem::replace(&mut self.targets[output_index % lookahead], target);
        self.sum += target as f64 - old as f64;
        let immediate = self.target(self.frame_peaks[output_index % self.frame_peaks.len()]);
        let smooth = (self.sum / lookahead as f64) as f32;
        self.level = smooth
            .min(immediate)
            .min(self.release * self.level + (1. - self.release));
        let mut out = self.delay[(delay_slot + 1) % self.delay.len()];
        for x in &mut out {
            *x *= self.level;
        }
        out
    }
}
