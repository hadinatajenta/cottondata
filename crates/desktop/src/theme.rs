use cottondata_sql::{tokenize, TokenKind};
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextFormat, Visuals};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassTheme {
    DarkGlass,
    LightGlass,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg_canvas: Color32,
    pub bg_panel: Color32,
    pub bg_surface: Color32,
    pub bg_elevated: Color32,
    pub glass_border: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
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

impl GlassTheme {
    pub fn palette(self) -> Palette {
        match self {
            Self::DarkGlass => Palette {
                bg_canvas: Color32::from_rgb(10, 17, 32),
                bg_panel: Color32::from_rgb(15, 24, 44),
                bg_surface: Color32::from_rgb(20, 32, 56),
                bg_elevated: Color32::from_rgb(27, 42, 72),
                glass_border: Color32::from_rgba_unmultiplied(186, 216, 255, 34),
                text_primary: Color32::from_rgb(236, 244, 255),
                text_muted: Color32::from_rgb(142, 164, 198),
                accent: Color32::from_rgb(98, 182, 255),
                accent_soft: Color32::from_rgba_unmultiplied(98, 182, 255, 36),
                success: Color32::from_rgb(82, 206, 156),
                warning: Color32::from_rgb(245, 188, 78),
                danger: Color32::from_rgb(244, 102, 114),
                row_alt: Color32::from_rgba_unmultiplied(255, 255, 255, 6),
                cell_modified: Color32::from_rgba_unmultiplied(245, 188, 78, 42),
                cell_inserted: Color32::from_rgba_unmultiplied(82, 206, 156, 38),
                cell_deleted: Color32::from_rgba_unmultiplied(244, 102, 114, 42),
                sql_keyword: Color32::from_rgb(112, 192, 255),
                sql_string: Color32::from_rgb(122, 218, 168),
                sql_number: Color32::from_rgb(246, 194, 104),
                sql_comment: Color32::from_rgb(118, 138, 168),
                sql_ident: Color32::from_rgb(218, 232, 255),
                sql_sysvar: Color32::from_rgb(198, 156, 255),
            },
            Self::LightGlass => Palette {
                bg_canvas: Color32::from_rgb(236, 242, 250),
                bg_panel: Color32::from_rgb(245, 249, 255),
                bg_surface: Color32::from_rgb(252, 254, 255),
                bg_elevated: Color32::from_rgb(228, 238, 252),
                glass_border: Color32::from_rgba_unmultiplied(28, 64, 128, 34),
                text_primary: Color32::from_rgb(18, 30, 52),
                text_muted: Color32::from_rgb(92, 112, 144),
                accent: Color32::from_rgb(24, 114, 224),
                accent_soft: Color32::from_rgba_unmultiplied(24, 114, 224, 28),
                success: Color32::from_rgb(26, 146, 98),
                warning: Color32::from_rgb(186, 122, 16),
                danger: Color32::from_rgb(206, 52, 68),
                row_alt: Color32::from_rgba_unmultiplied(20, 48, 96, 8),
                cell_modified: Color32::from_rgba_unmultiplied(240, 178, 52, 56),
                cell_inserted: Color32::from_rgba_unmultiplied(64, 196, 138, 48),
                cell_deleted: Color32::from_rgba_unmultiplied(232, 84, 98, 48),
                sql_keyword: Color32::from_rgb(18, 92, 206),
                sql_string: Color32::from_rgb(22, 132, 78),
                sql_number: Color32::from_rgb(176, 102, 12),
                sql_comment: Color32::from_rgb(122, 138, 162),
                sql_ident: Color32::from_rgb(22, 34, 56),
                sql_sysvar: Color32::from_rgb(126, 58, 198),
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let p = self.palette();
        let mut visuals = match self {
            Self::DarkGlass => Visuals::dark(),
            Self::LightGlass => Visuals::light(),
        };

        visuals.panel_fill = p.bg_panel;
        visuals.window_fill = p.bg_surface;
        visuals.extreme_bg_color = p.bg_canvas;
        visuals.faint_bg_color = p.row_alt;
        visuals.code_bg_color = p.bg_canvas;
        visuals.window_stroke = Stroke::new(1.0, p.glass_border);
        visuals.window_corner_radius = CornerRadius::same(12);
        visuals.menu_corner_radius = CornerRadius::same(10);
        visuals.selection.bg_fill = p.accent_soft;
        visuals.selection.stroke = Stroke::new(1.0, p.accent);

        visuals.widgets.noninteractive.bg_fill = p.bg_panel;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.glass_border);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text_primary);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(8);

        visuals.widgets.inactive.bg_fill = p.bg_surface;
        visuals.widgets.inactive.weak_bg_fill = p.bg_surface;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.glass_border);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text_primary);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(8);

        visuals.widgets.hovered.bg_fill = p.bg_elevated;
        visuals.widgets.hovered.weak_bg_fill = p.bg_elevated;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text_primary);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(8);

        visuals.widgets.active.bg_fill = p.accent_soft;
        visuals.widgets.active.bg_stroke = Stroke::new(1.2, p.accent);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, p.text_primary);
        visuals.widgets.active.corner_radius = CornerRadius::same(8);

        ctx.set_visuals(visuals);
    }
}

pub fn highlight_mysql_layout(
    ui: &egui::Ui,
    text: &str,
    wrap_width: f32,
    palette: Palette,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let font_id = FontId::new(13.5, FontFamily::Monospace);

    for tok in tokenize(text) {
        let color = match tok.kind {
            TokenKind::Keyword => palette.sql_keyword,
            TokenKind::StringLiteral => palette.sql_string,
            TokenKind::Number => palette.sql_number,
            TokenKind::Comment => palette.sql_comment,
            TokenKind::SystemVar => palette.sql_sysvar,
            TokenKind::QuotedIdentifier => palette.accent,
            TokenKind::Identifier | TokenKind::Operator | TokenKind::Punctuation => palette.sql_ident,
            TokenKind::Whitespace => palette.text_primary,
        };
        job.append(
            tok.text,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color,
                ..Default::default()
            },
        );
    }

    ui.fonts(|f| f.layout_job(job))
}
