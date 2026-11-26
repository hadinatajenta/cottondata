use cottondata_sql::{tokenize, TokenKind};
use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, FontFamily, FontId, Stroke, TextFormat, TextStyle, Visuals,
};

pub const RADIUS_CARD: u8 = 10;
pub const RADIUS_CONTROL: u8 = 6;
pub const GUTTER: i8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTheme {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg_backdrop: Color32,
    pub bg_card: Color32,
    pub bg_surface: Color32,
    pub bg_hover: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub dot_grid: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub highlight: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub icon_schema: Color32,
    pub icon_table: Color32,
    pub icon_view: Color32,
    pub row_alt: Color32,
    pub cell_modified: Color32,
    pub cell_inserted: Color32,
    pub cell_deleted: Color32,
    pub sql_keyword: Color32,
    pub sql_string: Color32,
    pub sql_number: Color32,
    pub sql_comment: Color32,
    pub sql_ident: Color32,
    pub sql_sysvar: Color32,
}

const LIGHT_PALETTE: Palette = Palette {
    bg_backdrop: Color32::from_rgb(236, 238, 246),
    bg_card: Color32::from_rgb(255, 255, 255),
    bg_surface: Color32::from_rgb(247, 248, 251),
    bg_hover: Color32::from_rgb(239, 241, 248),
    border: Color32::from_rgb(225, 228, 238),
    divider: Color32::from_rgb(233, 235, 242),
    dot_grid: Color32::from_rgb(214, 218, 230),
    text_primary: Color32::from_rgb(28, 32, 48),
    text_muted: Color32::from_rgb(110, 116, 138),
    accent: Color32::from_rgb(64, 70, 226),
    accent_soft: Color32::from_rgb(234, 235, 253),
    on_accent: Color32::from_rgb(255, 255, 255),
    highlight: Color32::from_rgb(238, 120, 138),
    success: Color32::from_rgb(30, 146, 98),
    warning: Color32::from_rgb(184, 120, 14),
    danger: Color32::from_rgb(210, 58, 76),
    icon_schema: Color32::from_rgb(226, 86, 108),
    icon_table: Color32::from_rgb(58, 104, 222),
    icon_view: Color32::from_rgb(132, 88, 232),
    row_alt: Color32::from_rgb(250, 251, 253),
    cell_modified: Color32::from_rgb(253, 240, 210),
    cell_inserted: Color32::from_rgb(222, 245, 233),
    cell_deleted: Color32::from_rgb(252, 226, 230),
    sql_keyword: Color32::from_rgb(64, 70, 226),
    sql_string: Color32::from_rgb(22, 132, 78),
    sql_number: Color32::from_rgb(176, 102, 12),
    sql_comment: Color32::from_rgb(132, 140, 162),
    sql_ident: Color32::from_rgb(28, 32, 48),
    sql_sysvar: Color32::from_rgb(132, 88, 232),
};

const DARK_PALETTE: Palette = Palette {
    bg_backdrop: Color32::from_rgb(14, 16, 23),
    bg_card: Color32::from_rgb(23, 26, 35),
    bg_surface: Color32::from_rgb(29, 33, 44),
    bg_hover: Color32::from_rgb(37, 42, 56),
    border: Color32::from_rgb(44, 49, 64),
    divider: Color32::from_rgb(36, 40, 53),
    dot_grid: Color32::from_rgb(46, 51, 67),
    text_primary: Color32::from_rgb(230, 233, 242),
    text_muted: Color32::from_rgb(144, 151, 172),
    accent: Color32::from_rgb(124, 128, 255),
    accent_soft: Color32::from_rgb(40, 43, 82),
    on_accent: Color32::from_rgb(255, 255, 255),
    highlight: Color32::from_rgb(244, 132, 150),
    success: Color32::from_rgb(82, 206, 156),
    warning: Color32::from_rgb(245, 188, 78),
    danger: Color32::from_rgb(244, 102, 114),
    icon_schema: Color32::from_rgb(244, 120, 140),
    icon_table: Color32::from_rgb(110, 156, 255),
    icon_view: Color32::from_rgb(178, 140, 255),
    row_alt: Color32::from_rgb(26, 29, 39),
    cell_modified: Color32::from_rgb(66, 54, 26),
    cell_inserted: Color32::from_rgb(24, 58, 44),
    cell_deleted: Color32::from_rgb(70, 30, 38),
    sql_keyword: Color32::from_rgb(140, 150, 255),
    sql_string: Color32::from_rgb(122, 218, 168),
    sql_number: Color32::from_rgb(246, 194, 104),
    sql_comment: Color32::from_rgb(118, 126, 150),
    sql_ident: Color32::from_rgb(222, 226, 240),
    sql_sysvar: Color32::from_rgb(198, 156, 255),
};

