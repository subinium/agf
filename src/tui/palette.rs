use slt::{Color, Context};

use crate::model::Agent;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    pub(crate) background: Color,
    pub(crate) text: Color,
    pub(crate) secondary: Color,
    pub(crate) muted: Color,
    pub(crate) selection_bg: Color,
    pub(crate) selection_text: Color,
    pub(crate) selection_muted: Color,
    pub(crate) accent: Color,
    pub(crate) warning: Color,
    pub(crate) danger: Color,
    pub(crate) success: Color,
    pub(crate) border: Color,
    basic: bool,
}

impl Palette {
    pub(crate) const fn dark() -> Self {
        Self {
            basic: false,
            background: Color::Rgb(24, 27, 29),
            text: Color::Rgb(232, 232, 232),
            secondary: Color::Rgb(202, 202, 202),
            muted: Color::Rgb(166, 166, 166),
            selection_bg: Color::Rgb(59, 59, 59),
            selection_text: Color::Rgb(250, 250, 250),
            selection_muted: Color::Rgb(193, 193, 193),
            accent: Color::Rgb(88, 205, 207),
            warning: Color::Rgb(231, 187, 101),
            danger: Color::Rgb(245, 139, 139),
            success: Color::Rgb(144, 196, 164),
            border: Color::Rgb(107, 107, 107),
        }
    }

    pub(crate) const fn light() -> Self {
        Self {
            basic: false,
            background: Color::Rgb(245, 247, 247),
            text: Color::Rgb(32, 32, 32),
            secondary: Color::Rgb(72, 72, 72),
            muted: Color::Rgb(96, 96, 96),
            selection_bg: Color::Rgb(228, 228, 228),
            selection_text: Color::Rgb(32, 32, 32),
            selection_muted: Color::Rgb(82, 82, 82),
            accent: Color::Rgb(0, 104, 111),
            warning: Color::Rgb(106, 68, 0),
            danger: Color::Rgb(172, 41, 48),
            success: Color::Rgb(36, 102, 65),
            border: Color::Rgb(125, 135, 138),
        }
    }

    pub(crate) fn from_ui(ui: &Context) -> Self {
        let palette = if ui.theme().is_dark {
            Self::dark()
        } else {
            Self::light()
        };
        if ui.try_use_context::<slt::ColorDepth>() == Some(&slt::ColorDepth::Basic) {
            palette.basic()
        } else {
            palette
        }
    }

    fn basic(self) -> Self {
        let dark = self.background.luminance_f64() < 0.5;
        let background = if dark { Color::Black } else { Color::White };
        let text = if dark { Color::White } else { Color::Black };
        // ANSI hue approximations vary; keep readable text and structural cues.
        Self {
            background,
            text,
            secondary: text,
            muted: text,
            selection_bg: background,
            selection_text: text,
            selection_muted: text,
            accent: text,
            warning: text,
            danger: text,
            success: text,
            border: text,
            basic: true,
        }
    }

    pub(crate) fn agent(self, agent: Agent) -> Color {
        if self.basic {
            return self.text;
        }
        let (r, g, b) = agent.color();
        let brand = Color::Rgb(r, g, b);
        let readable = |color: Color| {
            [slt::ColorDepth::TrueColor, slt::ColorDepth::EightBit]
                .into_iter()
                .all(|depth| {
                    let color = color.downsampled(depth);
                    Color::contrast_ratio_f64(color, self.background.downsampled(depth)) >= 4.5
                        && Color::contrast_ratio_f64(color, self.selection_bg.downsampled(depth))
                            >= 4.5
                })
        };
        if readable(brand) {
            return brand;
        }

        // A bounded blend preserves the hue while keeping either row readable.
        let target = Color::contrast_fg(self.background);
        for step in 1..=16 {
            let candidate = target.blend_f64(brand, f64::from(step) / 16.0);
            if readable(candidate) {
                return candidate;
            }
        }
        target
    }

    pub(crate) const fn row_text(self, selected: bool) -> Color {
        if selected {
            self.selection_text
        } else {
            self.text
        }
    }

    /// Navigation markers are neutral; agent and status colors have other roles.
    pub(crate) const fn marker(self, selected: bool) -> Color {
        if selected {
            self.selection_text
        } else {
            self.selection_muted
        }
    }

