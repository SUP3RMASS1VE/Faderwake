use master_core::{
    apply_edit, channel_peaks, copy_region, decode_wav, measure, render, resample, snap_frame,
    spectrogram, spectrum, Audio, Compressor, DeEsser, Edit, LiveMeter, Settings,
};
use std::f64::consts::TAU;

fn mono(samples: Vec<f32>, rate: u32) -> Audio {
    Audio {
        channels: 1,
        sample_rate: rate,
        samples,
    }
}

#[test]
fn adjustable_dynamics_matches_swift_demo_samples() {
    let source = decode_wav(include_bytes!("../../../assets/Half Light.wav")).unwrap();
    let settings = Settings {
        eq_bypassed: true,
        smooth_highs: true,
        de_esser: Some(DeEsser {
            frequency: 7200.,
            threshold: -32.,
            reduction: 9.,
        }),
        compressor: Compressor {
            threshold: -21.,
            ratio: 4.5,
            attack: 4.,
            release: 84.,
            makeup: 2.,
        },
        normalize: false,
        limit: false,
        ..Settings::preset("Warm Glue").unwrap()
    };
    let master = render(&source, &settings).unwrap();
    let metrics = measure(&master);
    assert!((metrics.lufs + 20.8984548).abs() < 0.002);
    assert!((metrics.range - 0.3748345).abs() < 0.002);
    for (frame, expected) in [
        (500, [-0.24520417, -0.26629737]),
        (11025, [0.028367262, 0.0348421]),
        (185186, [0.050688624, 0.0363703]),
        (770005, [0.043638192, 0.029932281]),
    ] {
        for (channel, reference) in expected.into_iter().enumerate() {
            assert!((master.samples[frame * 2 + channel] - reference).abs() < 1e-5);
        }
    }
}

#[test]
fn edits_keep_channel_pairs_and_selection_boundaries() {
    let original = Audio {
        sample_rate: 44_100,
        channels: 2,
        samples: vec![0., 0.9, 0.3, 0.6, -0.2, 0.4, -0.6, 0.1],
    };
    let clip = copy_region(&original, 1..3).unwrap();
    let result = |edit| {
        apply_edit(&original, edit, Some(1..3), 1., 6., None)
            .unwrap()
            .samples
    };
    assert_eq!(result(Edit::Trim), [0.3, 0.6, -0.2, 0.4]);
    assert_eq!(result(Edit::Cut), [0., 0.9, -0.6, 0.1]);
    assert_eq!(
        result(Edit::Reverse),
        [0., 0.9, -0.2, 0.4, 0.3, 0.6, -0.6, 0.1]
    );
    assert_eq!(result(Edit::Silence), [0., 0.9, 0., 0., 0., 0., -0.6, 0.1]);
    assert_eq!(
        result(Edit::FadeIn),
        [0., 0.9, 0., 0., -0.2, 0.4, -0.6, 0.1]
    );
    assert_eq!(
        result(Edit::FadeOut),
        [0., 0.9, 0.3, 0.6, 0., 0., -0.6, 0.1]
    );
    assert!((result(Edit::Normalize)[3] - 0.9660509).abs() < 1e-5);
    assert!((result(Edit::Gain)[2] - 0.5985787).abs() < 1e-5);
    assert_eq!(
        apply_edit(&original, Edit::Paste, Some(2..2), 1., 0., Some(&clip))
            .unwrap()
            .frames(),
        6
    );
    assert_eq!(
        apply_edit(&original, Edit::Paste, Some(1..3), 1., 0., Some(&clip))
            .unwrap()
            .samples,
        original.samples
    );
    assert!(apply_edit(&original, Edit::Cut, Some(0..4), 1., 0., None).is_err());
    assert!(apply_edit(&original, Edit::Silence, None, 1., 0., None).is_err());
    assert_eq!(snap_frame(&original, 3), 2);
    assert_eq!(
        channel_peaks(&original, 0..4, 2),
        vec![vec![[0., 0.3], [-0.6, -0.2]], vec![[0.6, 0.9], [0.1, 0.4]]]
    );
}