impl AppTheme {
    pub fn palette(self) -> Palette {
        match self {
            Self::Light => LIGHT_PALETTE,
            Self::Dark => DARK_PALETTE,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let egui_theme = match self {
            Self::Light => egui::Theme::Light,
            Self::Dark => egui::Theme::Dark,
        };
        ctx.set_theme(egui_theme);
        ctx.set_visuals(self.build_visuals());
        ctx.style_mut(apply_spacing_and_typography);
    }

    fn build_visuals(self) -> Visuals {
        let palette = self.palette();
        let mut visuals = match self {
            Self::Light => Visuals::light(),
            Self::Dark => Visuals::dark(),
        };
        let control_radius = CornerRadius::same(RADIUS_CONTROL);
        let border_stroke = Stroke::new(1.0, palette.border);

        visuals.panel_fill = palette.bg_backdrop;
        visuals.window_fill = palette.bg_card;
        visuals.extreme_bg_color = palette.bg_surface;
        visuals.faint_bg_color = palette.row_alt;
        visuals.code_bg_color = palette.bg_surface;
        visuals.window_stroke = border_stroke;
        visuals.window_corner_radius = CornerRadius::same(12);
        visuals.menu_corner_radius = CornerRadius::same(8);
        visuals.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(40),
        };
        visuals.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(28),
        };
        visuals.selection.bg_fill = palette.accent_soft;
        visuals.selection.stroke = Stroke::new(1.0, palette.accent);
        visuals.hyperlink_color = palette.accent;

        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = palette.bg_card;
        widgets.noninteractive.weak_bg_fill = palette.bg_card;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.divider);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text_primary);
        widgets.noninteractive.corner_radius = control_radius;

        widgets.inactive.bg_fill = palette.bg_surface;
        widgets.inactive.weak_bg_fill = palette.bg_card;
        widgets.inactive.bg_stroke = border_stroke;
        widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text_primary);
        widgets.inactive.corner_radius = control_radius;

        widgets.hovered.bg_fill = palette.bg_hover;
        widgets.hovered.weak_bg_fill = palette.bg_hover;
        widgets.hovered.bg_stroke = border_stroke;
        widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text_primary);
        widgets.hovered.corner_radius = control_radius;
        widgets.hovered.expansion = 0.0;

        widgets.active.bg_fill = palette.accent_soft;
        widgets.active.weak_bg_fill = palette.accent_soft;
        widgets.active.bg_stroke = Stroke::new(1.0, palette.accent);
        widgets.active.fg_stroke = Stroke::new(1.0, palette.text_primary);
        widgets.active.corner_radius = control_radius;
        widgets.active.expansion = 0.0;

        widgets.open = widgets.active;
        visuals
    }
}

