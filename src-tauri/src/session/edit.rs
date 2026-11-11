use super::*;
use master_core::{apply_edit, channel_peaks, copy_region, snap_frame, Edit};
use std::ops::Range;

fn frame(audio: &Audio, seconds: f64) -> usize {
    ((seconds * audio.sample_rate as f64).round() as usize).min(audio.frames())
}
fn range(audio: &Audio, region: Option<[f64; 2]>) -> Option<Range<usize>> {
    region
        .map(|[start, end]| frame(audio, start)..frame(audio, end))
        .filter(|r| !r.is_empty())
}
fn remember(stack: &mut Vec<EditState>, state: EditState) {
    stack.push(state);
    let mut bytes: usize = stack
        .iter()
        .map(|s| s.audio.samples.len().saturating_mul(4))
        .sum();
    while stack.len() > 1 && (stack.len() > 30 || bytes > 256_000_000) {
        bytes -= stack.remove(0).audio.samples.len() * 4;
    }
}
impl Studio {
    pub fn set_region(
        &self,
        id: u64,
        selection: Option<[f64; 2]>,
        snap: bool,
        looping: bool,
        snap_edge: Option<&str>,
    ) -> Result<Option<[f64; 2]>, String> {
        let mut ws = self.workspace.lock().unwrap();
        let p = ws
            .preview
            .as_mut()
            .filter(|p| p.id == id)
            .ok_or("Select the track first.")?;
        if selection.is_some_and(|pair| pair.iter().any(|t| !t.is_finite())) {
            return Err("Invalid selection.".into());
        }
        let selected = range(&p.source, selection).and_then(|r| {
            let a = if snap && snap_edge != Some("right") {
                snap_frame(&p.source, r.start)
            } else {
                r.start
            };
            let b = if snap && snap_edge != Some("left") {
                snap_frame(&p.source, r.end)
            } else {
                r.end
            };
            (a < b).then(|| {
                [
                    a as f64 / p.source.sample_rate as f64,
                    b as f64 / p.source.sample_rate as f64,
                ]
            })
        });
        p.selection = selected;
        p.looping = looping;
        self.transport
            .set_loop(if looping { selected } else { None });
        Ok(selected)
    }
    pub fn editor_waveform(
        &self,
        id: u64,
        start: f64,
        end: f64,
        bins: usize,
        original: bool,
    ) -> Result<Vec<Vec<[f32; 2]>>, String> {
        if !start.is_finite() || !end.is_finite() || end <= start || bins == 0 || bins > 8192 {
            return Err("Invalid waveform window.".into());
        }
        let audio = self.audio(id, original)?;
        let r = frame(&audio, start)..frame(&audio, end);
        Ok(channel_peaks(&audio, r, bins))
    }
    pub fn edit(&self, id: u64, operation: &str, fade: f64, gain: f64) -> Result<Track, String> {
        let _edit = self
            .edit_gate
            .try_lock()
            .map_err(|_| "Wait for the current audio edit to finish.")?;
        let generation = self.selection.load(Ordering::Acquire);
        if !(0.0..=60.0).contains(&fade) || !(-24.0..=24.0).contains(&gain) {
            return Err("Check the fade length or gain.".into());
        }
        let (old, clipboard, history, path) = {
            let ws = self.workspace.lock().unwrap();
            let p = ws.active(id)?;
            let e = ws.entry(id)?;
            let old = EditState {
                audio: p.source.clone(),
                edited: e.edited.is_some(),
                selection: p.selection,
                position: self.transport.position(),
            };
            let history = match operation {
                "undo" => Some(e.undo.last().cloned().ok_or("No audio edits to undo.")?),
                "redo" => Some(e.redo.last().cloned().ok_or("No audio edits to redo.")?),
                _ => None,
            };
            (old, ws.clipboard.clone(), history, e.path.clone())
        };
        let selected = range(&old.audio, old.selection);
        if operation == "copy" {
            let clip = copy_region(
                &old.audio,
                selected.ok_or("Select part of the waveform first.")?,
            )?;
            let mut ws = self.workspace.lock().unwrap();
            ws.active(id)?;
            ws.clipboard = Some(Arc::new(clip));
            return ws.track().ok_or("Open a track first.".into());
        }
        let mut copied = None;
        let next = if let Some(history) = history {
            history
        } else if operation == "reset" {
            let (audio, _) = media::load_cancellable(&path, || {
                self.selection.load(Ordering::Acquire) != generation
            })?;
            EditState {
                audio: Arc::new(audio),
                edited: false,
                selection: None,
                position: 0.,
            }
        } else {
            let action = match operation {
                "cut" => Edit::Cut,
                "trim" => Edit::Trim,
                "fadeIn" => Edit::FadeIn,
                "fadeOut" => Edit::FadeOut,
                "silence" => Edit::Silence,
                "normalize" => Edit::Normalize,
                "gain" => Edit::Gain,
                "reverse" => Edit::Reverse,
                "paste" => Edit::Paste,
                _ => return Err(format!("Unknown audio edit: {operation}")),
            };
            let cursor = frame(&old.audio, old.position);
            let selected = if operation == "paste" {
                Some(selected.unwrap_or(cursor..cursor))
            } else {
                selected
            };
            let audio = apply_edit(
                &old.audio,
                action,
                selected.clone(),
                fade,
                gain,
                clipboard.as_deref(),
            )?;
            let (selection, position) = match operation {
                "cut" => {
                    let r = selected.as_ref().ok_or("Select audio to cut.")?;
                    copied = Some(Arc::new(copy_region(&old.audio, r.clone())?));
                    (None, r.start as f64 / old.audio.sample_rate as f64)
                }
                "trim" => (None, 0.),
                "paste" => {
                    let r = selected.as_ref().unwrap();
                    let start = r.start as f64 / old.audio.sample_rate as f64;
                    let added = audio.frames() - (old.audio.frames() - r.len());
                    (
                        Some([start, start + added as f64 / old.audio.sample_rate as f64]),
                        start,
                    )
                }
                _ => (old.selection, old.position),
            };
            EditState {
                audio: Arc::new(audio),
                edited: true,
                selection,
                position,
            }
        };
        let metrics = measure_cancellable(&next.audio, || {
            self.selection.load(Ordering::Acquire) != generation
        })?;
        let peaks = waveform(&next.audio, 1600);
        let mut ws = self.workspace.lock().unwrap();
        if !Arc::ptr_eq(&ws.active(id)?.source, &old.audio) {
            return Err("The audio changed during this edit.".into());
        }
        let e = ws.entry_mut(id)?;
        match operation {
            "undo" => {
                e.undo.pop();
                remember(&mut e.redo, old);
            }
            "redo" => {
                e.redo.pop();
                remember(&mut e.undo, old);
            }
            _ => {
                remember(&mut e.undo, old);
                e.redo.clear();
            }
        }
        e.edited = next.edited.then(|| next.audio.clone());
        e.export_status = "Ready".into();
        let settings = e.setup.settings.clone();
        if let Some(copied) = copied {
            ws.clipboard = Some(copied);
        }
        let p = ws.preview.as_mut().unwrap();
        p.source = next.audio;
        p.reference = None;
        p.selection = next.selection;
        p.metrics = metrics;
        p.peaks = peaks;
        self.transport.set_playing(false)?;
        self.transport
            .set_loop(if p.looping { p.selection } else { None });
        self.schedule(&mut ws, settings)?;
        self.transport.seek(next.position)?;
        ws.track().ok_or("Open a track first.".into())
    }
}
