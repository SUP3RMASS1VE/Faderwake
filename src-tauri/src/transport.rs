use crate::resample::Kernel;
use arc_swap::ArcSwapOption;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream};
use master_core::{Audio, LiveLimiter, LiveMeter, Settings, ToneChain};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

const ORDER: Ordering = Ordering::SeqCst;

pub struct PlaybackMix {
    pub id: u64,
    pub revision: u64,
    pub source: Arc<Audio>,
    pub master: Option<Arc<Audio>>,
    pub gains: (f32, f32),
    pub normalization: f32,
    pub settings: Settings,
    preview: Mutex<Option<Monitor>>,
}

impl PlaybackMix {
    pub fn new(
        id: u64,
        revision: u64,
        source: Arc<Audio>,
        master: Option<Arc<Audio>>,
        gains: (f32, f32),
        normalization_db: f64,
        settings: Settings,
    ) -> Self {
        Self {
            id,
            revision,
            source,
            master,
            gains,
            normalization: 10f32.powf(normalization_db as f32 / 20.0),
            settings,
            preview: Mutex::default(),
        }
    }

    fn prepare(&self, rate: u32, seconds: f64) {
        if self.master.is_some() || rate == 0 {
            return;
        }
        let mut tone = ToneChain::new(&self.settings, rate, self.source.channels);
        // Warm EQ delays before the callback picks up the new settings.
        let end = (seconds.clamp(0., self.source.duration()) * rate as f64).round() as usize;
        let step = self.source.sample_rate as f64 / rate as f64;
        for i in end.saturating_sub(rate as usize / 10)..end {
            let position = (i as f64 * step).min((self.source.frames() - 1) as f64);
            let mut pair = stereo(&self.source, position);
            tone.process_frame(&mut pair[..self.source.channels as usize]);
        }
        let limiter = self
            .settings
            .limit
            .then(|| LiveLimiter::new(rate, self.settings.ceiling_db));
        *self.preview.lock().unwrap() = Some(Monitor {
            tone,
            limiter,
            primed: false,
        });
    }
}

struct Monitor {
    tone: ToneChain,
    limiter: Option<LiveLimiter>,
    primed: bool,
}
impl Monitor {
    fn push(&mut self, mut pair: [f32; 2], channels: u16, gain: f32) -> [f32; 2] {
        self.tone.process_frame(&mut pair[..channels as usize]);
        if channels == 1 {
            pair[1] = pair[0];
        }
        pair[0] *= gain;
        pair[1] *= gain;
        if let Some(limiter) = self.limiter.as_mut() {
            limiter.push(pair)
        } else {
            pair
        }
    }
    fn next(&mut self, at: impl Fn(usize) -> [f32; 2], channels: u16, gain: f32) -> [f32; 2] {
        let delay = self.limiter.as_ref().map_or(0, LiveLimiter::latency);
        if !self.primed {
            for i in 0..delay {
                self.push(at(i), channels, gain);
            }
            self.primed = true;
        }
        self.push(at(delay), channels, gain)
    }
}