#[test]
fn cross_track_paste_changes_rate_and_channel_layout() {
    let source = mono(vec![0.1; 4410], 44100);
    let stereo = Audio {
        sample_rate: 48000,
        channels: 2,
        samples: [0.2, -0.2].repeat(4800),
    };
    let paste = apply_edit(&source, Edit::Paste, Some(0..0), 1., 0., Some(&stereo)).unwrap();
    assert_eq!(paste.frames(), 8820);
    assert!(paste.samples[..4410].iter().all(|v| v.abs() < 1e-7));
    let paste = apply_edit(&stereo, Edit::Paste, Some(0..4800), 1., 0., Some(&source)).unwrap();
    assert_eq!(paste.frames(), 4800);
    assert!(paste.samples.as_chunks::<2>().0[100..4700]
        .iter()
        .all(|[l, r]| (l - 0.1).abs() < 1e-5 && l == r));
    assert!(resample(&source, 0).is_err());
}

#[test]
fn spectral_and_phase_analysis_follow_the_signal() {
    let rate = 48_000;
    let audio = Audio {
        channels: 2,
        sample_rate: rate,
        samples: (0..rate)
            .flat_map(|i| {
                let value = (TAU * 1000. * i as f64 / rate as f64).sin() as f32 * 0.25;
                [value, -value]
            })
            .collect(),
    };
    let frequency = spectrum(&audio, 20_000, 64);
    let max = frequency
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .unwrap()
        .0;
    assert!((35..=37).contains(&max), "1000 Hz bin = {max}");
    let image = spectrogram(&audio, || false).unwrap();
    assert_eq!(
        (image.width, image.height, image.levels.len()),
        (24, 80, 24 * 80)
    );
    let row = (0..80)
        .max_by_key(|row| image.levels[row * image.width + 10])
        .unwrap();
    assert!((36..=40).contains(&row), "1000 Hz row = {row}");
    assert!(spectrogram(&audio, || true).is_err());
    let report = measure(&audio);
    assert!((report.correlation + 1.).abs() < 1e-5);
    assert!(report.mid_db.is_infinite());
    assert!((report.crest_db - 3.0103).abs() < 0.001);
    let mut live = LiveMeter::new(rate, 2);
    for frame in audio.samples.as_chunks::<2>().0 {
        live.push(frame);
    }
    let values = live.values();
    assert!(values.correlation < -0.9);
    assert!((report.lufs - values.momentary).abs() < 0.04);
    assert!((report.lufs - values.short_term).abs() < 0.04);
}

#[test]
fn live_limiter_preserves_timeline_stereo_and_intersample_ceiling() {
    let rate = 48_000;
    let mut limiter = master_core::LiveLimiter::new(rate, -2.);
    let latency = limiter.latency();
    let frames = 12_000;
    let mut output = Vec::with_capacity(frames * 2);
    for i in 0..frames + latency {
        let v = if i < frames {
            ((i as f64 / 4. + 0.125) * TAU).sin() as f32 * 1.15
        } else {
            0.
        };
        let frame = limiter.push([v, -v]);
        if i >= latency {
            output.extend_from_slice(&frame);
        } else {
            assert_eq!(frame, [0.; 2]);
        }
    }
    let metrics = measure(&Audio {
        sample_rate: rate,
        channels: 2,
        samples: output,
    });
    assert!(
        metrics.true_peak_db <= -2.0 && metrics.true_peak_db > -2.6,
        "{}",
        metrics.true_peak_db
    );
    assert!((metrics.correlation + 1.).abs() < 1e-5);
    limiter.reset();
    let mut response = Vec::new();
    for i in 0..=latency + 4 {
        response.push(limiter.push(if i == 0 { [0.01, -0.01] } else { [0.; 2] }));
    }
    assert_eq!(response[latency], [0.01, -0.01]);
}

#[test]
fn changing_monitor_target_preserves_compressor_and_resonant_filter_state() {
    let mut settings = Settings::preset("Warm Glue").unwrap();
    settings.compressor.release = 950.;
    settings.bands[5].gain = 18.;
    settings.bands[5].q = 10.;
    settings.de_esser = Some(DeEsser::default());
    settings.smooth_highs = true;
    let mut original = master_core::ToneChain::new(&settings, 44100, 2);
    for i in 0..6000 {
        let v = (i as f64 * TAU * 1000. / 44100.).sin() as f32 * 0.9;
        original.process_frame(&mut [v, -v * 0.6]);
    }
    settings.target_lufs = -12.;
    let mut replacement = master_core::ToneChain::new(&settings, 44100, 2);
    replacement.inherit_dynamics(&original);
    for i in 0..512 {
        let v = (i as f64 * TAU * 1000. / 44100.).sin() as f32 * 0.2;
        let mut a = [v, -v * 0.6];
        let mut b = a;
        original.process_frame(&mut a);
        replacement.process_frame(&mut b);
        assert_eq!(a, b);
    }
}