    pub(crate) const fn row_muted(self, selected: bool) -> Color {
        if selected {
            self.selection_muted
        } else {
            self.muted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_contrast(foreground: Color, background: Color, minimum: f64) {
        let ratio = Color::contrast_ratio_f64(foreground, background);
        assert!(
            ratio >= minimum,
            "{foreground:?} on {background:?}: {ratio:.3} < {minimum}"
        );
        assert_ne!(foreground, background);
    }

    #[test]
    fn basic_terminals_keep_text_legible_without_unsafe_hue_approximations() {
        let mut backend = slt::TestBackend::new(20, 8);
        for theme in [slt::Theme::dark(), slt::Theme::light()] {
            backend.render(|ui| {
                ui.set_theme(theme);
                ui.provide(slt::ColorDepth::Basic, |ui| {
                    let palette = Palette::from_ui(ui);
                    for background in [palette.background, palette.selection_bg] {
                        for foreground in [
                            palette.text,
                            palette.secondary,
                            palette.muted,
                            palette.marker(true),
                            palette.marker(false),
                            palette.accent,
                            palette.warning,
                            palette.danger,
                            palette.success,
                        ] {
                            assert_contrast(
                                foreground.downsampled(slt::ColorDepth::Basic),
                                background.downsampled(slt::ColorDepth::Basic),
                                4.5,
                            );
                        }
                        for &agent in Agent::all() {
                            assert_contrast(
                                palette.agent(agent).downsampled(slt::ColorDepth::Basic),
                                background.downsampled(slt::ColorDepth::Basic),
                                4.5,
                            );
                        }
                    }
                });
                assert!(
                    !Palette::from_ui(ui).basic,
                    "limited-color context must not leak"
                );
            });
        }
    }

    #[test]
    fn selection_surfaces_and_markers_are_achromatic() {
        for palette in [Palette::dark(), Palette::light()] {
            for depth in [slt::ColorDepth::TrueColor, slt::ColorDepth::EightBit] {
                for color in [
                    palette.selection_bg,
                    palette.marker(false),
                    palette.marker(true),
                ] {
                    let Some(Color::Rgb(r, g, b)) =
                        Color::from_hex(&color.downsampled(depth).to_hex())
                    else {
                        panic!("RGB conversion failed");
                    };
                    assert_eq!((r, g), (g, b), "navigation must not carry an agent hue");
                }
                for selected in [false, true] {
                    let marker = palette.marker(selected).downsampled(depth);
                    let background = if selected {
                        palette.selection_bg
                    } else {
                        palette.background
                    }
                    .downsampled(depth);
                    assert_contrast(marker, background, 4.5);
                    assert_ne!(marker, palette.accent.downsampled(depth));
                    assert_ne!(marker, palette.agent(Agent::Codex).downsampled(depth));
                }
            }
        }
    }

    #[test]
    fn semantic_text_is_readable_on_both_surfaces() {
        for palette in [Palette::dark(), Palette::light()] {
            for background in [palette.background, palette.selection_bg] {
                for foreground in [
                    palette.text,
                    palette.secondary,
                    palette.muted,
                    palette.selection_text,
                    palette.selection_muted,
                    palette.accent,
                    palette.warning,
                    palette.danger,
                    palette.success,
                ] {
                    assert_contrast(foreground, background, 4.5);
                }
                assert_ne!(palette.border, background);
            }
            assert_ne!(palette.background, palette.selection_bg);
            assert_contrast(palette.border, palette.background, 3.0);
        }
    }

    #[test]
    fn agent_colors_are_readable_on_both_surfaces() {
        for palette in [Palette::dark(), Palette::light()] {
            for &agent in Agent::all() {
                let color = palette.agent(agent);
                assert_contrast(color, palette.background, 4.5);
                assert_contrast(color, palette.selection_bg, 4.5);
                assert_eq!(color, palette.agent(agent));
            }
        }
    }

    #[test]
    fn readable_brand_colors_are_not_changed() {
        for palette in [Palette::dark(), Palette::light()] {
            for &agent in Agent::all() {
                let (r, g, b) = agent.color();
                let brand = Color::Rgb(r, g, b);
                if [slt::ColorDepth::TrueColor, slt::ColorDepth::EightBit]
                    .into_iter()
                    .all(|depth| {
                        let brand = brand.downsampled(depth);
                        Color::contrast_ratio_f64(brand, palette.background.downsampled(depth))
                            >= 4.5
                            && Color::contrast_ratio_f64(
                                brand,
                                palette.selection_bg.downsampled(depth),
                            ) >= 4.5
                    })
                {
                    assert_eq!(palette.agent(agent), brand);
                }
            }
        }
    }

    #[test]
    fn rows_use_selected_text_and_metadata_tokens() {
        for palette in [Palette::dark(), Palette::light()] {
            assert_eq!(palette.row_text(false), palette.text);
            assert_eq!(palette.row_text(true), palette.selection_text);
            assert_eq!(palette.row_muted(false), palette.muted);
            assert_eq!(palette.row_muted(true), palette.selection_muted);
        }
        assert_eq!(Palette::dark().selection_bg, Color::Rgb(59, 59, 59));
    }

    #[test]
    fn default_and_reset_themes_respect_the_dark_flag() {
        let mut backend = slt::TestBackend::new(20, 8);
        backend.render(|ui| {
            assert_eq!(Palette::from_ui(ui).background, Palette::dark().background);
            let mut theme = slt::Theme::light();
            theme.bg = Color::Reset;
            ui.set_theme(theme);
            assert_eq!(Palette::from_ui(ui).background, Palette::light().background);
        });
    }

    #[test]
    fn explicit_theme_flag_takes_precedence_over_background_luminance() {
        let mut backend = slt::TestBackend::new(20, 8);
        backend.render(|ui| {
            for background in [Color::Rgb(247, 247, 247), Color::Indexed(255), Color::White] {
                let mut theme = slt::Theme::dark();
                theme.bg = background;
                ui.set_theme(theme);
                assert_eq!(Palette::from_ui(ui).background, Palette::dark().background);
            }
            for background in [Color::Rgb(24, 24, 24), Color::Indexed(232), Color::Black] {
                let mut theme = slt::Theme::light();
                theme.bg = background;
                ui.set_theme(theme);
                assert_eq!(Palette::from_ui(ui).background, Palette::light().background);
            }
        });
    }
}