// The UI reads an independent window of post-monitor samples at display rate.
// The audio callback never waits for the analyzer or runs an FFT.
struct Scope {
    ring: Box<[AtomicU64]>,
    end: AtomicU64,
}
impl Default for Scope {
    fn default() -> Self {
        Self {
            ring: (0..8192).map(|_| AtomicU64::new(0)).collect(),
            end: AtomicU64::new(0),
        }
    }
}
impl Scope {
    fn write(&self, index: u64, frame: [f32; 2]) {
        let packed = u64::from(frame[0].to_bits()) | (u64::from(frame[1].to_bits()) << 32);
        self.ring[index as usize % self.ring.len()].store(packed, Ordering::Relaxed);
    }
    fn spectrum(&self, rate: u32) -> Vec<f32> {
        let end = self.end.load(Ordering::Acquire);
        if end == 0 || rate == 0 {
            return vec![];
        }
        let start = end.saturating_sub(2048);
        let mut samples = Vec::with_capacity(4096);
        for i in start..end {
            let bits = self.ring[i as usize % self.ring.len()].load(Ordering::Relaxed);
            samples.extend_from_slice(&[
                f32::from_bits(bits as u32),
                f32::from_bits((bits >> 32) as u32),
            ]);
        }
        let audio = Audio {
            samples,
            sample_rate: rate,
            channels: 2,
        };
        master_core::spectrum(&audio, audio.frames(), 80)
    }
}
#[derive(Default)]
struct Signals {
    mix: ArcSwapOption<PlaybackMix>,
    scope: Scope,
    resampler: ArcSwapOption<Kernel>,
    master_resampler: ArcSwapOption<Kernel>,
    playing: AtomicBool,
    original: AtomicBool,
    matched: AtomicBool,
    position: AtomicU64,
    seek_position: AtomicU64,
    seek_id: AtomicU64,
    loop_range: ArcSwapOption<[f64; 2]>,
    momentary: AtomicU64,
    short_term: AtomicU64,
    correlation: AtomicU64,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    device_rate: AtomicU32,
    device_lost: AtomicBool,
    play_request: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Signals {
    fn configure_resamplers(&self, mix: &PlaybackMix, output: u32) {
        let prepare = |slot: &ArcSwapOption<Kernel>, source: u32| {
            if source != output
                && !slot
                    .load()
                    .as_ref()
                    .is_some_and(|k| k.rates == (source, output))
            {
                slot.store(Some(Arc::new(Kernel::new(source, output))));
            }
        };
        prepare(&self.resampler, mix.source.sample_rate);
        if let Some(master) = &mix.master {
            prepare(&self.master_resampler, master.sample_rate);
        }
    }
}

#[derive(Clone)]
pub struct Transport {
    signals: Arc<Signals>,
    device: mpsc::Sender<mpsc::SyncSender<Result<(), String>>>,
}

impl Transport {
    pub fn new() -> Self {
        let signals = Arc::new(Signals::default());
        signals.matched.store(true, ORDER);
        let (device, requests) = mpsc::channel::<mpsc::SyncSender<Result<(), String>>>();
        let thread_signals = signals.clone();
        // CPAL streams are thread-affine on some platforms.
        thread::spawn(move || {
            let mut stream: Option<Stream> = None;
            let mut running = false;
            loop {
                let reply = if running {
                    match requests.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(reply) => reply,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if !thread_signals.playing.load(ORDER) {
                                if let Some(output) = &stream {
                                    running = output.pause().is_err();
                                }
                            }
                            continue;
                        }
                    }
                } else {
                    let Ok(reply) = requests.recv() else {
                        break;
                    };
                    reply
                };
                if thread_signals.device_lost.swap(false, ORDER) {
                    stream.take();
                    running = false;
                }
                let mut start = || -> Result<(), String> {
                    if stream.is_none() {
                        stream = Some(build_stream(thread_signals.clone())?);
                    }
                    if !running {
                        stream
                            .as_ref()
                            .unwrap()
                            .play()
                            .map_err(|e| format!("Cannot start playback: {e}"))?;
                        running = true;
                    }
                    Ok(())
                };
                let _ = reply.send(start());
            }
        });
        Self { signals, device }
    }

    pub fn publish(&self, mix: PlaybackMix) {
        let rate = self.signals.device_rate.load(ORDER);
        let same = self
            .signals
            .mix
            .load()
            .as_ref()
            .is_some_and(|old| old.id == mix.id);
        let position = if same { self.position() } else { 0. };
        if rate > 0 {
            self.signals.configure_resamplers(&mix, rate);
            mix.prepare(rate, position);
        }
        let mix = Arc::new(mix);
        self.signals.mix.store(Some(mix.clone()));
        let opened_rate = self.signals.device_rate.load(ORDER);
        if opened_rate != 0 && opened_rate != rate {
            self.signals.configure_resamplers(&mix, opened_rate);
            mix.prepare(opened_rate, position);
        }
    }

    pub fn set_playing(&self, playing: bool) -> Result<(), String> {
        let request = self.signals.play_request.fetch_add(1, ORDER) + 1;
        if playing {
            let mix = self
                .signals
                .mix
                .load_full()
                .ok_or("Open an audio file first.")?;
            let (send, receive) = mpsc::sync_channel(1);
            self.device
                .send(send)
                .map_err(|_| "The playback device thread stopped.")?;
            receive
                .recv()
                .map_err(|_| "The playback device stopped responding.")??;
            if self.signals.play_request.load(ORDER) != request {
                return Ok(());
            }
            if self.position() >= mix.source.duration() {
                self.seek(0.0)?;
            }
            self.signals.error.lock().unwrap().take();
        }
        self.signals.playing.store(playing, ORDER);
        if !playing {
            self.signals.peak_left.store(0, ORDER);
            self.signals.peak_right.store(0, ORDER);
        }
        Ok(())
    }

    pub fn seek(&self, seconds: f64) -> Result<(), String> {
        let snapshot = self.signals.mix.load();
        let mix = snapshot.as_ref().ok_or("Open an audio file first.")?;
        if !seconds.is_finite() {
            return Err("Invalid seek position.".into());
        }
        let seconds = seconds.clamp(0.0, mix.source.duration());
        mix.prepare(self.signals.device_rate.load(ORDER), seconds);
        let position = seconds.to_bits();
        self.signals.seek_position.store(position, ORDER);
        self.signals.seek_id.fetch_add(1, ORDER);
        self.signals.position.store(position, ORDER);
        Ok(())
    }

    pub fn set_audition(&self, original: bool, matched: bool) {
        if !original && self.original() {
            if let Some(mix) = self.signals.mix.load_full() {
                mix.prepare(self.signals.device_rate.load(ORDER), self.position());
            }
        }
        self.signals.original.store(original, ORDER);
        self.signals.matched.store(matched, ORDER);
    }

    pub fn playing(&self) -> bool {
        self.signals.playing.load(ORDER)
    }
    pub fn clear(&self) {
        let _ = self.set_playing(false);
        self.signals.mix.store(None);
        self.set_loop(None);
        self.signals.position.store(0, ORDER);
    }
    pub fn set_loop(&self, range: Option<[f64; 2]>) {
        self.signals.loop_range.store(range.map(Arc::new));
    }
    pub fn spectrum(&self) -> (u32, Vec<f32>) {
        let rate = self.signals.device_rate.load(ORDER);
        (
            rate,
            if self.playing() {
                self.signals.scope.spectrum(rate)
            } else {
                vec![]
            },
        )
    }
    pub fn live(&self) -> (f64, f64, f64) {
        (
            f64::from_bits(self.signals.momentary.load(ORDER)),
            f64::from_bits(self.signals.short_term.load(ORDER)),
            f64::from_bits(self.signals.correlation.load(ORDER)),
        )
    }
    pub fn original(&self) -> bool {
        self.signals.original.load(ORDER)
    }
    pub fn matched(&self) -> bool {
        self.signals.matched.load(ORDER)
    }
    pub fn position(&self) -> f64 {
        f64::from_bits(self.signals.position.load(ORDER))
    }
    pub fn peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.signals.peak_left.load(ORDER)),
            f32::from_bits(self.signals.peak_right.load(ORDER)),
        )
    }
    pub fn error(&self) -> Option<String> {
        self.signals.error.lock().unwrap().clone()
    }
}

