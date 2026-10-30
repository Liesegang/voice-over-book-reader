use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpreadMode {
    #[default]
    Auto,
    Single,
    Double,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Light,
    Dark,
    Sepia,
    SolarizedLight,
    SolarizedDark,
    OneDark,
    Nord,
    GruvboxDark,
}
impl Theme {
    pub const ALL: [Self; 8] = [
        Self::Light,
        Self::Sepia,
        Self::Dark,
        Self::SolarizedLight,
        Self::SolarizedDark,
        Self::OneDark,
        Self::Nord,
        Self::GruvboxDark,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "ライト",
            Self::Dark => "ダーク",
            Self::Sepia => "セピア",
            Self::SolarizedLight => "Solarized Light",
            Self::SolarizedDark => "Solarized Dark",
            Self::OneDark => "One Dark",
            Self::Nord => "Nord",
            Self::GruvboxDark => "Gruvbox Dark",
        }
    }
    pub fn is_dark(self) -> bool {
        matches!(
            self,
            Self::Dark | Self::SolarizedDark | Self::OneDark | Self::Nord | Self::GruvboxDark
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadingMode {
    #[default]
    Auto,
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsSettings {
    pub pronunciations: BTreeMap<String, String>,
    pub pre_cache: bool,
    pub provider_id: String,
    pub provider_config: Option<serde_json::Value>,
    pub voice: Option<String>,
    pub speed: f32,
    pub auto_follow: bool,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            pronunciations: BTreeMap::new(),
            pre_cache: true,
            provider_id: "irodori".into(),
            provider_config: None,
            voice: None,
            speed: 1.0,
            auto_follow: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub scroll_speed: f32,
    pub text_spread: bool,
    pub library_list: bool,
    pub custom_shelves: BTreeMap<String, Vec<String>>,
    pub key_bindings: BTreeMap<String, String>,
    pub spread: SpreadMode,
    pub theme: Theme,
    pub custom_background: Option<[u8; 3]>,
    pub custom_foreground: Option<[u8; 3]>,
    pub adapt_monochrome_images: bool,
    pub accent: [u8; 3],
    pub font_family: String,
    pub font_size: f32,
    pub line_spacing: f32,
    pub letter_spacing: f32,
    pub margins: f32,
    /// Missing values retain the older single-margin setting on upgrade.
    pub margin_horizontal: Option<f32>,
    pub margin_vertical: Option<f32>,
    /// None retains the original prose/image spread defaults (24/0 px).
    pub page_gap: Option<f32>,
    pub show_ruby: bool,
    pub ruby_scale: f32,
    pub reading_mode: ReadingMode,
    pub tts: TtsSettings,
    pub calibre_path: Option<PathBuf>,
    pub dictionary_paths: Vec<PathBuf>,
    pub last_book: Option<PathBuf>,
    pub opds: Option<crate::services::OpdsConfig>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scroll_speed: 2.25,
            text_spread: false,
            library_list: false,
            custom_shelves: BTreeMap::new(),
            key_bindings: BTreeMap::new(),
            spread: SpreadMode::Auto,
            theme: Theme::Light,
            custom_background: None,
            custom_foreground: None,
            adapt_monochrome_images: true,
            accent: [37, 99, 235],
            font_family: "Noto Serif CJK JP".into(),
            font_size: 24.0,
            line_spacing: 1.65,
            letter_spacing: 0.0,
            margins: 48.0,
            margin_horizontal: None,
            margin_vertical: None,
            page_gap: None,
            show_ruby: true,
            ruby_scale: 0.5,
            reading_mode: ReadingMode::Auto,
            tts: TtsSettings::default(),
            calibre_path: None,
            dictionary_paths: Vec::new(),
            last_book: None,
            opds: None,
        }
    }
}

impl Settings {
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.custom_background = None;
        self.custom_foreground = None;
    }
    pub fn horizontal_margin(&self) -> f32 {
        self.margin_horizontal.unwrap_or(self.margins)
    }

    pub fn vertical_margin(&self) -> f32 {
        self.margin_vertical.unwrap_or(self.margins)
    }

    pub fn spread_gap(&self, image_pages: bool) -> f32 {
        self.page_gap.unwrap_or(if image_pages { 0. } else { 24. })
    }

    /// Reject non-finite values before they reach layout, playback or JSON.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.scroll_speed.is_finite() && (0.5..=4.0).contains(&self.scroll_speed),
            "Scroll speed must be 0.5–4"
        );
        anyhow::ensure!(
            self.font_size.is_finite() && (10.0..=96.0).contains(&self.font_size),
            "Font size must be 10–96"
        );
        anyhow::ensure!(
            self.line_spacing.is_finite() && (1.0..=4.0).contains(&self.line_spacing),
            "Line spacing must be 1–4"
        );
        anyhow::ensure!(
            self.ruby_scale.is_finite() && (0.2..=1.0).contains(&self.ruby_scale),
            "Ruby scale must be 0.2–1"
        );
        anyhow::ensure!(
            self.letter_spacing.is_finite() && (-0.1..=1.0).contains(&self.letter_spacing),
            "Letter spacing must be -0.1–1 em"
        );
        anyhow::ensure!(
            self.margins.is_finite() && (0.0..=200.0).contains(&self.margins),
            "Margins must be 0–200"
        );
        for value in [self.margin_horizontal, self.margin_vertical, self.page_gap]
            .into_iter()
            .flatten()
        {
            anyhow::ensure!(
                value.is_finite() && (0.0..=200.0).contains(&value),
                "Page spacing must be 0–200"
            );
        }
        anyhow::ensure!(
            self.tts.speed.is_finite() && (0.5..=10.0).contains(&self.tts.speed),
            "Playback speed must be 0.5–10"
        );
        Ok(())
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    #[test]
    fn old_settings_keep_original_colors_and_new_presets_roundtrip() {
        let old: Settings =
            serde_json::from_str(r#"{"theme":"Sepia","accent":[37,99,235]}"#).unwrap();
        assert_eq!(old.theme, Theme::Sepia);
        assert!(old.custom_background.is_none() && old.custom_foreground.is_none());
        for theme in Theme::ALL {
            let settings = Settings {
                theme,
                custom_background: Some([18, 27, 38]),
                custom_foreground: Some([230, 224, 211]),
                ..Settings::default()
            };
            let restored: Settings =
                serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
            assert_eq!(restored.theme, theme);
            assert_eq!(restored.custom_background, settings.custom_background);
            assert_eq!(restored.custom_foreground, settings.custom_foreground);
        }
    }
    #[test]
    fn choosing_a_preset_clears_only_color_overrides() {
        let mut settings = Settings {
            custom_background: Some([1, 2, 3]),
            custom_foreground: Some([4, 5, 6]),
            font_size: 32.,
            ..Settings::default()
        };
        settings
            .tts
            .pronunciations
            .insert("東京".into(), "とうきょう".into());
        settings.set_theme(Theme::Nord);
        assert_eq!(settings.theme, Theme::Nord);
        assert!(settings.custom_background.is_none() && settings.custom_foreground.is_none());
        assert_eq!(settings.font_size, 32.);
        assert_eq!(settings.tts.pronunciations["東京"], "とうきょう");
    }
}

#[cfg(test)]
mod margin_tests {
    use super::*;

    #[test]
    fn old_saved_margin_survives_upgrade_and_each_axis_can_change_independently() {
        let mut old: Settings = serde_json::from_str(r#"{"margins":36.0}"#).unwrap();
        assert_eq!((old.horizontal_margin(), old.vertical_margin()), (36., 36.));
        assert_eq!((old.spread_gap(false), old.spread_gap(true)), (24., 0.));
        old.margin_horizontal = Some(72.);
        old.page_gap = Some(12.);
        let roundtrip: Settings =
            serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
        assert_eq!(
            (roundtrip.horizontal_margin(), roundtrip.vertical_margin()),
            (72., 36.)
        );
        assert_eq!(
            (roundtrip.spread_gap(false), roundtrip.spread_gap(true)),
            (12., 12.)
        );
        old.margin_vertical = Some(f32::NAN);
        assert!(old.validate().is_err());
    }
}
