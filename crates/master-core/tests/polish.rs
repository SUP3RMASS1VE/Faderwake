use master_core::{render, Audio, Settings};
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const MID: [f32; 2] = [1., 1.];
const SIDE: [f32; 2] = [1., -1.];

fn bypass() -> Settings {
    Settings {
        clean_low_end: false,
        normalize: false,
        limit: false,
        eq_bypassed: true,
        ..Settings::default()
    }
}

fn gain_change(settings: &Settings, hz: f64, peak: f64, stereo: [f32; 2]) -> f64 {
    let source = Audio {
        channels: 2,
        sample_rate: RATE,
        samples: (0..RATE)
            .flat_map(|i| {
                let value = (peak * (TAU * hz * f64::from(i) / f64::from(RATE)).sin()) as f32;
                stereo.map(|polarity| polarity * value)
            })
            .collect(),
    };
    let off = render(&source, &bypass()).unwrap();
    let on = render(&source, settings).unwrap();
    assert_eq!(off.samples, source.samples);

    // Give filters and envelopes half a second to settle before measuring power.
    let power = |audio: &Audio| -> f64 {
        audio.samples[audio.samples.len() / 2..]
            .iter()
            .map(|&x| f64::from(x).powi(2))
            .sum()
    };
    10. * (power(&on) / power(&off)).log10()
}

#[test]
fn tonal_polish_switches_change_the_target_band_and_preserve_distant_tones() {
    let cases = [
        (
            Settings {
                clean_low_end: true,
                ..bypass()
            },
            [(15., -12.31), (30., -3.01), (1000., 0.)],
        ),
        (
            Settings {
                cut_mud: true,
                ..bypass()
            },
            [(250., -3.), (1000., -0.09), (4000., 0.)],
        ),
        (
            Settings {
                tame_harsh: true,
                ..bypass()
            },
            [(250., 0.), (4000., -2.53), (6000., -1.97)],
        ),
        (
            Settings {
                add_air: true,
                ..bypass()
            },
            [(1000., 0.), (12000., 1.25), (18000., 2.43)],
        ),
    ];
    for (settings, probes) in cases {
        for (hz, expected) in probes {
            let measured = gain_change(&settings, hz, 0.1, MID);
            assert!((measured - expected).abs() < 0.08, "{hz} Hz: {measured} dB");
        }
    }
}

#[test]
fn centering_reduces_only_the_low_frequency_side_component() {
    let center = Settings {
        center_bass: true,
        ..bypass()
    };
    assert!(gain_change(&center, 40., 0.1, MID).abs() < 0.001);
    assert!((gain_change(&center, 40., 0.1, SIDE) + 19.16).abs() < 0.08);
    assert!(gain_change(&center, 1000., 0.1, SIDE).abs() < 0.01);
}

#[test]
fn de_shimmer_responds_to_level_and_reduces_stereo_highs() {
    let smooth = Settings {
        smooth_highs: true,
        ..bypass()
    };
    assert!(gain_change(&smooth, 1000., 0.1, MID).abs() < 0.01);
    let quiet = gain_change(&smooth, 15000., 0.003, MID);
    let loud = gain_change(&smooth, 15000., 0.25, MID);
    assert!((-2.1..-1.8).contains(&quiet), "quiet 15 kHz: {quiet} dB");
    assert!((-4.7..-4.2).contains(&loud), "loud 15 kHz: {loud} dB");
    let stereo_highs = gain_change(&smooth, 18000., 0.25, SIDE);
    assert!(
        (-13.5..-12.9).contains(&stereo_highs),
        "18 kHz Side: {stereo_highs} dB"
    );
}