// The callback keeps only its cursor, filter state, and a short smoothing ramp.
// Audio buffers and filter setup are published by the desktop worker threads.
#[derive(Default)]
struct Cursor {
    track: u64,
    revision: u64,
    seek_id: u64,
    frame: f64,
    timeline_rate: u32,
    scope_cursor: u64,
    original: bool,
    matched: bool,
    rendered: bool,
    was_playing: bool,
    last: [f32; 2],
    fade_from: [f32; 2],
    fade: u32,
    preview: Option<Monitor>,
    meter: Option<LiveMeter>,
    meter_channels: u16,
}

fn stereo(audio: &Audio, frame: f64) -> [f32; 2] {
    let channels = usize::from(audio.channels);
    let base = frame as usize;
    let next = (base + 1).min(audio.frames() - 1);
    let fraction = (frame - base as f64) as f32;
    let sample = |ch: usize| {
        let a = audio.samples[base * channels + ch];
        a + (audio.samples[next * channels + ch] - a) * fraction
    };
    [sample(0), sample((channels - 1).min(1))]
}

impl Cursor {
    fn fill<T: SizedSample + FromSample<f32>>(
        &mut self,
        buffer: &mut [T],
        channels: usize,
        rate: u32,
        signals: &Signals,
    ) {
        let snapshot = signals.mix.load();
        let Some(mix) = snapshot.as_ref() else {
            buffer.fill(T::EQUILIBRIUM);
            return;
        };
        let metered_channels = mix.source.channels.min(channels.min(2) as u16);
        if self.meter_channels != metered_channels {
            self.meter = Some(LiveMeter::new(rate, metered_channels));
            self.meter_channels = metered_channels;
        }
        let playing = signals.playing.load(ORDER);
        let original = signals.original.load(ORDER);
        let matched = signals.matched.load(ORDER);
        let seek_id = signals.seek_id.load(ORDER);
        let changed = mix.id != self.track
            || mix.revision != self.revision
            || self.timeline_rate != mix.source.sample_rate;
        let rendered = mix.master.is_some();
        if changed
            || !rendered
                && (self.preview.is_none() || self.seek_id != seek_id || self.original != original)
        {
            if let Ok(mut pending) = mix.preview.try_lock() {
                if let Some(mut preview) = pending.take() {
                    if self.track == mix.id
                        && self.seek_id == seek_id
                        && !self.original
                        && !self.rendered
                    {
                        if let Some(previous) = &self.preview {
                            preview.tone.inherit_dynamics(&previous.tone);
                        }
                    }
                    self.preview = Some(preview);
                }
            }
        }
        if self.track != mix.id
            || self.seek_id != seek_id
            || self.original != original
            || (!self.was_playing && playing)
        {
            self.scope_cursor = 0;
            signals.scope.end.store(0, Ordering::Release);
            if let Some(meter) = &mut self.meter {
                meter.reset();
            }
        }
        if self.track != mix.id {
            self.frame = 0.0;
        } else if self.timeline_rate != 0 && self.timeline_rate != mix.source.sample_rate {
            self.frame *= mix.source.sample_rate as f64 / self.timeline_rate as f64;
        }
        self.timeline_rate = mix.source.sample_rate;
        if self.seek_id != seek_id {
            self.frame = f64::from_bits(signals.seek_position.load(ORDER))
                * f64::from(mix.source.sample_rate);
        }
        let ramp_frames = (rate / 200).max(1);
        if changed
            || rendered != self.rendered
            || original != self.original
            || matched != self.matched
            || seek_id != self.seek_id
            || playing != self.was_playing
        {
            self.fade = ramp_frames;
            self.fade_from = self.last;
        }
        self.track = mix.id;
        self.revision = mix.revision;
        self.seek_id = seek_id;
        self.original = original;
        self.matched = matched;
        self.rendered = rendered;
        self.was_playing = playing;

        if !playing && self.fade == 0 {
            buffer.fill(T::EQUILIBRIUM);
            signals.peak_left.store(0, ORDER);
            signals.peak_right.store(0, ORDER);
            return;
        }
        let audio = if original {
            &mix.source
        } else {
            mix.master.as_ref().unwrap_or(&mix.source)
        };
        let kernel = if !original && rendered {
            signals.master_resampler.load()
        } else {
            signals.resampler.load()
        };
        let resampler = kernel
            .as_ref()
            .filter(|k| k.rates == (audio.sample_rate, rate));
        let audio_per_source = f64::from(audio.sample_rate) / f64::from(mix.source.sample_rate);
        let gain = if !matched {
            1.0
        } else if original {
            mix.gains.0
        } else {
            mix.gains.1
        };
        let step = f64::from(mix.source.sample_rate) / f64::from(rate);
        let mut peaks = [0.0f32; 2];
        let total = mix.source.frames() as f64;
        let loop_snapshot = signals.loop_range.load();
        let loop_frames = loop_snapshot.as_ref().map(|range| {
            (
                range[0] * mix.source.sample_rate as f64,
                range[1] * mix.source.sample_rate as f64,
            )
        });

        for output in buffer.chunks_mut(channels) {
            if let Some((first, last)) = loop_frames {
                if playing && (self.frame < first || self.frame >= last) {
                    self.frame = if self.frame < first {
                        first
                    } else {
                        first + (self.frame - last) % (last - first)
                    };
                }
            }
            let active = playing && self.frame < total;
            let mut pair = [0.0; 2];
            if active {
                let at = |offset: usize| {
                    let mut frame = self.frame + offset as f64 * step;
                    if let Some((first, last)) = loop_frames {
                        if frame >= last {
                            frame = first + (frame - last) % (last - first);
                        }
                    }
                    if frame >= total {
                        return [0.; 2];
                    }
                    let audio_frame = (frame * audio_per_source).min((audio.frames() - 1) as f64);
                    if let Some(kernel) = resampler {
                        kernel.sample(audio, audio_frame)
                    } else {
                        stereo(audio, audio_frame)
                    }
                };
                pair = if !original && !rendered {
                    self.preview.as_mut().map_or_else(
                        || at(0),
                        |monitor| monitor.next(at, mix.source.channels, mix.normalization),
                    )
                } else {
                    at(0)
                };
                self.frame = (self.frame + step).min(total);
            }
            for (i, sample) in pair.iter_mut().enumerate() {
                *sample *= gain;
                if self.fade > 0 {
                    let blend = self.fade as f32 / ramp_frames as f32;
                    *sample += (self.fade_from[i] - *sample) * blend;
                }
                *sample = sample.clamp(-1.0, 1.0);
                peaks[i] = peaks[i].max(sample.abs());
            }
            if let Some(meter) = self.meter.as_mut() {
                if active {
                    let frame = if channels == 1 {
                        [(pair[0] + pair[1]) * 0.5, 0.]
                    } else {
                        pair
                    };
                    meter.push(&frame[..usize::from(metered_channels)]);
                }
            }
            if active {
                let audible = if channels == 1 {
                    let mono = (pair[0] + pair[1]) * 0.5;
                    [mono, mono]
                } else {
                    pair
                };
                signals.scope.write(self.scope_cursor, audible);
                self.scope_cursor = self.scope_cursor.wrapping_add(1);
            }
            self.last = pair;
            self.fade = self.fade.saturating_sub(1);
            if channels == 1 {
                output[0] = T::from_sample((pair[0] + pair[1]) * 0.5);
            } else {
                output[0] = T::from_sample(pair[0]);
                output[1] = T::from_sample(pair[1]);
                output[2..].fill(T::EQUILIBRIUM);
            }
        }
        if signals.seek_id.load(ORDER) == seek_id {
            signals.position.store(
                (self.frame / f64::from(mix.source.sample_rate)).to_bits(),
                ORDER,
            );
            if playing && self.frame >= total && loop_frames.is_none() {
                signals.playing.store(false, ORDER);
            }
        }
        signals
            .scope
            .end
            .store(self.scope_cursor, Ordering::Release);
        if let Some(meter) = &self.meter {
            let live = meter.values();
            signals.momentary.store(live.momentary.to_bits(), ORDER);
            signals.short_term.store(live.short_term.to_bits(), ORDER);
            signals.correlation.store(live.correlation.to_bits(), ORDER);
        }
        signals.peak_left.store(peaks[0].to_bits(), ORDER);
        signals.peak_right.store(peaks[1].to_bits(), ORDER);
    }
}

