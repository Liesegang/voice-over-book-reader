use eframe::egui::{self, Color32};
use reader_library::{Settings, Theme};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub paper: Color32,
    pub panel: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub selection: Color32,
}
impl Palette {
    pub fn for_settings(settings: &Settings) -> Self {
        let mut palette = Self::for_theme(settings.theme, settings.accent);
        if let Some([r, g, b]) = settings.custom_background {
            palette.paper = Color32::from_rgb(r, g, b);
        }
        if let Some([r, g, b]) = settings.custom_foreground {
            palette.ink = Color32::from_rgb(r, g, b);
        }
        if settings.custom_background.is_some() || settings.custom_foreground.is_some() {
            palette.panel = mix(palette.paper, palette.ink, 0.065);
            palette.muted = mix(palette.paper, palette.ink, 0.63);
            palette.border = mix(palette.paper, palette.ink, 0.20);
        }
        if settings.accent == [37, 99, 235] {
            palette.accent = if palette.is_dark() {
                Color32::from_rgb(138, 180, 255)
            } else {
                Color32::from_rgb(37, 99, 235)
            };
        }
        palette.selection =
            palette
                .accent
                .gamma_multiply(if palette.is_dark() { 0.25 } else { 0.15 });
        palette
    }
    pub fn for_theme(theme: Theme, accent: [u8; 3]) -> Self {
        let c = |rgb: [u8; 3]| Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        let accent = if theme.is_dark() && accent == [37, 99, 235] {
            c([138, 180, 255])
        } else {
            c(accent)
        };
        match theme {
            Theme::Light => Self {
                paper: c([250, 250, 247]),
                panel: c([240, 241, 237]),
                ink: c([41, 45, 44]),
                muted: c([105, 113, 110]),
                border: c([220, 224, 220]),
                accent,
                selection: accent.gamma_multiply(0.14),
            },
            Theme::Sepia => Self {
                paper: c([243, 233, 213]),
                panel: c([232, 223, 201]),
                ink: c([73, 63, 50]),
                muted: c([126, 114, 94]),
                border: c([215, 204, 182]),
                accent,
                selection: accent.gamma_multiply(0.15),
            },
            Theme::Dark => Self {
                paper: c([28, 32, 35]),
                panel: c([37, 42, 45]),
                ink: c([231, 231, 223]),
                muted: c([162, 173, 168]),
                border: c([57, 65, 68]),
                accent,
                selection: accent.gamma_multiply(0.25),
            },
            // Canonical paper/text pairs; surrounding native UI shades are
            // adapted from each theme's neutral palette.
            // https://ethanschoonover.com/solarized/
            Theme::SolarizedLight => Self {
                paper: c([253, 246, 227]),
                panel: c([238, 232, 213]),
                ink: c([101, 123, 131]),
                muted: c([131, 148, 150]),
                border: c([147, 161, 161]),
                accent,
                selection: accent.gamma_multiply(0.15),
            },
            Theme::SolarizedDark => Self {
                paper: c([0, 43, 54]),
                panel: c([7, 54, 66]),
                ink: c([131, 148, 150]),
                muted: c([101, 123, 131]),
                border: c([88, 110, 117]),
                accent,
                selection: accent.gamma_multiply(0.25),
            },
            // https://github.com/atom/one-dark-syntax/blob/master/styles/colors.less
            Theme::OneDark => Self {
                paper: c([40, 44, 52]),
                panel: c([33, 37, 43]),
                ink: c([171, 178, 191]),
                muted: c([127, 132, 142]),
                border: c([62, 68, 81]),
                accent,
                selection: accent.gamma_multiply(0.25),
            },
            // https://www.nordtheme.com/docs/colors-and-palettes/
            Theme::Nord => Self {
                paper: c([46, 52, 64]),
                panel: c([59, 66, 82]),
                ink: c([236, 239, 244]),
                muted: c([216, 222, 233]),
                border: c([76, 86, 106]),
                accent,
                selection: accent.gamma_multiply(0.25),
            },
            // https://github.com/morhetz/gruvbox/blob/master/colors/gruvbox.vim
            Theme::GruvboxDark => Self {
                paper: c([40, 40, 40]),
                panel: c([60, 56, 54]),
                ink: c([235, 219, 178]),
                muted: c([168, 153, 132]),
                border: c([80, 73, 69]),
                accent,
                selection: accent.gamma_multiply(0.25),
            },
        }
    }
    pub fn is_dark(self) -> bool {
        let [r, g, b, _] = self.paper.to_array();
        (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) < 128.
    }
    pub fn apply(self, ctx: &egui::Context, _legacy_dark: bool) {
        let dark = self.is_dark();
        ctx.set_theme(if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        let mut visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.panel_fill = self.paper;
        visuals.window_fill = self.paper;
        visuals.extreme_bg_color = self.panel;
        visuals.override_text_color = Some(self.ink);
        visuals.faint_bg_color = self.panel;
        visuals.selection.bg_fill = self.selection;
        visuals.selection.stroke.color = self.accent;
        visuals.hyperlink_color = self.accent;
        visuals.weak_text_color = Some(self.muted);
        visuals.widgets.noninteractive.bg_fill = self.paper;
        visuals.widgets.noninteractive.weak_bg_fill = self.paper;
        visuals.widgets.noninteractive.bg_stroke.color = self.border;
        visuals.widgets.inactive.bg_fill = self.panel;
        visuals.widgets.inactive.weak_bg_fill = self.panel;
        visuals.widgets.inactive.fg_stroke.color = self.ink;
        visuals.widgets.inactive.bg_stroke.color = self.border;
        visuals.widgets.hovered.bg_fill = mix(self.panel, self.ink, 0.08);
        visuals.widgets.hovered.weak_bg_fill = visuals.widgets.hovered.bg_fill;
        visuals.widgets.hovered.fg_stroke.color = self.ink;
        visuals.widgets.active.bg_fill = self.selection;
        visuals.widgets.active.fg_stroke.color = self.ink;
        ctx.set_visuals(visuals);
        ctx.global_style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(10.0, 10.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
            style.spacing.scroll.floating = false;
            style.spacing.scroll.bar_width = 9.0;
            style.spacing.scroll.bar_outer_margin = 0.0;
            style.spacing.scroll.bar_inner_margin = 5.0;
            style.spacing.scroll.handle_min_length = 36.0;
            style.scroll_animation = egui::style::ScrollAnimation {
                points_per_second: 1800.0,
                duration: (0.06..=0.20).into(),
            };
        });
    }
}

