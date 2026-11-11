use super::*;

// Estimate the audible change near the playhead until the full file has been measured.
// Both chains receive identical preceding audio to settle their filters before comparison.
fn provisional_gain(
    audio: &Audio,
    position: f64,
    prior_gain: f64,
    old: &Settings,
    new: &Settings,
) -> f64 {
    if !new.normalize {
        return 0.;
    }
    let rate = audio.sample_rate;
    let channels = audio.channels as usize;
    let at = ((position * rate as f64).round() as usize).min(audio.frames().saturating_sub(1024));
    let start = at.saturating_sub(rate as usize / 10);
    let end = (at + 1024).min(audio.frames());
    let mut old_tone = master_core::ToneChain::new(old, rate, audio.channels);
    let mut new_tone = master_core::ToneChain::new(new, rate, audio.channels);
    let mut energy = [0.; 2];
    for i in start..end {
        let mut a = [0f32; 2];
        a[..channels].copy_from_slice(&audio.samples[i * channels..(i + 1) * channels]);
        let mut b = a;
        if old.normalize {
            old_tone.process_frame(&mut a[..channels]);
        }
        new_tone.process_frame(&mut b[..channels]);
        if i >= at {
            energy[0] += a.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>();
            energy[1] += b.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>();
        }
    }
    let delta = if energy[0] > 1e-10 && energy[1] > 1e-10 {
        (10. * (energy[0] / energy[1]).log10()).clamp(-24., 24.)
    } else {
        0.
    };
    prior_gain
        + delta
        + if old.normalize {
            new.target_lufs - old.target_lufs
        } else {
            0.
        }
}

pub(super) enum Change {
    Setups(Vec<(u64, Setup)>, Option<Box<Setup>>),
    Tags(Vec<(u64, Option<Tags>)>),
    Restore(usize, Box<Entry>),
    Remove(u64),
}

