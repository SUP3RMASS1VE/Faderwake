mod edit;
mod export;
mod history;
#[cfg(test)]
mod tests;

use crate::media::{self, Output, Tags};
use crate::transport::{PlaybackMix, Transport};
use master_core::{
    audition_gains, measure_cancellable, render_with_gain, waveform, Audio, Band, Metrics, Settings,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Setup {
    settings: Settings,
    output: Output,
    preset: String,
}
impl Default for Setup {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            output: Output::default(),
            preset: "Balanced".into(),
        }
    }
}

#[derive(Clone)]
struct EditState {
    audio: Arc<Audio>,
    edited: bool,
    selection: Option<[f64; 2]>,
    position: f64,
}

struct Entry {
    id: u64,
    path: PathBuf,
    setup: Setup,
    tags: Option<Tags>,
    original_tags: Option<Tags>,
    edited: Option<Arc<Audio>>,
    undo: Vec<EditState>,
    redo: Vec<EditState>,
    edit_id: u64,
    export_status: String,
    load_error: Option<String>,
    export_error: Option<String>,
}

impl Entry {
    fn effective_tags(&self) -> Option<&Tags> {
        self.tags.as_ref().or(self.original_tags.as_ref())
    }
    fn name(&self) -> String {
        self.path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: u64,
    name: String,
    file_name: String,
    duration: f64,
    sample_rate: u32,
    channels: u16,
    peaks: Vec<f32>,
    bands: [Band; 10],
    preset: String,
    settings: Settings,
    output: Output,
    tags: Tags,
    selection: Option<[f64; 2]>,
    can_undo: bool,
    can_redo: bool,
    can_paste: bool,
    edited: bool,
    original_lufs: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueRow {
    pub(crate) id: u64,
    settings: Settings,
    name: String,
    file_name: String,
    selected: bool,
    preset: String,
    output: Output,
    edited: bool,
    export_status: String,
    error: Option<String>,
}

struct Master {
    audio: Arc<Audio>,
    metrics: Metrics,
    peaks: Vec<f32>,
}
struct Preview {
    id: u64,
    // The undecorated monitor input at the output rate, shared across setting edits.
    reference: Option<(Arc<Audio>, Metrics)>,
    source: Arc<Audio>,
    metrics: Metrics,
    peaks: Vec<f32>,
    master: Option<Master>,
    revision: u64,
    normalization_db: f64,
    gains: (f32, f32),
    error: Option<String>,
    selection: Option<[f64; 2]>,
    looping: bool,
}
impl Preview {
    fn publish(&self, transport: &Transport, settings: Settings) {
        transport.publish(PlaybackMix::new(
            self.id,
            self.revision,
            self.reference
                .as_ref()
                .map(|(audio, _)| audio)
                .unwrap_or(&self.source)
                .clone(),
            self.master.as_ref().map(|m| m.audio.clone()),
            self.gains,
            self.normalization_db,
            settings,
        ));
    }
}

#[derive(Default)]
struct Workspace {
    entries: Vec<Entry>,
    preview: Option<Preview>,
    clipboard: Option<Arc<Audio>>,
    defaults: Setup,
    prefs_path: Option<PathBuf>,
    undo: Vec<history::Change>,
    redo: Vec<history::Change>,
    gesture: Option<(u64, u64)>,
    exporting: bool,
    progress: f64,
    last_export: Option<PathBuf>,
}

impl Workspace {
    fn entry(&self, id: u64) -> Result<&Entry, String> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| "This track is no longer in the queue.".into())
    }
    fn entry_mut(&mut self, id: u64) -> Result<&mut Entry, String> {
        self.entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| "This track is no longer in the queue.".into())
    }
    fn active(&self, id: u64) -> Result<&Preview, String> {
        self.preview
            .as_ref()
            .filter(|p| p.id == id)
            .ok_or_else(|| "This track is no longer selected.".into())
    }
    fn track(&self) -> Option<Track> {
        let p = self.preview.as_ref()?;
        let e = self.entry(p.id).ok()?;
        Some(Track {
            id: e.id,
            name: e.name(),
            file_name: e
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            duration: p.source.duration(),
            sample_rate: p.source.sample_rate,
            channels: p.source.channels,
            peaks: p.peaks.clone(),
            bands: e.setup.settings.bands,
            preset: e.setup.preset.clone(),
            settings: e.setup.settings.clone(),
            output: e.setup.output.clone(),
            tags: e.effective_tags().cloned().unwrap_or_default(),
            selection: p.selection,
            can_undo: !e.undo.is_empty(),
            can_redo: !e.redo.is_empty(),
            edited: e.edited.is_some(),
            can_paste: self.clipboard.is_some(),
            original_lufs: finite(p.metrics.lufs),
        })
    }
}
fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}
struct Job {
    reference: Option<(Arc<Audio>, Metrics)>,
    source_metrics: Metrics,
    output_rate: u32,
    revision: u64,
    audio: Arc<Audio>,
    settings: Settings,
}
#[derive(Clone)]
pub struct Studio {
    workspace: Arc<Mutex<Workspace>>,
    revision: Arc<AtomicU64>,
    selection: Arc<AtomicU64>,
    next_id: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    edit_gate: Arc<Mutex<()>>,
    jobs: mpsc::Sender<Job>,
    pub transport: Transport,
}

