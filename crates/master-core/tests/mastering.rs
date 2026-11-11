use master_core::{
    audition_gains, decode_wav, measure, render, render_cancellable, render_with_gain, waveform,
    write_wav, Audio, Settings,
};
use std::f64::consts::TAU;

const DEMO: &[u8] = include_bytes!("../../../assets/Half Light.wav");

#[test]
fn warm_glue_matches_swift_reference() {
    // The Swift app's working-copy DSP rendered its own Half Light demo with
    // Warm Glue, 44.1 kHz / 24-bit. Its converter and our polyphase FIR differ.
    let source = decode_wav(DEMO).unwrap();
    let settings = Settings::preset("Warm Glue").unwrap();
    let source_metrics = measure(&source);
    let (master, gain_db) = render_with_gain(&source, &settings, || false).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Half Light - mastered.wav");
    write_wav(&path, &master).unwrap();
    let reopened = decode_wav(&std::fs::read(&path).unwrap()).unwrap();
    let metrics = measure(&reopened);
    assert_eq!(
        (reopened.sample_rate, reopened.channels, reopened.frames()),
        (44100, 2, 819280)
    );
    assert!((source_metrics.lufs - -16.705180).abs() < 0.002);
    // The source contains intersample overs; each converter uses its own low-pass response.
    assert!(source_metrics.true_peak_db >= -2.50 && source_metrics.true_peak_db < 0.0);
    assert!((metrics.lufs - -14.015401).abs() < 0.06);
    assert!((metrics.true_peak_db - -1.149997).abs() < 0.09);
    assert!((gain_db - 5.190149).abs() < 0.04);
    for (i, left, right) in [
        (500, -0.7242961, -0.78673434),
        (4789, 0.08326618, 0.018176338),
        (11025, 0.11666198, 0.14030895),
        (44100, 0.1462286, 0.15747736),
        (185186, 0.08857284, 0.054181766),
        (302200, -0.02571167, -0.010997927),
        (592836, -0.16263989, -0.19571893),
        (770005, 0.06593254, 0.0415525),
    ] {
        assert!(
            (reopened.samples[i * 2] - left).abs() < 0.006,
            "left at {i}: {}",
            reopened.samples[i * 2]
        );
        assert!(
            (reopened.samples[i * 2 + 1] - right).abs() < 0.006,
            "right at {i}: {}",
            reopened.samples[i * 2 + 1]
        );
    }
    let (a, b) = audition_gains(source_metrics, metrics);
    assert!(
        (source_metrics.lufs + 20. * (a as f64).log10() - metrics.lufs - 20. * (b as f64).log10())
            .abs()
            < 0.001
    );
    println!(
        "Warm Glue export: {:.6} LUFS, {:.6} dBTP, {:.6} dB gain",
        metrics.lufs, metrics.true_peak_db, gain_db
    );
}

#[test]
fn limiter_catches_peaks_between_samples() {
    // Quarter-rate sine at a 45-degree phase shift: samples miss the true peaks by 3 dB.
    let sample_rate = 48000;
    let source = Audio {
        sample_rate,
        channels: 1,
        samples: (0..sample_rate)
            .map(|i| ((i as f64 / 4. + 0.125) * TAU).sin() as f32)
            .collect(),
    };
    let mut settings = Settings {
        clean_low_end: false,
        normalize: false,
        ceiling_db: -2.,
        ..Settings::default()
    };
    let sample_peak = source.samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    assert!(20. * sample_peak.log10() < -2.9);
    assert!(measure(&source).true_peak_db > -0.1);
    let master = render(&source, &settings).unwrap();
    let true_peak = measure(&master).true_peak_db;
    assert!(true_peak <= -2. && true_peak > -2.5, "{true_peak}");
    settings.limit = false;
    assert_eq!(render(&source, &settings).unwrap().samples, source.samples);
}

#[test]
fn silence_and_short_audio_are_not_boosted() {
    let silent = Audio {
        sample_rate: 44100,
        channels: 2,
        samples: vec![0.; 44100],
    };
    let result = render(&silent, &Settings::default()).unwrap();
    assert_eq!(result.samples, silent.samples);
    assert!(measure(&result).lufs.is_infinite());
    assert_eq!(waveform(&result, 100), vec![0.; 100]);
    assert!(render_cancellable(&silent, &Settings::default(), || true).is_err());
}

#[test]
fn wav_preserves_mono_odd_length_and_atomic_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("track.wav");
    std::fs::write(&path, b"previous").unwrap();
    let mono = Audio {
        sample_rate: 96000,
        channels: 1,
        samples: vec![-1., 0., 0.612345],
    };
    write_wav(&path, &mono).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let restored = decode_wav(&bytes).unwrap();
    assert_eq!(restored.frames(), 3);
    assert_eq!(restored.sample_rate, 96000);
    assert!((restored.samples[2] - mono.samples[2]).abs() < 1.2e-7);
    assert!(decode_wav(&bytes[..bytes.len() - 2]).is_err());
    let broken = Audio {
        samples: vec![f32::NAN],
        ..mono
    };
    assert!(write_wav(&path, &broken).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
