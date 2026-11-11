use super::*;
use std::collections::HashSet;

struct ExportItem {
    id: u64,
    path: PathBuf,
    setup: Setup,
    audio: Option<Arc<Audio>>,
    tags: Option<Tags>,
    ready: Option<Arc<Audio>>,
}
pub struct ExportPlan {
    items: Vec<ExportItem>,
    sources: HashSet<PathBuf>,
}
impl ExportPlan {
    pub fn extension(&self) -> &'static str {
        self.items[0].setup.output.extension()
    }
    pub fn suggested_name(&self) -> String {
        format!(
            "{}_mastered.{}",
            self.items[0]
                .path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy(),
            self.extension()
        )
    }

    fn target(item: &ExportItem, choice: &Path, batch: bool) -> Result<PathBuf, String> {
        let ext = item.setup.output.extension();
        if batch {
            let base = item.path.file_stem().unwrap_or_default().to_string_lossy();
            let mut dest = choice.join(format!("{base}_mastered.{ext}"));
            let mut n = 2;
            while dest.exists() {
                dest = choice.join(format!("{base}_mastered_{n}.{ext}"));
                n += 1;
            }
            return Ok(dest);
        }
        if choice.extension().is_none() {
            let dest = choice.with_extension(ext);
            if dest.exists() {
                return Err("Select the existing file in the save dialog to replace it.".into());
            }
            Ok(dest)
        } else if choice
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            Ok(choice.to_owned())
        } else {
            Err(format!("Choose a .{ext} filename for this export."))
        }
    }
}
impl ExportItem {
    fn write(
        &self,
        target: &Path,
        overwrite: bool,
        cancelled: impl Fn() -> bool + Copy,
    ) -> Result<(), String> {
        let output = &self.setup.output;
        if let Some(rendered) = &self.ready {
            return media::encode_cancellable(
                target,
                rendered,
                self.tags.as_ref().unwrap_or(&Tags::default()),
                output,
                overwrite,
                cancelled,
            );
        }
        let (source, tags) = match &self.audio {
            Some(edited) => (edited.clone(), self.tags.clone().unwrap_or_default()),
            None => {
                let (audio, original) = media::load_cancellable(&self.path, cancelled)?;
                (Arc::new(audio), self.tags.clone().unwrap_or(original))
            }
        };
        if cancelled() {
            return Err("Export cancelled.".into());
        }
        let converted;
        let input = if source.sample_rate == output.sample_rate {
            &*source
        } else {
            converted = master_core::resample_cancellable(&source, output.sample_rate, cancelled)?;
            &converted
        };
        let master = master_core::render_cancellable(input, &self.setup.settings, cancelled)?;
        media::encode_cancellable(target, &master, &tags, output, overwrite, cancelled)
    }
}
impl Studio {
    pub fn plan_export(&self, all: bool) -> Result<ExportPlan, String> {
        let _edit = self
            .edit_gate
            .try_lock()
            .map_err(|_| "Wait for the current audio edit to finish before exporting.")?;
        let ws = self.workspace.lock().unwrap();
        if ws.exporting {
            return Err("An export is already running.".into());
        }
        let mut items = vec![];
        for e in &ws.entries {
            let active = ws.preview.as_ref().filter(|p| p.id == e.id);
            if !all && active.is_none() {
                continue;
            }
            e.setup.output.validate()?;
            let ready = active
                .and_then(|p| p.master.as_ref())
                .filter(|m| m.audio.sample_rate == e.setup.output.sample_rate)
                .map(|m| m.audio.clone());
            items.push(ExportItem {
                id: e.id,
                path: e.path.clone(),
                setup: e.setup.clone(),
                audio: active
                    .map(|p| p.source.clone())
                    .or_else(|| e.edited.clone()),
                tags: e.effective_tags().cloned(),
                ready,
            });
        }
        if items.is_empty() {
            return Err("Add and select an audio file first.".into());
        }
        Ok(ExportPlan {
            items,
            sources: ws.entries.iter().map(|e| e.path.clone()).collect(),
        })
    }
    pub fn export_to(
        &self,
        plan: ExportPlan,
        choice: PathBuf,
        batch: bool,
    ) -> Result<String, String> {
        {
            let mut ws = self.workspace.lock().unwrap();
            if ws.exporting {
                return Err("An export is already running.".into());
            }
            self.cancel.store(false, Ordering::Release);
            ws.exporting = true;
            ws.progress = 0.;
            for item in &plan.items {
                if let Ok(e) = ws.entry_mut(item.id) {
                    e.export_status = "Queued".into();
                    e.export_error = None;
                }
            }
        }
        let cancelled = || self.cancel.load(Ordering::Acquire);
        let count = plan.items.len();
        let mut successes = vec![];
        let mut errors = vec![];
        for (index, item) in plan.items.iter().enumerate() {
            if cancelled() {
                break;
            }
            if let Ok(e) = self.workspace.lock().unwrap().entry_mut(item.id) {
                e.export_status = "Exporting".into();
            }
            let result = (|| {
                let dest = ExportPlan::target(item, &choice, batch)?;
                if dest.canonicalize().is_ok_and(|p| plan.sources.contains(&p)) {
                    return Err(
                        "Choose a different filename to preserve the original audio.".into(),
                    );
                }
                item.write(&dest, !batch, cancelled)?;
                Ok::<_, String>(dest)
            })();
            let mut ws = self.workspace.lock().unwrap();
            ws.progress = (index + 1) as f64 / count as f64;
            match result {
                Ok(path) => {
                    if let Ok(e) = ws.entry_mut(item.id) {
                        e.export_status = "Exported".into();
                    }
                    ws.last_export = Some(path.clone());
                    successes.push(path);
                }
                Err(error) => {
                    if let Ok(e) = ws.entry_mut(item.id) {
                        e.export_status = if cancelled() { "Cancelled" } else { "Error" }.into();
                        e.export_error = Some(error.clone());
                    }
                    errors.push(format!(
                        "{}: {error}",
                        item.path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
        let mut ws = self.workspace.lock().unwrap();
        ws.exporting = false;
        if cancelled() {
            for e in &mut ws.entries {
                if e.export_status == "Queued" {
                    e.export_status = "Cancelled".into();
                }
            }
            return Ok(format!(
                "Export cancelled. {} of {count} files saved.",
                successes.len()
            ));
        }
        if !errors.is_empty() {
            return Err(format!(
                "{} of {count} files exported. {}",
                successes.len(),
                errors.join("\n")
            ));
        }
        if batch {
            Ok(format!(
                "Exported {count} file{} to {}",
                if count == 1 { "" } else { "s" },
                choice.display()
            ))
        } else {
            Ok(successes[0].to_string_lossy().into_owned())
        }
    }
    pub fn cancel_export(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