impl Workspace {
    fn remember(&mut self, change: Change) {
        self.undo.push(change);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
}

impl Studio {
    pub fn update_settings(
        &self,
        id: u64,
        settings: Settings,
        preset: Option<String>,
        edit_id: Option<u64>,
        gesture: Option<u64>,
        output: Option<Output>,
    ) -> Result<(), String> {
        settings.validate()?;
        if let Some(output) = &output {
            output.validate()?;
        }
        let mut ws = self.workspace.lock().unwrap();
        let e = ws.entry_mut(id)?;
        if let Some(edit) = edit_id {
            if edit <= e.edit_id {
                return Ok(());
            }
            e.edit_id = edit;
        }
        let previous = e.setup.clone();
        e.setup.settings = settings.clone();
        if let Some(output) = output {
            e.setup.output = output;
        }
        if let Some(name) = preset {
            e.setup.preset = name;
        }
        e.export_status = "Ready".into();
        let key = gesture.map(|g| (id, g));
        if key.is_none() || ws.gesture != key {
            ws.remember(Change::Setups(vec![(id, previous.clone())], None));
        }
        ws.gesture = key;
        if let Some(p) = ws.preview.as_mut().filter(|p| p.id == id) {
            let base = if !previous.settings.normalize
                && p.metrics.lufs.is_finite()
                && p.metrics.lufs > -70.
            {
                settings.target_lufs - p.metrics.lufs
            } else {
                p.normalization_db
            };
            p.normalization_db = provisional_gain(
                &p.source,
                self.transport.position(),
                base,
                &previous.settings,
                &settings,
            );
            self.schedule(&mut ws, settings)?;
        }
        Ok(())
    }
    pub fn set_output(&self, id: u64, output: Output) -> Result<(), String> {
        output.validate()?;
        let mut ws = self.workspace.lock().unwrap();
        let e = ws.entry_mut(id)?;
        if e.setup.output == output {
            return Ok(());
        }
        let old = e.setup.clone();
        let changed_rate = e.setup.output.sample_rate != output.sample_rate;
        let settings = e.setup.settings.clone();
        e.setup.output = output;
        e.setup.preset = "Custom".into();
        e.export_status = "Ready".into();
        ws.gesture = None;
        ws.remember(Change::Setups(vec![(id, old)], None));
        if changed_rate && ws.preview.as_ref().is_some_and(|p| p.id == id) {
            self.schedule(&mut ws, settings)?;
        }
        Ok(())
    }
    pub fn set_tags(&self, id: u64, tags: Tags) -> Result<(), String> {
        let mut ws = self.workspace.lock().unwrap();
        let e = ws.entry_mut(id)?;
        let old = e.tags.replace(tags);
        e.export_status = "Ready".into();
        ws.gesture = None;
        ws.remember(Change::Tags(vec![(id, old)]));
        Ok(())
    }
    pub fn apply_all(&self, id: u64, what: &str) -> Result<(), String> {
        let mut ws = self.workspace.lock().unwrap();
        let e = ws.entry(id)?;
        let setup = e.setup.clone();
        let tags = e.effective_tags().cloned();
        ws.gesture = None;
        match what {
            "settings" => {
                let mut previous = Vec::new();
                for other in &mut ws.entries {
                    if other.id != id {
                        previous
                            .push((other.id, std::mem::replace(&mut other.setup, setup.clone())));
                        other.export_status = "Ready".into();
                    }
                }
                let old = std::mem::replace(&mut ws.defaults, setup.clone());
                ws.remember(Change::Setups(previous, Some(Box::new(old))));
                if ws.preview.as_ref().is_some_and(|p| p.id != id) {
                    self.schedule(&mut ws, setup.settings)?;
                }
                Self::save_defaults(&ws)?;
            }
            "tags" => {
                let tags = tags.ok_or("Load the source track before applying its metadata.")?;
                let previous = ws
                    .entries
                    .iter_mut()
                    .filter(|e| e.id != id)
                    .map(|e| {
                        e.export_status = "Ready".into();
                        (e.id, e.tags.replace(tags.clone()))
                    })
                    .collect();
                ws.remember(Change::Tags(previous));
            }
            _ => return Err("Choose settings or tags.".into()),
        }
        Ok(())
    }
    fn apply_change(
        &self,
        ws: &mut Workspace,
        change: Change,
    ) -> Result<(Change, Option<u64>), String> {
        let mut select = None;
        let inverse = match change {
            Change::Setups(values, defaults) => {
                let mut previous = vec![];
                let mut rerender = None;
                let active = ws.preview.as_ref().map(|p| p.id);
                for (id, value) in values {
                    let e = ws.entry_mut(id)?;
                    if Some(id) == active {
                        rerender = Some(value.settings.clone());
                    }
                    previous.push((id, std::mem::replace(&mut e.setup, value)));
                    e.export_status = "Ready".into();
                }
                let def = defaults.map(|d| Box::new(std::mem::replace(&mut ws.defaults, *d)));
                if def.is_some() {
                    Self::save_defaults(ws)?;
                }
                if let Some(settings) = rerender {
                    self.schedule(ws, settings)?;
                }
                Change::Setups(previous, def)
            }
            Change::Tags(values) => {
                let mut old = vec![];
                for (id, value) in values {
                    let e = ws.entry_mut(id)?;
                    old.push((id, std::mem::replace(&mut e.tags, value)));
                    e.export_status = "Ready".into();
                }
                Change::Tags(old)
            }
            Change::Restore(index, entry) => {
                let id = entry.id;
                ws.entries.insert(index.min(ws.entries.len()), *entry);
                select = Some(id);
                Change::Remove(id)
            }
            Change::Remove(id) => {
                let idx = ws
                    .entries
                    .iter()
                    .position(|e| e.id == id)
                    .ok_or("Track is no longer in the queue.")?;
                let entry = ws.entries.remove(idx);
                if ws.preview.as_ref().is_some_and(|p| p.id == id) {
                    self.selection.fetch_add(1, Ordering::AcqRel);
                    self.revision.fetch_add(1, Ordering::AcqRel);
                    ws.preview = None;
                    self.transport.clear();
                    select = ws
                        .entries
                        .get(idx.min(ws.entries.len().saturating_sub(1)))
                        .map(|e| e.id);
                }
                Change::Restore(idx, Box::new(entry))
            }
        };
        Ok((inverse, select))
    }
    pub fn remove(&self, id: u64) -> Result<Option<Track>, String> {
        let select = {
            let mut ws = self.workspace.lock().unwrap();
            if ws.exporting {
                return Err("Wait for the current export to finish.".into());
            }
            let (inverse, select) = self.apply_change(&mut ws, Change::Remove(id))?;
            ws.gesture = None;
            ws.remember(inverse);
            select
        };
        match select {
            Some(id) => self.select(id).map(Some),
            None => Ok(self.track()),
        }
    }
    pub fn history(&self, redo: bool) -> Result<Option<Track>, String> {
        let select = {
            let mut ws = self.workspace.lock().unwrap();
            if ws.exporting {
                return Err("Wait for the current export to finish.".into());
            }
            let next = if redo { ws.redo.pop() } else { ws.undo.pop() };
            let Some(change) = next else {
                return Ok(ws.track());
            };
            let (inverse, select) = self.apply_change(&mut ws, change)?;
            ws.gesture = None;
            if redo {
                ws.undo.push(inverse)
            } else {
                ws.redo.push(inverse)
            }
            select
        };
        match select {
            Some(id) => self.select(id).map(Some),
            None => Ok(self.track()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provisional_gain_compensates_input_and_preserves_target_change() {
        let rate = 48000;
        let a = Audio {
            channels: 2,
            sample_rate: rate,
            samples: (0..rate)
                .flat_map(|i| {
                    let v = (std::f64::consts::TAU * 1000. * i as f64 / rate as f64).sin() as f32
                        * 0.01;
                    [v, v]
                })
                .collect(),
        };
        let old = Settings {
            clean_low_end: false,
            limit: false,
            ..Settings::default()
        };
        let new = Settings {
            input_gain_db: 6.,
            target_lufs: old.target_lufs - 3.,
            ..old.clone()
        };
        assert!((provisional_gain(&a, 0.35, 0., &old, &new) + 9.).abs() < 1e-5);
    }
}