fn build_stream(signals: Arc<Signals>) -> Result<Stream, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("Connect or select an audio output device, then press Play.")?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Cannot read the output device configuration: {e}"))?;
    let config = supported.config();
    signals.device_rate.store(config.sample_rate.0, ORDER);
    if let Some(mix) = signals.mix.load_full() {
        signals.configure_resamplers(&mix, config.sample_rate.0);
        mix.prepare(
            config.sample_rate.0,
            f64::from_bits(signals.position.load(ORDER)),
        );
    }
    macro_rules! stream {
        ($t:ty) => {
            typed_stream::<$t>(&device, &config, signals)
        };
    }
    match supported.sample_format() {
        SampleFormat::F32 => stream!(f32),
        SampleFormat::I16 => stream!(i16),
        SampleFormat::U16 => stream!(u16),
        SampleFormat::I32 => stream!(i32),
        SampleFormat::U32 => stream!(u32),
        SampleFormat::F64 => stream!(f64),
        SampleFormat::I8 => stream!(i8),
        SampleFormat::U8 => stream!(u8),
        SampleFormat::I64 => stream!(i64),
        SampleFormat::U64 => stream!(u64),
        other => Err(format!("Unsupported audio device sample format: {other}")),
    }
}

fn typed_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    signals: Arc<Signals>,
) -> Result<Stream, String> {
    let mut cursor = Cursor::default();
    let rate = config.sample_rate.0;
    let channels = usize::from(config.channels);
    let error_signals = signals.clone();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| cursor.fill(data, channels, rate, &signals),
            move |error| {
                error_signals.playing.store(false, ORDER);
                error_signals.device_lost.store(true, ORDER);
                *error_signals.error.lock().unwrap() =
                    Some(format!("Playback device error: {error}"));
            },
            None,
        )
        .map_err(|e| format!("Cannot open the audio output: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matched_audition_switch_keeps_the_playhead_and_applies_the_correct_gain() {
        let signals = Signals::default();
        let audio = |a| {
            Arc::new(Audio {
                sample_rate: 8_000,
                channels: 1,
                samples: vec![a; 8000],
            })
        };
        signals.mix.store(Some(Arc::new(PlaybackMix::new(
            1,
            1,
            audio(0.2),
            Some(audio(0.8)),
            (1.0, 0.25),
            0.,
            Settings::default(),
        ))));
        signals.playing.store(true, ORDER);
        signals.matched.store(true, ORDER);
        let mut cursor = Cursor::default();
        let mut out = [0f32; 320];
        cursor.fill(&mut out, 2, 8_000, &signals);
        assert!((out[319] - 0.2).abs() < 1e-6);
        assert_eq!(cursor.frame, 160.);
        signals.original.store(true, ORDER);
        cursor.fill(&mut out, 2, 8_000, &signals);
        assert!((out[319] - 0.2).abs() < 1e-6);
        assert_eq!(cursor.frame, 320.);
        signals.original.store(false, ORDER);
        signals.matched.store(false, ORDER);
        cursor.fill(&mut out, 2, 8_000, &signals);
        assert!((out[319] - 0.8).abs() < 1e-6);
        assert_eq!(cursor.frame, 480.);
        signals.seek_position.store(0.99f64.to_bits(), ORDER);
        signals.seek_id.fetch_add(1, ORDER);
        cursor.fill(&mut out, 2, 8_000, &signals);
        assert_eq!(cursor.frame, 8000.);
        assert_eq!(out[319], 0.);
        assert!(!signals.playing.load(ORDER));
    }
    #[test]
    fn transport_uses_source_seconds_for_a_different_master_rate_and_loops_inside_each_callback() {
        let signals = Signals::default();
        let source = Arc::new(Audio {
            sample_rate: 8000,
            channels: 1,
            samples: vec![0.; 8000],
        });
        let master = Arc::new(Audio {
            sample_rate: 16000,
            channels: 1,
            samples: (0..16000).map(|i| i as f32 / 16000.).collect(),
        });
        signals.mix.store(Some(Arc::new(PlaybackMix::new(
            1,
            1,
            source,
            Some(master),
            (1., 1.),
            0.,
            Settings::default(),
        ))));
        signals.seek_position.store(0.5f64.to_bits(), ORDER);
        signals.seek_id.store(1, ORDER);
        signals.playing.store(true, ORDER);
        let mut cursor = Cursor::default();
        let mut output = [0f32; 800];
        cursor.fill(&mut output, 1, 8000, &signals);
        assert!((output[799] - 0.599875).abs() < 0.00001);
        signals.loop_range.store(Some(Arc::new([0.5, 0.52])));
        cursor.fill(&mut output, 1, 8000, &signals);
        assert!(output.iter().all(|v| (0.5..=0.52).contains(v)));
        assert!(signals.playing.load(ORDER));
    }
    #[test]
    fn pending_preview_limits_real_peaks_without_moving_an_impulse() {
        let rate = 48000;
        let mut source = vec![0.; 4800];
        source[500] = 0.8;
        let audio = Arc::new(Audio {
            sample_rate: rate,
            channels: 1,
            samples: source,
        });
        let settings = Settings {
            clean_low_end: false,
            normalize: false,
            ceiling_db: -6.,
            ..Settings::default()
        };
        let mix = PlaybackMix::new(1, 1, audio, None, (1., 1.), 12., settings);
        mix.prepare(rate, 0.);
        let signals = Signals::default();
        signals.mix.store(Some(Arc::new(mix)));
        signals.playing.store(true, ORDER);
        let mut cursor = Cursor::default();
        let mut buffer = vec![0.; 1600];
        cursor.fill(&mut buffer, 1, rate, &signals);
        let largest = buffer
            .iter()
            .enumerate()
            .max_by(|a, b| f32::total_cmp(a.1, b.1))
            .unwrap();
        assert_eq!(largest.0, 500);
        assert!(*largest.1 > 0.2 && *largest.1 < 0.502);
    }
    #[test]
    fn mono_source_meter_counts_one_channel_on_a_stereo_device() {
        let rate = 8000;
        let audio = Arc::new(Audio {
            sample_rate: rate,
            channels: 1,
            samples: (0..rate)
                .map(|i| (std::f64::consts::TAU * 500. * i as f64 / rate as f64).sin() as f32 * 0.2)
                .collect(),
        });
        let expected = master_core::measure(&audio).lufs;
        let sig = Signals::default();
        sig.mix.store(Some(Arc::new(PlaybackMix::new(
            1,
            1,
            audio.clone(),
            Some(audio),
            (1., 1.),
            0.,
            Settings::default(),
        ))));
        sig.playing.store(true, ORDER);
        let mut cursor = Cursor::default();
        cursor.fill(&mut vec![0f32; rate as usize * 2], 2, rate, &sig);
        let measured = f64::from_bits(sig.momentary.load(ORDER));
        assert!(
            (measured - expected).abs() < 0.1,
            "live={measured}, file={expected}"
        );
    }
}