fn apply_spacing_and_typography(style: &mut egui::Style) {
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 24.0;
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.scroll = egui::style::ScrollStyle {
        foreground_color: true,
        dormant_handle_opacity: 0.35,
        active_handle_opacity: 0.55,
        interact_handle_opacity: 0.75,
        dormant_background_opacity: 0.0,
        ..egui::style::ScrollStyle::solid()
    };
    style.text_styles = [
        (TextStyle::Heading, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
}

const UI_FONT_NAME: &str = "inter";
const UI_FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const CODE_FONT_NAME: &str = "jetbrains_mono";
const CODE_FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    register_primary_font(&mut fonts, FontFamily::Proportional, UI_FONT_NAME, UI_FONT_BYTES);
    register_primary_font(&mut fonts, FontFamily::Monospace, CODE_FONT_NAME, CODE_FONT_BYTES);
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

fn register_primary_font(
    fonts: &mut egui::FontDefinitions,
    family: FontFamily,
    font_name: &str,
    font_bytes: &'static [u8],
) {
    fonts.font_data.insert(
        font_name.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(font_bytes)),
    );
    fonts
        .families
        .entry(family)
        .or_default()
        .insert(0, font_name.to_owned());
}

pub fn highlight_mysql_layout(
    ui: &egui::Ui,
    text: &str,
    wrap_width: f32,
    palette: Palette,
) -> std::sync::Arc<egui::Galley> {
    build_mysql_layout(ui, text, wrap_width, palette, &[], None)
}

pub fn highlight_mysql_editor_layout(
    ui: &egui::Ui,
    text: &str,
    palette: Palette,
) -> std::sync::Arc<egui::Galley> {
    build_mysql_layout(ui, text, f32::INFINITY, palette, &[], None)
}

pub fn highlight_mysql_editor_with_matches(
    ui: &egui::Ui,
    text: &str,
    palette: Palette,
    matches: &[(usize, usize)],
    active_match: Option<usize>,
) -> std::sync::Arc<egui::Galley> {
    build_mysql_layout(ui, text, f32::INFINITY, palette, matches, active_match)
}

fn build_mysql_layout(
    ui: &egui::Ui,
    text: &str,
    wrap_width: f32,
    palette: Palette,
    matches: &[(usize, usize)],
    active_match: Option<usize>,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let font_id = FontId::new(13.0, FontFamily::Monospace);
    let mut byte_pos = 0usize;
    let mut match_idx = 0usize;
    let match_bg = palette.warning.gamma_multiply(0.32);
    let active_bg = palette.accent.gamma_multiply(0.45);

    for token in tokenize(text) {
        let color = match token.kind {
            TokenKind::Keyword => palette.sql_keyword,
            TokenKind::StringLiteral => palette.sql_string,
            TokenKind::Number => palette.sql_number,
            TokenKind::Comment => palette.sql_comment,
            TokenKind::SystemVar => palette.sql_sysvar,
            TokenKind::QuotedIdentifier => palette.icon_table,
            TokenKind::Identifier | TokenKind::Operator | TokenKind::Punctuation => palette.sql_ident,
            TokenKind::Whitespace => palette.text_primary,
        };
        let tok_start = byte_pos;
        let tok_end = tok_start + token.text.len();
        byte_pos = tok_end;

        if matches.is_empty() {
            job.append(
                token.text,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color,
                    ..Default::default()
                },
            );
            continue;
        }

        while match_idx < matches.len() && matches[match_idx].1 <= tok_start {
            match_idx += 1;
        }

        let mut cursor = tok_start;
        let mut scan = match_idx;
        while cursor < tok_end {
            while scan < matches.len() && matches[scan].1 <= cursor {
                scan += 1;
            }
            if scan >= matches.len() || matches[scan].0 >= tok_end {
                let slice = &token.text[(cursor - tok_start)..];
                job.append(
                    slice,
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color,
                        ..Default::default()
                    },
                );
                break;
            }
            let (m_start, m_end) = matches[scan];
            if cursor < m_start {
                let seg_end = m_start.min(tok_end);
                let slice = &token.text[(cursor - tok_start)..(seg_end - tok_start)];
                job.append(
                    slice,
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color,
                        ..Default::default()
                    },
                );
                cursor = seg_end;
            } else {
                let seg_end = m_end.min(tok_end);
                let slice = &token.text[(cursor - tok_start)..(seg_end - tok_start)];
                let bg = if active_match == Some(scan) { active_bg } else { match_bg };
                job.append(
                    slice,
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color,
                        background: bg,
                        ..Default::default()
                    },
                );
                cursor = seg_end;
            }
        }
    }

    ui.fonts(|fonts| fonts.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::icon;

    fn fonts_with_only(font_name: &str, font_bytes: &'static [u8]) -> egui::epaint::text::Fonts {
        let mut definitions = egui::FontDefinitions::empty();
        definitions.font_data.insert(
            font_name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(font_bytes)),
        );
        definitions
            .families
            .insert(FontFamily::Proportional, vec![font_name.to_owned()]);
        egui::epaint::text::Fonts::new(1.0, 2048, definitions)
    }

    #[test]
    fn text_fonts_never_shadow_phosphor_icon_codepoints() {
        let icon_glyphs = [
            icon::DATABASE,
            icon::GEAR_SIX,
            icon::FILE,
            icon::FILE_PLUS,
            icon::GLOBE,
            icon::CARET_DOWN,
            icon::FLOPPY_DISK,
            icon::KEY,
            icon::INFO,
            icon::FUNNEL,
        ];
        let font_id = FontId::new(13.0, FontFamily::Proportional);
        for (font_name, font_bytes) in [(UI_FONT_NAME, UI_FONT_BYTES), (CODE_FONT_NAME, CODE_FONT_BYTES)] {
            let fonts = fonts_with_only(font_name, font_bytes);
            for glyph in icon_glyphs {
                let codepoint = glyph.chars().next().expect("icon codepoint");
                assert!(
                    !fonts.has_glyph(&font_id, codepoint),
                    "{font_name} defines U+{:04X}, which hides the Phosphor icon",
                    codepoint as u32
                );
            }
            assert!(fonts.has_glyphs(&font_id, "Server 4/10 settings"));
        }
    }
}
