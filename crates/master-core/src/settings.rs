use serde::{Deserialize, Serialize};

pub const PRESETS: &[&str] = &[
    "Balanced",
    "AI Cleanup",
    "Vocal Boost",
    "Bass Boost",
    "Bright & Wide",
    "Warm Glue",
    "Club Loud",
];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Shape {
    #[serde(alias = "Bell")]
    Bell,
    #[serde(alias = "Low Shelf")]
    LowShelf,
    #[serde(alias = "High Shelf")]
    HighShelf,
    #[serde(alias = "High Pass")]
    HighPass,
    #[serde(alias = "Low Pass")]
    LowPass,
    #[serde(alias = "Notch")]
    Notch,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Band {
    pub frequency: f64,
    pub gain: f64,
    pub q: f64,
    pub shape: Shape,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Compressor {
    pub threshold: f64,
    pub ratio: f64,
    pub attack: f64,
    pub release: f64,
    pub makeup: f64,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            threshold: -18.,
            ratio: 3.,
            attack: 20.,
            release: 250.,
            makeup: 0.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeEsser {
    pub frequency: f64,
    pub threshold: f64,
    pub reduction: f64,
}

impl Default for DeEsser {
    fn default() -> Self {
        Self {
            frequency: 5500.,
            threshold: -24.,
            reduction: 6.,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub bands: [Band; 10],
    #[serde(alias = "inputGain")]
    pub input_gain_db: f64,
    pub width: f64,
    #[serde(alias = "targetLUFS")]
    pub target_lufs: f64,
    #[serde(alias = "ceiling")]
    pub ceiling_db: f64,
    pub glue: bool,
    pub compressor: Compressor,
    pub de_esser: Option<DeEsser>,
    pub smooth_highs: bool,
    pub eq_bypassed: bool,
    pub center_bass: bool,
    pub clean_low_end: bool,
    pub cut_mud: bool,
    pub tame_harsh: bool,
    pub add_air: bool,
    pub normalize: bool,
    pub limit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let bands = [
            40., 80., 160., 250., 500., 1000., 2000., 4000., 8000., 12000.,
        ]
        .map(|frequency| {
            let shape = if frequency == 80.0 {
                Shape::LowShelf
            } else if frequency == 12000.0 {
                Shape::HighShelf
            } else {
                Shape::Bell
            };
            Band {
                frequency,
                shape,
                gain: 0.0,
                q: if shape == Shape::Bell { 1.0 } else { 0.707 },
                enabled: true,
            }
        });
        Self {
            bands,
            input_gain_db: 0.0,
            width: 100.0,
            target_lufs: -14.0,
            ceiling_db: -1.0,
            glue: false,
            compressor: Compressor::default(),
            de_esser: None,
            smooth_highs: false,
            eq_bypassed: false,
            center_bass: false,
            clean_low_end: true,
            cut_mud: false,
            tame_harsh: false,
            add_air: false,
            normalize: true,
            limit: true,
        }
    }
}

impl Settings {
    pub fn preset(name: &str) -> Result<Self, String> {
        let mut s = Self::default();
        let gains = match name {
            "Balanced" => [0.; 5],
            "AI Cleanup" => {
                s.glue = true;
                s.center_bass = true;
                s.cut_mud = true;
                s.add_air = true;
                s.tame_harsh = true;
                [1., -2., 1., -1., 1.]
            }
            "Vocal Boost" => {
                s.glue = true;
                s.cut_mud = true;
                s.add_air = true;
                [-2., -1., 2., 3., 1.]
            }
            "Bass Boost" => {
                s.width = 105.;
                s.target_lufs = -12.;
                s.glue = true;
                s.center_bass = true;
                [5., 2., 0., -1., -1.]
            }
            "Bright & Wide" => {
                s.width = 120.;
                s.center_bass = true;
                s.add_air = true;
                [-1., 0., 1., 3., 4.]
            }
            "Warm Glue" => {
                s.width = 95.;
                s.glue = true;
                s.center_bass = true;
                s.tame_harsh = true;
                [3., 2., 0., -2., -3.]
            }
            "Club Loud" => {
                s.width = 110.;
                s.target_lufs = -9.;
                s.ceiling_db = -0.8;
                s.glue = true;
                s.center_bass = true;
                s.cut_mud = true;
                [2., -1., 0., 1., 2.]
            }
            _ => return Err(format!("Unknown preset: {name}")),
        };
        for (index, gain) in [1, 3, 5, 7, 9].into_iter().zip(gains) {
            s.bands[index].gain = gain;
        }
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (i, band) in self.bands.iter().enumerate() {
            if !(20.0..=20_000.0).contains(&band.frequency) {
                return Err(format!("EQ band {} frequency must be 20–20,000 Hz.", i + 1));
            }
            if !(-18.0..=18.0).contains(&band.gain) {
                return Err(format!("EQ band {} gain must be –18 to +18 dB.", i + 1));
            }
            if !(0.2..=12.0).contains(&band.q) {
                return Err(format!("EQ band {} Q must be 0.2–12.", i + 1));
            }
        }
        let c = self.compressor;
        let compressor_ok = (-48.0..=0.0).contains(&c.threshold)
            && (1.0..=10.0).contains(&c.ratio)
            && (0.1..=100.0).contains(&c.attack)
            && (10.0..=1000.0).contains(&c.release)
            && (0.0..=12.0).contains(&c.makeup);
        let de_esser_ok = self.de_esser.is_none_or(|d| {
            (2000.0..=10000.0).contains(&d.frequency)
                && (-48.0..=0.0).contains(&d.threshold)
                && (0.0..=12.0).contains(&d.reduction)
        });
        if !compressor_ok {
            return Err(
                "The compressor threshold, ratio, attack, release, or makeup is out of range."
                    .into(),
            );
        }
        if !de_esser_ok {
            return Err("The de-esser frequency, threshold, or reduction is out of range.".into());
        }
        if !(-48.0..=24.0).contains(&self.input_gain_db)
            || !(0.0..=200.0).contains(&self.width)
            || !(-36.0..=-3.0).contains(&self.target_lufs)
            || !(-12.0..=0.0).contains(&self.ceiling_db)
        {
            return Err("One or more mastering controls are out of range.".into());
        }
        Ok(())
    }
}