impl Studio {
    pub fn new() -> Self {
        let workspace: Arc<Mutex<Workspace>> = Arc::default();
        let revision: Arc<AtomicU64> = Arc::default();
        let transport = Transport::new();
        let (jobs, incoming) = mpsc::channel::<Job>();
        let ws = workspace.clone();
        let serial = revision.clone();
        let player = transport.clone();
        thread::spawn(move || {
            while let Ok(first) = incoming.recv() {
                let job = incoming.try_iter().last().unwrap_or(first);
                let cancelled = || serial.load(Ordering::Acquire) != job.revision;
                let render = || {
                    let (reference, reference_metrics) = if let Some(cached) = &job.reference {
                        cached.clone()
                    } else if job.audio.sample_rate == job.output_rate {
                        (job.audio.clone(), job.source_metrics)
                    } else {
                        let audio = Arc::new(master_core::resample_cancellable(
                            &job.audio,
                            job.output_rate,
                            cancelled,
                        )?);
                        let metrics = measure_cancellable(&audio, cancelled)?;
                        (audio, metrics)
                    };
                    if job.reference.is_none() {
                        let mut state = ws.lock().unwrap();
                        let Some(p) = state
                            .preview
                            .as_mut()
                            .filter(|p| p.revision == job.revision)
                        else {
                            return Err("Render superseded.".into());
                        };
                        p.reference = Some((reference.clone(), reference_metrics));
                        if reference.sample_rate != p.source.sample_rate {
                            p.publish(&player, job.settings.clone());
                        }
                    }
                    render_with_gain(&reference, &job.settings, cancelled)
                };
                let result = render().and_then(|(audio, gain)| {
                    let metrics = measure_cancellable(&audio, cancelled)?;
                    let peaks = waveform(&audio, 1600);
                    Ok((
                        Master {
                            audio: Arc::new(audio),
                            metrics,
                            peaks,
                        },
                        gain,
                    ))
                });
                let mut ws = ws.lock().unwrap();
                let Some(p) = ws.preview.as_mut().filter(|p| p.revision == job.revision) else {
                    continue;
                };
                match result {
                    Ok((master, gain)) => {
                        p.gains = audition_gains(
                            p.reference.as_ref().map_or(p.metrics, |r| r.1),
                            master.metrics,
                        );
                        p.normalization_db = gain;
                        p.master = Some(master);
                        p.publish(&player, job.settings);
                    }
                    Err(e) => p.error = Some(e),
                }
            }
        });
        Self {
            workspace,
            revision,
            selection: Arc::default(),
            next_id: Arc::default(),
            cancel: Arc::default(),
            edit_gate: Arc::default(),
            jobs,
            transport,
        }
    }

