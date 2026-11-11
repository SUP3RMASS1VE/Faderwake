use crate::media::Output;
use master_core::Settings;
use serde::Serialize;

#[derive(Serialize)]
pub struct SavedPreset {
    name: String,
    settings: Settings,
    output: Output,
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use master_core::Band;
    use serde::Deserialize;
    use std::path::Path;

    #[derive(Deserialize)]
    struct LegacyPreset {
        name: String,
        settings: LegacySettings,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacySettings {
        #[serde(flatten)]
        chain: Settings,
        #[serde(rename = "parametricEQ")]
        parametric_eq: Option<[Band; 10]>,
        eq: Option<[f64; 5]>,
        format: Option<String>,
        sample_rate: Option<f64>,
        bit_depth: Option<u16>,
    }

    impl LegacyPreset {
        fn convert(self) -> Result<SavedPreset, String> {
            let source = self.settings;
            let mut settings = source.chain;
            if let Some(bands) = source.parametric_eq {
                settings.bands = bands;
            } else if let Some(gains) = source.eq {
                for (index, gain) in [1, 3, 5, 7, 9].into_iter().zip(gains) {
                    settings.bands[index].gain = gain;
                }
            }
            settings.validate()?;
            let mut output = Output {
                format: source
                    .format
                    .as_deref()
                    .unwrap_or("wav")
                    .to_ascii_lowercase(),
                sample_rate: source.sample_rate.unwrap_or(44100.) as u32,
                bit_depth: source.bit_depth.unwrap_or(24),
            };
            if output.format == "aac" && ![44100, 48000].contains(&output.sample_rate) {
                output.sample_rate = 48000;
            }
            output.validate()?;
            Ok(SavedPreset {
                name: self.name,
                settings,
                output,
            })
        }
    }

    pub fn load(path: &Path) -> Result<Vec<SavedPreset>, String> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.to_string()),
        };
        let prefs =
            plist::Value::from_reader(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
        let Some(value) = prefs
            .as_dictionary()
            .and_then(|d| d.get("native-master-presets"))
        else {
            return Ok(vec![]);
        };
        let data = value
            .as_data()
            .ok_or("The saved preset collection is unreadable.")?;
        let presets: Vec<LegacyPreset> = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        presets.into_iter().map(LegacyPreset::convert).collect()
    }

    #[test]
    fn imports_five_band_presets_with_swift_defaults() {
        let original = r#"{"name":"Old chain","settings":{"eq":[1,2,-3,4,5],"ceiling":-2,"inputGain":3,"sampleRate":96000,"bitDepth":16,"format":"AAC"}}"#;
        let preset: LegacyPreset = serde_json::from_str(original).unwrap();
        let result = preset.convert().unwrap();
        assert_eq!(result.settings.bands[5].gain, -3.);
        assert_eq!(result.settings.input_gain_db, 3.);
        assert_eq!(result.settings.ceiling_db, -2.);
        assert_eq!(result.output.sample_rate, 48000);
        assert_eq!(result.output.bit_depth, 16);
    }
}

#[tauri::command]
pub fn import_legacy_presets(app: tauri::AppHandle) -> Result<Vec<SavedPreset>, String> {
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;
        if crate::snapshot::active() {
            return Ok(vec![]);
        }
        let home = app.path().home_dir().map_err(|e| e.to_string())?;
        mac::load(&home.join("Library/Preferences/com.ai.music.remastering.native.plist"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(vec![])
    }
}
