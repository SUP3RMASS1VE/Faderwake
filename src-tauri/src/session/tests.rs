use super::*;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn fixture(root: &TempDir, file: &str, rate: u32, freq: f64) -> PathBuf {
    let audio = Audio {
        channels: 1,
        sample_rate: rate,
        samples: (0..rate)
            .map(|i| (std::f64::consts::TAU * freq * i as f64 / rate as f64).sin() as f32 * 0.1)
            .collect(),
    };
    let path = root.path().join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    master_core::write_wav(&path, &audio).unwrap();
    path
}
fn wait_master(studio: &Studio) {
    let end = Instant::now() + Duration::from_secs(20);
    while studio.status().rendering {
        assert!(Instant::now() < end, "render timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn polish_controls_reach_the_monitored_master_and_export() {
    let tmp = TempDir::new().unwrap();
    let rate = 44100;
    let mut samples = Vec::new();
    for i in 0..rate / 2 {
        let phase = std::f64::consts::TAU * i as f64 / rate as f64;
        let mid = [18., 250., 4500., 14500.]
            .iter()
            .map(|hz| (hz * phase).sin() * 0.08)
            .sum::<f64>();
        let side = (70. * phase).sin() * 0.08;
        samples.extend([(mid + side) as f32, (mid - side) as f32]);
    }
    let path = tmp.path().join("polish-input.wav");
    let input = Audio {
        samples,
        sample_rate: rate,
        channels: 2,
    };
    master_core::write_wav(&path, &input).unwrap();
    let studio = Studio::new();
    let id = studio.add(vec![path], false).unwrap().id;
    let dry = studio.audio(id, true).unwrap();
    let flat = Settings {
        clean_low_end: false,
        normalize: false,
        limit: false,
        ..Settings::default()
    };
    for key in [
        "cleanLowEnd",
        "centerBass",
        "cutMud",
        "tameHarsh",
        "addAir",
        "smoothHighs",
    ] {
        let mut payload = serde_json::to_value(&flat).unwrap();
        payload[key] = serde_json::Value::Bool(true);
        let settings: Settings = serde_json::from_value(payload).unwrap();
        studio
            .update_settings(id, settings.clone(), None, None, None, None)
            .unwrap();
        // Capture the export immediately, including the path used before a new
        // monitored master is ready. Each iteration replaces the previous chain.
        let plan = studio.plan_export(false).unwrap();
        let dest = tmp.path().join(format!("{key}.wav"));
        studio.export_to(plan, dest.clone(), false).unwrap();
        wait_master(&studio);
        let expected = master_core::render(&dry, &settings).unwrap();
        let monitored = studio.audio(id, false).unwrap();
        let exported = master_core::decode_wav(&std::fs::read(dest).unwrap()).unwrap();
        assert!(
            expected
                .samples
                .iter()
                .zip(&dry.samples)
                .any(|(a, b)| (a - b).abs() > 0.001),
            "{key} left the dedicated test signal unchanged"
        );
        assert_eq!(monitored.samples, expected.samples, "{key} master");
        assert_eq!(exported.samples.len(), expected.samples.len());
        assert!(
            exported
                .samples
                .iter()
                .zip(&expected.samples)
                .all(|(a, b)| (a - b).abs() < 0.000001),
            "{key} exported different samples"
        );
    }
}
#[test]
fn export_uses_latest_gesture_preserves_source_and_groups_undo() {
    let tmp = TempDir::new().unwrap();
    let source = fixture(&tmp, "input.wav", 44100, 250.);
    let studio = Studio::new();
    let item = studio.add(vec![source.clone()], false).unwrap();
    let mut edited = Settings::preset("Warm Glue").unwrap();
    edited.normalize = false;
    edited.bands[3].gain = 2.;
    studio
        .update_settings(
            item.id,
            edited.clone(),
            Some("Warm Glue".into()),
            Some(2),
            Some(5),
            None,
        )
        .unwrap();
    edited.bands[3].gain = 8.;
    studio
        .update_settings(item.id, edited.clone(), None, Some(3), Some(5), None)
        .unwrap();
    studio
        .update_settings(item.id, Settings::default(), None, Some(1), None, None)
        .unwrap();
    assert_eq!(
        studio.history(false).unwrap().unwrap().settings.bands[3].gain,
        0.
    );
    assert_eq!(
        studio.history(true).unwrap().unwrap().settings.bands[3].gain,
        8.
    );
    wait_master(&studio);
    let plan = studio.plan_export(false).unwrap();
    assert!(studio.export_to(plan, source.clone(), false).is_err());
    let target = tmp.path().join("output.wav");
    studio
        .export_to(studio.plan_export(false).unwrap(), target.clone(), false)
        .unwrap();
    let decoded = master_core::decode_wav(&std::fs::read(source).unwrap()).unwrap();
    let expected = master_core::render(&decoded, &edited).unwrap();
    let encoded = master_core::decode_wav(&std::fs::read(target).unwrap()).unwrap();
    let error = expected
        .samples
        .iter()
        .zip(encoded.samples)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(error < 0.000001, "max sample error {error}");
}

#[test]
fn queue_edit_clipboard_selection_and_removal_round_trip() {
    let tmp = TempDir::new().unwrap();
    let first = fixture(&tmp, "mono.wav", 44100, 250.);
    let second = fixture(&tmp, "other.wav", 48000, 630.);
    let studio = Studio::new();
    let item = studio
        .add(vec![first.clone(), second, first], false)
        .unwrap();
    assert_eq!(studio.queue().len(), 2);
    studio
        .set_region(item.id, Some([0.25, 0.5]), false, true, None)
        .unwrap();
    let cut = studio.edit(item.id, "cut", 1., 0.).unwrap();
    assert!((cut.duration - 0.75).abs() < 1e-6);
    assert!(cut.edited && cut.can_undo && cut.can_paste);
    let other_id = studio.queue()[1].id;
    studio.select(other_id).unwrap();
    let pasted = studio.edit(other_id, "paste", 1., 0.).unwrap();
    assert!((pasted.duration - 1.25).abs() < 1e-6);
    assert_eq!(pasted.selection, Some([0., 0.25]));
    let undo = studio.edit(other_id, "undo", 1., 0.).unwrap();
    assert_eq!(undo.duration, 1.);
    assert!(!undo.edited);
    let redo = studio.edit(other_id, "redo", 1., 0.).unwrap();
    assert_eq!(redo.duration, 1.25);
    studio.remove(item.id).unwrap();
    assert_eq!(studio.queue().len(), 1);
    let restored = studio.history(false).unwrap().unwrap();
    assert_eq!(restored.id, item.id);
    assert_eq!(restored.duration, 0.75);
    assert_eq!(studio.edit(item.id, "reset", 1., 0.).unwrap().duration, 1.);
}

#[test]
fn batch_keeps_duplicate_names_and_writes_tagged_files_at_selected_rates() {
    let tmp = TempDir::new().unwrap();
    let first = fixture(&tmp, "one/song.wav", 44100, 260.);
    let second = fixture(&tmp, "two/song.wav", 32000, 550.);
    let out = tmp.path().join("result");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("song_mastered.wav"), b"existing").unwrap();
    let studio = Studio::new();
    let item = studio.add(vec![first, second], false).unwrap();
    let mut output = item.output.clone();
    output.sample_rate = 48000;
    output.bit_depth = 16;
    studio.set_output(item.id, output).unwrap();
    studio.apply_all(item.id, "settings").unwrap();
    let tags = Tags {
        artist: "Studio tests".into(),
        title: "Batch".into(),
        ..Tags::default()
    };
    studio.set_tags(item.id, tags.clone()).unwrap();
    studio.apply_all(item.id, "tags").unwrap();
    let plan = studio.plan_export(true).unwrap();
    studio.export_to(plan, out.clone(), true).unwrap();
    assert_eq!(
        std::fs::read(out.join("song_mastered.wav")).unwrap(),
        b"existing"
    );
    for filename in ["song_mastered_2.wav", "song_mastered_3.wav"] {
        let (audio, export_tags) = media::load(&out.join(filename)).unwrap();
        assert_eq!(audio.sample_rate, 48000);
        assert_eq!(audio.frames(), 48000);
        assert_eq!(export_tags.artist, tags.artist);
    }
    assert!(studio.queue().iter().all(|e| e.export_status == "Exported"));
    assert_eq!(studio.status().export_progress, 1.);
}

#[test]
fn changing_output_rate_updates_the_audition_master_and_undo_restores_it() {
    let tmp = TempDir::new().unwrap();
    let source = fixture(&tmp, "rate.wav", 48000, 16000.);
    let studio = Studio::new();
    let item = studio.add(vec![source], false).unwrap();
    wait_master(&studio);
    assert_eq!(studio.audio(item.id, false).unwrap().sample_rate, 44100);
    let mut changed = item.output.clone();
    changed.sample_rate = 96000;
    studio.set_output(item.id, changed).unwrap();
    wait_master(&studio);
    let monitor = studio.audio(item.id, false).unwrap();
    assert_eq!(monitor.sample_rate, 96000);
    assert_eq!(monitor.frames(), 96000);
    let exported = tmp.path().join("rate-master.wav");
    studio
        .export_to(studio.plan_export(false).unwrap(), exported.clone(), false)
        .unwrap();
    let final_audio = master_core::decode_wav(&std::fs::read(exported).unwrap()).unwrap();
    assert!(final_audio
        .samples
        .iter()
        .zip(&monitor.samples)
        .all(|(a, b)| (a - b).abs() < 0.000001));
    studio.history(false).unwrap();
    wait_master(&studio);
    assert_eq!(studio.audio(item.id, false).unwrap().sample_rate, 44100);
}

#[test]
fn appending_keeps_playhead_and_selection_but_open_demo_selects_the_demo() {
    let dir = TempDir::new().unwrap();
    let a = fixture(&dir, "a.wav", 44100, 440.);
    let b = fixture(&dir, "b.wav", 48000, 550.);
    let demo = fixture(&dir, "demo.wav", 48000, 660.);
    let studio = Studio::new();
    let first = studio.add(vec![a.clone(), b.clone()], false).unwrap();
    assert_eq!(first.name, "a");
    studio.transport.seek(0.25).unwrap();
    let keep = studio.add(vec![b, a], false).unwrap();
    assert_eq!(keep.id, first.id);
    assert_eq!(studio.transport.position(), 0.25);
    let selected = studio.add(vec![demo.clone()], true).unwrap();
    assert_eq!(selected.name, "demo");
    assert_eq!(selected.preset, "Warm Glue");
    assert_eq!(studio.queue().len(), 3);
    let custom = Settings::preset("Club Loud").unwrap();
    studio
        .update_settings(
            selected.id,
            custom,
            Some("Club Loud".into()),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(studio.add(vec![demo], true).unwrap().preset, "Club Loud");
}

#[test]
fn bypass_gain_matches_the_resampled_original_when_ultrasonic_energy_is_removed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ultrasonic.wav");
    let rate = 96000;
    let audio = Audio {
        channels: 1,
        sample_rate: rate,
        samples: (0..rate)
            .map(|i| {
                let t = i as f64 / rate as f64;
                (0.1 * ((std::f64::consts::TAU * 1000. * t).sin()
                    + (std::f64::consts::TAU * 24000. * t).sin())) as f32
            })
            .collect(),
    };
    master_core::write_wav(&path, &audio).unwrap();
    let studio = Studio::new();
    studio.add(vec![path], false).unwrap();
    wait_master(&studio);
    let ws = studio.workspace.lock().unwrap();
    let preview = ws.preview.as_ref().unwrap();
    let (reference, m) = preview.reference.as_ref().unwrap();
    assert_eq!(reference.sample_rate, 44100);
    assert!(preview.metrics.lufs - m.lufs > 1.);
    let wet =
        preview.master.as_ref().unwrap().metrics.lufs + 20. * f64::from(preview.gains.1).log10();
    let dry = m.lufs + 20. * f64::from(preview.gains.0).log10();
    assert!((dry - wet).abs() < 0.1);
}

#[test]
fn retry_recovers_a_missing_queued_file_and_retains_a_prior_export_error() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("later.wav");
    let studio = Studio::new();
    assert!(studio.add(vec![path], false).is_err());
    let row = studio.queue().pop().unwrap();
    assert_eq!(row.export_status, "Error");
    assert!(row.error.is_some());
    {
        let mut ws = studio.workspace.lock().unwrap();
        let e = ws.entry_mut(row.id).unwrap();
        e.export_error = Some("Encoder failed.".into());
        e.export_status = "Error".into();
    }
    fixture(&tmp, "later.wav", 44100, 250.);
    studio.select(row.id).unwrap();
    let row = studio.queue().pop().unwrap();
    assert_eq!(row.error.as_deref(), Some("Encoder failed."));
}