    pub fn restore_defaults(&self, path: PathBuf) {
        let mut ws = self.workspace.lock().unwrap();
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(setup) = serde_json::from_slice::<Setup>(&bytes) {
                if setup.settings.validate().is_ok() && setup.output.validate().is_ok() {
                    ws.defaults = setup;
                }
            }
        }
        ws.prefs_path = Some(path);
    }
    fn save_defaults(ws: &Workspace) -> Result<(), String> {
        let Some(path) = &ws.prefs_path else {
            return Ok(());
        };
        let parent = path.parent().ok_or("Invalid preferences folder.")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer(file.as_file_mut(), &ws.defaults).map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn add(&self, paths: Vec<PathBuf>, demo: bool) -> Result<Track, String> {
        let first = {
            let mut ws = self.workspace.lock().unwrap();
            let mut first = None;
            for path in paths {
                if path.is_dir() {
                    continue;
                }
                let path = path.canonicalize().unwrap_or(path);
                let id = if let Some(e) = ws.entries.iter().find(|e| e.path == path) {
                    e.id
                } else {
                    let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
                    let setup = if demo {
                        Setup {
                            settings: Settings::preset("Warm Glue")?,
                            preset: "Warm Glue".into(),
                            ..Setup::default()
                        }
                    } else {
                        ws.defaults.clone()
                    };
                    ws.entries.push(Entry {
                        id,
                        path,
                        setup,
                        tags: None,
                        original_tags: None,
                        edited: None,
                        undo: vec![],
                        redo: vec![],
                        edit_id: 0,
                        export_status: "Ready".into(),
                        load_error: None,
                        export_error: None,
                    });
                    id
                };
                first.get_or_insert(id);
            }
            let first = first.ok_or("No audio files were selected.")?;
            if !demo {
                if let Some(current) = ws.track() {
                    return Ok(current);
                }
            }
            first
        };
        self.select(first)
    }

    pub fn select(&self, id: u64) -> Result<Track, String> {
        let (ticket, path, edited) = {
            let ws = self.workspace.lock().unwrap();
            let ticket = self.selection.fetch_add(1, Ordering::AcqRel) + 1;
            if ws.preview.as_ref().is_some_and(|p| p.id == id) {
                return ws.track().ok_or("No audio is selected.".into());
            }
            let e = ws.entry(id)?;
            (ticket, e.path.clone(), e.edited.clone())
        };
        let cancelled = || self.selection.load(Ordering::Acquire) != ticket;
        let result = (|| {
            let (audio, tags) = if let Some(audio) = edited {
                (audio, None)
            } else {
                let (audio, tags) = media::load_cancellable(&path, cancelled)?;
                (Arc::new(audio), Some(tags))
            };
            let metrics = measure_cancellable(&audio, cancelled)?;
            let peaks = waveform(&audio, 1600);
            Ok((audio, tags, metrics, peaks))
        })();
        let mut ws = self.workspace.lock().unwrap();
        if cancelled() {
            return Err("Another track was selected.".into());
        }
        let e = ws.entry_mut(id)?;
        let (audio, tags, metrics, peaks) =
            result.inspect_err(|error: &String| e.load_error = Some(error.clone()))?;
        if let Some(tags) = tags {
            e.original_tags = Some(tags);
        }
        e.load_error = None;
        let setup = e.setup.clone();
        self.transport.set_playing(false)?;
        self.transport.set_loop(None);
        let gain_db = if setup.settings.normalize && metrics.lufs.is_finite() && metrics.lufs > -70.
        {
            setup.settings.target_lufs - metrics.lufs
        } else {
            0.
        };
        let gain = 10f32.powf(gain_db as f32 / 20.);
        ws.preview = Some(Preview {
            reference: None,
            id,
            source: audio,
            metrics,
            peaks,
            master: None,
            revision: 0,
            normalization_db: gain_db,
            gains: (gain.min(1.), gain.recip().min(1.)),
            error: None,
            selection: None,
            looping: false,
        });
        self.schedule(&mut ws, setup.settings)?;
        self.transport.seek(0.)?;
        ws.track().ok_or("No audio is selected.".into())
    }

    fn schedule(&self, ws: &mut Workspace, settings: Settings) -> Result<(), String> {
        let id = ws.preview.as_ref().ok_or("No audio is selected.")?.id;
        let output_rate = ws.entry(id)?.setup.output.sample_rate;
        let p = ws.preview.as_mut().unwrap();
        if p.reference
            .as_ref()
            .is_some_and(|r| r.0.sample_rate != output_rate)
        {
            p.reference = None;
        }
        p.revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        p.error = None;
        p.master = None;
        p.publish(&self.transport, settings.clone());
        self.jobs
            .send(Job {
                reference: p.reference.clone(),
                source_metrics: p.metrics,
                output_rate,
                revision: p.revision,
                audio: p.source.clone(),
                settings,
            })
            .map_err(|_| "The mastering worker stopped.".into())
    }
    pub fn track(&self) -> Option<Track> {
        self.workspace.lock().unwrap().track()
    }
    pub fn queue(&self) -> Vec<QueueRow> {
        let ws = self.workspace.lock().unwrap();
        ws.entries
            .iter()
            .map(|e| QueueRow {
                id: e.id,
                settings: e.setup.settings.clone(),
                name: e.name(),
                file_name: e
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                selected: ws.preview.as_ref().is_some_and(|p| p.id == e.id),
                preset: e.setup.preset.clone(),
                output: e.setup.output.clone(),
                edited: e.edited.is_some(),
                export_status: if e.load_error.is_some() && e.export_status != "Exporting" {
                    "Error".into()
                } else {
                    e.export_status.clone()
                },
                error: e.load_error.clone().or_else(|| e.export_error.clone()),
            })
            .collect()
    }
    pub fn path(&self, id: u64) -> Result<PathBuf, String> {
        Ok(self.workspace.lock().unwrap().entry(id)?.path.clone())
    }
    pub fn last_export(&self) -> Result<PathBuf, String> {
        self.workspace
            .lock()
            .unwrap()
            .last_export
            .clone()
            .ok_or("Export a track first.".into())
    }
    fn audio(&self, id: u64, original: bool) -> Result<Arc<Audio>, String> {
        let ws = self.workspace.lock().unwrap();
        let p = ws.active(id)?;
        Ok(if !original {
            p.master
                .as_ref()
                .map(|m| m.audio.clone())
                .unwrap_or_else(|| p.source.clone())
        } else {
            p.source.clone()
        })
    }
    pub fn overview(&self, id: u64, original: bool) -> Result<Vec<f32>, String> {
        let ws = self.workspace.lock().unwrap();
        let p = ws.active(id)?;
        Ok(if !original {
            p.master.as_ref().map(|m| &m.peaks).unwrap_or(&p.peaks)
        } else {
            &p.peaks
        }
        .clone())
    }

    pub fn spectrogram(&self, id: u64, original: bool) -> Result<master_core::Spectrogram, String> {
        let serial = self.revision.load(Ordering::Acquire);
        let audio = self.audio(id, original)?;
        master_core::spectrogram(&audio, || self.revision.load(Ordering::Acquire) != serial)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    track_id: u64,
    playing: bool,
    position: f64,
    original: bool,
    matched: bool,
    rendering: bool,
    render_revision: u64,
    peak_left: f32,
    peak_right: f32,
    original_lufs: Option<f64>,
    original_true_peak: Option<f64>,
    original_range: Option<f64>,
    original_crest: Option<f64>,
    original_rms: Option<f64>,
    master_lufs: Option<f64>,
    master_true_peak: Option<f64>,
    master_range: Option<f64>,
    master_crest: Option<f64>,
    master_rms: Option<f64>,
    correlation: Option<f64>,
    mid_db: Option<f64>,
    side_db: Option<f64>,
    spectrum: Vec<f32>,
    spectrum_sample_rate: u32,
    momentary_lufs: Option<f64>,
    short_term_lufs: Option<f64>,
    exporting: bool,
    export_progress: f64,
    last_export: Option<String>,
    can_undo: bool,
    can_redo: bool,
    can_paste: bool,
    edited: bool,
    can_undo_workspace: bool,
    can_redo_workspace: bool,
    error: Option<String>,
}

impl Studio {
    pub fn status(&self) -> Status {
        let ws = self.workspace.lock().unwrap();
        let preview = ws.preview.as_ref();
        let orig = preview.map(|p| p.metrics);
        let master = preview.and_then(|p| p.master.as_ref().map(|m| m.metrics));
        let entry = preview.and_then(|p| ws.entry(p.id).ok());
        let original = self.transport.original();
        let current = if original { orig } else { master.or(orig) };
        let (peak_left, peak_right) = self.transport.peaks();
        let (momentary, short_term, correlation) = self.transport.live();
        let position = self.transport.position();
        let playing = self.transport.playing();
        let mut status = Status {
            track_id: preview.map_or(0, |p| p.id),
            playing,
            position,
            original,
            matched: self.transport.matched(),
            rendering: preview.is_some_and(|p| p.master.is_none() && p.error.is_none()),
            render_revision: preview.map_or(0, |p| p.revision),
            peak_left,
            peak_right,
            original_lufs: orig.and_then(|m| finite(m.lufs)),
            original_true_peak: orig.and_then(|m| finite(m.true_peak_db)),
            original_range: orig.and_then(|m| finite(m.range)),
            original_crest: orig.and_then(|m| finite(m.crest_db)),
            original_rms: orig.and_then(|m| finite(m.rms_db)),
            master_lufs: master.and_then(|m| finite(m.lufs)),
            master_true_peak: master.and_then(|m| finite(m.true_peak_db)),
            master_range: master.and_then(|m| finite(m.range)),
            master_crest: master.and_then(|m| finite(m.crest_db)),
            master_rms: master.and_then(|m| finite(m.rms_db)),
            correlation: if playing {
                finite(correlation)
            } else {
                current.and_then(|m| finite(m.correlation))
            },
            mid_db: current.and_then(|m| finite(m.mid_db)),
            side_db: current.and_then(|m| finite(m.side_db)),
            spectrum: vec![],
            spectrum_sample_rate: 0,
            momentary_lufs: if playing { finite(momentary) } else { None },
            short_term_lufs: if playing { finite(short_term) } else { None },
            exporting: ws.exporting,
            export_progress: ws.progress,
            last_export: ws
                .last_export
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            can_undo: entry.is_some_and(|e| !e.undo.is_empty()),
            can_redo: entry.is_some_and(|e| !e.redo.is_empty()),
            edited: entry.is_some_and(|e| e.edited.is_some()),
            can_paste: ws.clipboard.is_some(),
            can_undo_workspace: !ws.undo.is_empty(),
            can_redo_workspace: !ws.redo.is_empty(),
            error: preview
                .and_then(|p| p.error.clone())
                .or_else(|| self.transport.error()),
        };
        drop(ws);
        (status.spectrum_sample_rate, status.spectrum) = self.transport.spectrum();
        status
    }
}
