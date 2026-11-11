use crate::media::{self, Output};
use master_core::{copy_region, measure, render, Settings};
use std::path::Path;

/// Exercise the actual bundled demo and codecs without an audio device or a display.
pub fn run(resources: &Path) -> Result<(), String> {
    let (original, mut tags) = media::load(&resources.join("assets/Half Light.wav"))?;
    let master = render(&original, &Settings::preset("Warm Glue")?)?;
    let metrics = measure(&master);
    if (metrics.lufs + 14.0154).abs() > 0.03 || (metrics.true_peak_db + 1.15).abs() > 0.03 {
        return Err(format!("Unexpected demo render: {metrics:?}"));
    }
    let clip = copy_region(&master, 0..master.sample_rate as usize)?;
    tags.artist = "Faderwake self-test".into();
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    for format in ["wav", "flac", "alac", "aac"] {
        let output = Output {
            format: format.into(),
            sample_rate: clip.sample_rate,
            bit_depth: 24,
        };
        let path = directory
            .path()
            .join(format!("check-{format}.{}", output.extension()));
        media::encode_cancellable(&path, &clip, &tags, &output, false, || false)?;
        let (reopened, read_tags) = media::load(&path)?;
        if reopened.sample_rate != clip.sample_rate
            || reopened.channels != clip.channels
            || read_tags != tags
        {
            return Err(format!("Failed {format} round trip"));
        }
        if format == "aac" {
            if reopened.frames().abs_diff(clip.frames()) > 1024 {
                return Err("Unexpected AAC duration".into());
            }
        } else if reopened.frames() != clip.frames()
            || reopened
                .samples
                .iter()
                .zip(&clip.samples)
                .any(|(a, b)| (a - b).abs() > 1.0e-6)
        {
            return Err(format!("Lossless sample check failed for {format}"));
        }
    }
    println!("Faderwake self-test passed. Warm Glue: {:.4} LUFS, {:.2} dBTP. WAV, FLAC, ALAC and AAC round trips passed.", metrics.lufs, metrics.true_peak_db);
    Ok(())
}