fn mix(a: Color32, b: Color32, weight: f32) -> Color32 {
    let a = a.to_array();
    let b = b.to_array();
    let channel =
        |i: usize| (f32::from(a[i]) * (1. - weight) + f32::from(b[i]) * weight).round() as u8;
    Color32::from_rgb(channel(0), channel(1), channel(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_use_their_published_paper_and_text_pairs() {
        for (theme, paper, ink) in [
            (Theme::SolarizedLight, [253, 246, 227], [101, 123, 131]),
            (Theme::SolarizedDark, [0, 43, 54], [131, 148, 150]),
            (Theme::OneDark, [40, 44, 52], [171, 178, 191]),
            (Theme::Nord, [46, 52, 64], [236, 239, 244]),
            (Theme::GruvboxDark, [40, 40, 40], [235, 219, 178]),
        ] {
            let p = Palette::for_theme(theme, [37, 99, 235]);
            assert_eq!(&p.paper.to_array()[..3], &paper);
            assert_eq!(&p.ink.to_array()[..3], &ink);
            assert_eq!(p.is_dark(), theme.is_dark());
        }
    }
    #[test]
    fn effective_custom_paper_controls_native_widget_mode_and_exact_text_color() {
        let settings = Settings {
            theme: Theme::Light,
            custom_background: Some([16, 24, 32]),
            custom_foreground: Some([225, 214, 199]),
            ..Settings::default()
        };
        let p = Palette::for_settings(&settings);
        let ctx = egui::Context::default();
        p.apply(&ctx, false);
        assert_eq!(p.paper, Color32::from_rgb(16, 24, 32));
        assert_eq!(p.ink, Color32::from_rgb(225, 214, 199));
        assert!(ctx.global_style().visuals.dark_mode);
        assert_eq!(ctx.global_style().visuals.override_text_color, Some(p.ink));
        let bright = Palette::for_settings(&Settings {
            theme: Theme::Nord,
            custom_background: Some([248, 244, 236]),
            ..settings
        });
        bright.apply(&ctx, true);
        assert!(
            !ctx.global_style().visuals.dark_mode,
            "custom paper brightness supersedes the underlying preset mode"
        );
    }
}
