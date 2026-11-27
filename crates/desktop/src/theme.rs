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
const PHOSPHOR_FONT_NAME: &str = "phosphor";
const PHOSPHOR_FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Phosphor-Subset.ttf");

pub fn build_font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        UI_FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(UI_FONT_BYTES)),
    );
    fonts.font_data.insert(
        CODE_FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(CODE_FONT_BYTES)),
    );
    fonts.font_data.insert(
        PHOSPHOR_FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(PHOSPHOR_FONT_BYTES)),
    );
    fonts.families.insert(
        FontFamily::Proportional,
        vec![
            UI_FONT_NAME.to_owned(),
            PHOSPHOR_FONT_NAME.to_owned(),
            CODE_FONT_NAME.to_owned(),
        ],
    );
    fonts.families.insert(
        FontFamily::Monospace,
        vec![
            CODE_FONT_NAME.to_owned(),
            PHOSPHOR_FONT_NAME.to_owned(),
            UI_FONT_NAME.to_owned(),
        ],
    );
    fonts
}

pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(build_font_definitions());
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SqlHighlightKey {
    sql_hash: u64,
    sql_len: usize,
    wrap_width_bits: u32,
    is_dark: bool,
    matches_hash: u64,
    fold_hash: u64,
    pixels_per_point_bits: u32,
}

impl SqlHighlightKey {
    fn compute(
        text: &str,
        wrap_width: f32,
        palette: Palette,
        matches: &[(usize, usize)],
        active_match: Option<usize>,
        fold_hash: u64,
        pixels_per_point: f32,
    ) -> Self {
        Self {
            sql_hash: fnv1a64(text.as_bytes()),
            sql_len: text.len(),
            wrap_width_bits: wrap_width.to_bits(),
            is_dark: palette.bg_card.r() < 128,
            matches_hash: hash_matches(matches, active_match),
            fold_hash,
            pixels_per_point_bits: pixels_per_point.to_bits(),
        }
    }
}

#[derive(Default, Clone)]
pub struct SqlHighlightCache {
    key: Option<SqlHighlightKey>,
    galley: Option<std::sync::Arc<egui::Galley>>,
}

#[allow(dead_code)]
impl SqlHighlightCache {
    pub fn layout(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        wrap_width: f32,
        palette: Palette,
        matches: &[(usize, usize)],
        active_match: Option<usize>,
    ) -> std::sync::Arc<egui::Galley> {
        let key = SqlHighlightKey::compute(
            text,
            wrap_width,
            palette,
            matches,
            active_match,
            0,
            ui.ctx().pixels_per_point(),
        );
        if self.key.as_ref() == Some(&key) {
            if let Some(galley) = &self.galley {
                if galley.text() == text {
                    return std::sync::Arc::clone(galley);
                }
            }
        }
        let galley = build_mysql_layout(ui, text, wrap_width, palette, matches, active_match);
        self.key = Some(key);
        self.galley = Some(std::sync::Arc::clone(&galley));
        galley
    }

    pub fn layout_editor(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        palette: Palette,
        matches: &[(usize, usize)],
        active_match: Option<usize>,
    ) -> std::sync::Arc<egui::Galley> {
        self.layout(ui, text, f32::INFINITY, palette, matches, active_match)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layout_projected_editor(
        &mut self,
        ui: &egui::Ui,
        full_sql: &str,
        display_text: &str,
        doc_segments: &[(usize, usize)],
        fold_hash: u64,
        palette: Palette,
        display_matches: &[(usize, usize)],
        active_match: Option<usize>,
    ) -> std::sync::Arc<egui::Galley> {
        let key = SqlHighlightKey::compute(
            display_text,
            f32::INFINITY,
            palette,
            display_matches,
            active_match,
            fold_hash,
            ui.ctx().pixels_per_point(),
        );
        if self.key.as_ref() == Some(&key) {
            if let Some(galley) = &self.galley {
                if galley.text() == display_text {
                    return std::sync::Arc::clone(galley);
                }
            }
        }
        let galley = build_projected_mysql_layout(
            ui,
            full_sql,
            doc_segments,
            palette,
            display_matches,
            active_match,
        );
        self.key = Some(key);
        self.galley = Some(std::sync::Arc::clone(&galley));
        galley
    }

    pub fn invalidate(&mut self) {
        self.key = None;
        self.galley = None;
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn hash_matches(matches: &[(usize, usize)], active_match: Option<usize>) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    let mut feed = |v: u64| {
        for b in v.to_le_bytes() {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    feed(matches.len() as u64);
    match active_match {
        Some(idx) => {
            feed(1);
            feed(idx as u64);
        }
        None => feed(0),
    }
    for &(start, end) in matches {
        feed(start as u64);
        feed(end as u64);
    }
    hash
}

#[allow(dead_code)]
pub fn highlight_mysql_editor_cached(
    ui: &egui::Ui,
    cache_id: egui::Id,
    text: &str,
    palette: Palette,
    matches: &[(usize, usize)],
    active_match: Option<usize>,
) -> std::sync::Arc<egui::Galley> {
    let mut cache = ui
        .ctx()
        .data(|data| data.get_temp::<SqlHighlightCache>(cache_id))
        .unwrap_or_default();
    let galley = cache.layout_editor(ui, text, palette, matches, active_match);
    ui.ctx().data_mut(|data| data.insert_temp(cache_id, cache));
    galley
}

#[allow(clippy::too_many_arguments)]
pub fn highlight_sql_projected_cached(
    ui: &egui::Ui,
    cache_id: egui::Id,
    full_sql: &str,
    display_text: &str,
    doc_segments: &[(usize, usize)],
    fold_hash: u64,
    palette: Palette,
    display_matches: &[(usize, usize)],
    active_match: Option<usize>,
) -> std::sync::Arc<egui::Galley> {
    let mut cache = ui
        .ctx()
        .data(|data| data.get_temp::<SqlHighlightCache>(cache_id))
        .unwrap_or_default();
    let galley = cache.layout_projected_editor(
        ui,
        full_sql,
        display_text,
        doc_segments,
        fold_hash,
        palette,
        display_matches,
        active_match,
    );
    ui.ctx().data_mut(|data| data.insert_temp(cache_id, cache));
    galley
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

fn token_color(kind: TokenKind, palette: Palette) -> Color32 {
    match kind {
        TokenKind::Keyword => palette.sql_keyword,
        TokenKind::StringLiteral => palette.sql_string,
        TokenKind::Number => palette.sql_number,
        TokenKind::Comment => palette.sql_comment,
        TokenKind::SystemVar => palette.sql_sysvar,
        TokenKind::QuotedIdentifier => palette.icon_table,
        TokenKind::Identifier | TokenKind::Operator | TokenKind::Punctuation => palette.sql_ident,
        TokenKind::Whitespace => palette.text_primary,
    }
}

struct MatchOverlay<'a> {
    matches: &'a [(usize, usize)],
    active_match: Option<usize>,
    match_bg: Color32,
    active_bg: Color32,
}

fn append_slice_with_matches(
    job: &mut egui::text::LayoutJob,
    slice: &str,
    display_start: usize,
    font_id: &FontId,
    color: Color32,
    overlay: &MatchOverlay<'_>,
    match_idx: &mut usize,
) {
    let seg_end_abs = display_start + slice.len();
    if overlay.matches.is_empty() {
        job.append(
            slice,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color,
                ..Default::default()
            },
        );
        return;
    }

    while *match_idx < overlay.matches.len() && overlay.matches[*match_idx].1 <= display_start {
        *match_idx += 1;
    }

    let mut cursor = display_start;
    let mut scan = *match_idx;
    while cursor < seg_end_abs {
        while scan < overlay.matches.len() && overlay.matches[scan].1 <= cursor {
            scan += 1;
        }
        if scan >= overlay.matches.len() || overlay.matches[scan].0 >= seg_end_abs {
            let sub = &slice[(cursor - display_start)..];
            job.append(
                sub,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color,
                    ..Default::default()
                },
            );
            break;
        }
        let (m_start, m_end) = overlay.matches[scan];
        if cursor < m_start {
            let piece_end = m_start.min(seg_end_abs);
            let sub = &slice[(cursor - display_start)..(piece_end - display_start)];
            job.append(
                sub,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color,
                    ..Default::default()
                },
            );
            cursor = piece_end;
        } else {
            let piece_end = m_end.min(seg_end_abs);
            let sub = &slice[(cursor - display_start)..(piece_end - display_start)];
            let bg = if overlay.active_match == Some(scan) {
                overlay.active_bg
            } else {
                overlay.match_bg
            };
            job.append(
                sub,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color,
                    background: bg,
                    ..Default::default()
                },
            );
            cursor = piece_end;
        }
    }
}

fn build_projected_mysql_layout(
    ui: &egui::Ui,
    full_sql: &str,
    doc_segments: &[(usize, usize)],
    palette: Palette,
    display_matches: &[(usize, usize)],
    active_match: Option<usize>,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let font_id = FontId::new(13.0, FontFamily::Monospace);
    let overlay = MatchOverlay {
        matches: display_matches,
        active_match,
        match_bg: palette.warning.gamma_multiply(0.32),
        active_bg: palette.accent.gamma_multiply(0.45),
    };
    let tokens = tokenize(full_sql);

    let mut tok_idx = 0usize;
    let mut match_idx = 0usize;
    let mut display_byte_pos = 0usize;

    for (seg_idx, &(seg_doc_start, seg_doc_end)) in doc_segments.iter().enumerate() {
        if seg_idx > 0 {
            job.append(
                "\n",
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color: palette.text_primary,
                    ..Default::default()
                },
            );
            display_byte_pos += 1;
        }
        while tok_idx < tokens.len() && tokens[tok_idx].end <= seg_doc_start {
            tok_idx += 1;
        }
        let mut scan = tok_idx;
        while scan < tokens.len() && tokens[scan].start < seg_doc_end {
            let tok = &tokens[scan];
            let lo = tok.start.max(seg_doc_start);
            let hi = tok.end.min(seg_doc_end);
            if lo < hi {
                let slice = &full_sql[lo..hi];
                let color = token_color(tok.kind, palette);
                append_slice_with_matches(
                    &mut job,
                    slice,
                    display_byte_pos,
                    &font_id,
                    color,
                    &overlay,
                    &mut match_idx,
                );
                display_byte_pos += slice.len();
            }
            if tok.end <= seg_doc_end {
                scan += 1;
            } else {
                break;
            }
        }
        tok_idx = scan;
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

    #[test]
    fn ui_glyphs_present_without_default_fonts() {
        let definitions = build_font_definitions();
        assert_eq!(definitions.font_data.len(), 3);
        let fonts = egui::epaint::text::Fonts::new(1.0, 2048, definitions);

        let special_ui_glyphs = "↑↓←→⌘⌥⇧…·•×—–✓↵─└";
        let key_icons = [
            icon::DATABASE,
            icon::TABLE,
            icon::GEAR_SIX,
            icon::ARROW_CIRCLE_UP,
            icon::DOTS_SIX_VERTICAL,
            icon::SORT_ASCENDING,
            icon::SORT_DESCENDING,
            icon::ARROWS_DOWN_UP,
            icon::FUNNEL,
            icon::MAGNIFYING_GLASS,
            icon::COPY,
            icon::ARROW_CLOCKWISE,
            icon::CHECK,
            icon::X,
            icon::WARNING,
            icon::INFO,
            icon::DOWNLOAD_SIMPLE,
            icon::ARROW_SQUARE_OUT,
            icon::SHIELD_CHECK,
            icon::SPARKLE,
            icon::PLAY,
            icon::STOP,
            icon::KEY,
            icon::LIGHTNING,
            icon::FLOPPY_DISK,
        ];

        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let font_id = FontId::new(13.0, family.clone());
            let supported = fonts.lock().fonts.font(&font_id).characters().clone();
            for ch in ' '..='~' {
                assert!(
                    supported.contains_key(&ch),
                    "missing ASCII glyph {ch:?} (U+{:04X}) in {family:?} without default_fonts",
                    ch as u32
                );
                if ch != '?' {
                    assert!(
                        fonts.has_glyph(&font_id, ch),
                        "epaint fell back on ASCII glyph {ch:?} (U+{:04X}) in {family:?}",
                        ch as u32
                    );
                }
            }
            for ch in special_ui_glyphs.chars() {
                assert!(
                    supported.contains_key(&ch) && fonts.has_glyph(&font_id, ch),
                    "missing UI glyph {ch:?} (U+{:04X}) in {family:?} without default_fonts",
                    ch as u32
                );
            }
            for glyph in key_icons {
                let cp = glyph.chars().next().expect("icon codepoint");
                assert!(
                    supported.contains_key(&cp) && fonts.has_glyph(&font_id, cp),
                    "missing Phosphor icon U+{:04X} in {family:?} without default_fonts",
                    cp as u32
                );
            }
        }
    }

    #[test]
    fn phosphor_subset_covers_all_used_icons() {
        let phosphor_fonts = fonts_with_only(PHOSPHOR_FONT_NAME, PHOSPHOR_FONT_BYTES);
        let font_id = FontId::new(13.0, FontFamily::Proportional);

        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let used_icon_names = collect_icon_names_in_dir(&src_dir);
        assert!(
            !used_icon_names.is_empty(),
            "expected to discover icon::* usages under {}",
            src_dir.display()
        );

        let mut missing = Vec::new();
        for name in used_icon_names {
            let Some(cp) = lookup_phosphor_codepoint(&name) else {
                missing.push(format!(
                    "{name} (not in verified subset table — run `python3 scripts/subset-phosphor.py` and register in theme.rs test)"
                ));
                continue;
            };
            if !phosphor_fonts.has_glyph(&font_id, cp) {
                missing.push(format!(
                    "{name} (U+{:04X} missing from assets/fonts/Phosphor-Subset.ttf — run `python3 scripts/subset-phosphor.py`)",
                    cp as u32
                ));
            }
        }
        assert!(
            missing.is_empty(),
            "Phosphor-Subset.ttf is missing used icon(s):\n  {}",
            missing.join("\n  ")
        );
    }

    #[test]
    fn sql_highlight_cache_key_distinguishes_inputs() {
        let light = AppTheme::Light.palette();
        let dark = AppTheme::Dark.palette();
        let k1 = SqlHighlightKey::compute("SELECT 1", f32::INFINITY, light, &[], None, 0, 2.0);
        let k2 = SqlHighlightKey::compute("SELECT 1", f32::INFINITY, light, &[], None, 0, 2.0);
        assert_eq!(k1, k2);
        assert_ne!(
            k1,
            SqlHighlightKey::compute("SELECT 2", f32::INFINITY, light, &[], None, 0, 2.0)
        );
        assert_ne!(
            k1,
            SqlHighlightKey::compute("SELECT 1", 400.0, light, &[], None, 0, 2.0)
        );
        assert_ne!(
            k1,
            SqlHighlightKey::compute("SELECT 1", f32::INFINITY, dark, &[], None, 0, 2.0)
        );
        assert_ne!(
            k1,
            SqlHighlightKey::compute("SELECT 1", f32::INFINITY, light, &[(0, 6)], Some(0), 0, 2.0)
        );
        assert_ne!(
            k1,
            SqlHighlightKey::compute("SELECT 1", f32::INFINITY, light, &[], None, 42, 2.0)
        );
        assert_ne!(
            k1,
            SqlHighlightKey::compute("SELECT 1", f32::INFINITY, light, &[], None, 0, 1.0)
        );
    }

    fn collect_icon_names_in_dir(root: &std::path::Path) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        extract_icon_names(&contents, &mut names);
                    }
                }
            }
        }
        names
    }

    fn extract_icon_names(src: &str, out: &mut std::collections::BTreeSet<String>) {
        let bytes = src.as_bytes();
        let needle = b"icon::";
        let mut i = 0;
        while i + needle.len() < bytes.len() {
            if &bytes[i..i + needle.len()] == needle {
                let prev_is_ident = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
                if !prev_is_ident {
                    let start = i + needle.len();
                    let mut end = start;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_uppercase()
                            || bytes[end].is_ascii_digit()
                            || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    if end > start {
                        let name = &src[start..end];
                        if name.bytes().any(|b| b.is_ascii_uppercase()) {
                            out.insert(name.to_string());
                        }
                    }
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
    }

    fn lookup_phosphor_codepoint(name: &str) -> Option<char> {
        let glyph = match name {
            "ARROWS_CLOCKWISE" => icon::ARROWS_CLOCKWISE,
            "ARROWS_DOWN_UP" => icon::ARROWS_DOWN_UP,
            "ARROW_CIRCLE_UP" => icon::ARROW_CIRCLE_UP,
            "ARROW_CLOCKWISE" => icon::ARROW_CLOCKWISE,
            "ARROW_COUNTER_CLOCKWISE" => icon::ARROW_COUNTER_CLOCKWISE,
            "ARROW_DOWN" => icon::ARROW_DOWN,
            "ARROW_RIGHT" => icon::ARROW_RIGHT,
            "ARROW_SQUARE_OUT" => icon::ARROW_SQUARE_OUT,
            "ARROW_UP" => icon::ARROW_UP,
            "ARROW_U_UP_LEFT" => icon::ARROW_U_UP_LEFT,
            "AT" => icon::AT,
            "BINARY" => icon::BINARY,
            "BOOKMARK_SIMPLE" => icon::BOOKMARK_SIMPLE,
            "BRACKETS_CURLY" => icon::BRACKETS_CURLY,
            "CALENDAR_BLANK" => icon::CALENDAR_BLANK,
            "CARET_DOWN" => icon::CARET_DOWN,
            "CARET_LEFT" => icon::CARET_LEFT,
            "CARET_RIGHT" => icon::CARET_RIGHT,
            "CARET_UP" => icon::CARET_UP,
            "CHART_BAR" => icon::CHART_BAR,
            "CHECK" => icon::CHECK,
            "CHECK_CIRCLE" => icon::CHECK_CIRCLE,
            "CLOCK" => icon::CLOCK,
            "CLOCK_COUNTER_CLOCKWISE" => icon::CLOCK_COUNTER_CLOCKWISE,
            "CLOUD" => icon::CLOUD,
            "CODE" => icon::CODE,
            "COLUMNS" => icon::COLUMNS,
            "COPY" => icon::COPY,
            "DATABASE" => icon::DATABASE,
            "DOTS_SIX_VERTICAL" => icon::DOTS_SIX_VERTICAL,
            "DOWNLOAD_SIMPLE" => icon::DOWNLOAD_SIMPLE,
            "EXPORT" => icon::EXPORT,
            "EYE" => icon::EYE,
            "EYE_SLASH" => icon::EYE_SLASH,
            "FAST_FORWARD" => icon::FAST_FORWARD,
            "FILE" => icon::FILE,
            "FILE_PLUS" => icon::FILE_PLUS,
            "FLOPPY_DISK" => icon::FLOPPY_DISK,
            "FLOW_ARROW" => icon::FLOW_ARROW,
            "FOLDER" => icon::FOLDER,
            "FOLDER_OPEN" => icon::FOLDER_OPEN,
            "FUNCTION" => icon::FUNCTION,
            "FUNNEL" => icon::FUNNEL,
            "GAUGE" => icon::GAUGE,
            "GEAR" => icon::GEAR,
            "GEAR_SIX" => icon::GEAR_SIX,
            "GLOBE" => icon::GLOBE,
            "HAND_PALM" => icon::HAND_PALM,
            "HASH" => icon::HASH,
            "INFO" => icon::INFO,
            "KEY" => icon::KEY,
            "LIGHTNING" => icon::LIGHTNING,
            "LINK" => icon::LINK,
            "LIST" => icon::LIST,
            "LIST_BULLETS" => icon::LIST_BULLETS,
            "LOCK_KEY" => icon::LOCK_KEY,
            "MAGIC_WAND" => icon::MAGIC_WAND,
            "MAGNIFYING_GLASS" => icon::MAGNIFYING_GLASS,
            "MEMORY" => icon::MEMORY,
            "MINUS" => icon::MINUS,
            "MOON" => icon::MOON,
            "PALETTE" => icon::PALETTE,
            "PENCIL_SIMPLE" => icon::PENCIL_SIMPLE,
            "PLAY" => icon::PLAY,
            "PLUG" => icon::PLUG,
            "PLUGS_CONNECTED" => icon::PLUGS_CONNECTED,
            "PLUS" => icon::PLUS,
            "PUSH_PIN" => icon::PUSH_PIN,
            "ROWS" => icon::ROWS,
            "SELECTION" => icon::SELECTION,
            "SHIELD_CHECK" => icon::SHIELD_CHECK,
            "SLIDERS" => icon::SLIDERS,
            "SORT_ASCENDING" => icon::SORT_ASCENDING,
            "SORT_DESCENDING" => icon::SORT_DESCENDING,
            "SPARKLE" => icon::SPARKLE,
            "SQUARES_FOUR" => icon::SQUARES_FOUR,
            "STACK" => icon::STACK,
            "STOP" => icon::STOP,
            "SUN" => icon::SUN,
            "TABLE" => icon::TABLE,
            "TAG" => icon::TAG,
            "TERMINAL_WINDOW" => icon::TERMINAL_WINDOW,
            "TEXT_AA" => icon::TEXT_AA,
            "TEXT_T" => icon::TEXT_T,
            "TOGGLE_LEFT" => icon::TOGGLE_LEFT,
            "TRASH" => icon::TRASH,
            "TREE_STRUCTURE" => icon::TREE_STRUCTURE,
            "UPLOAD_SIMPLE" => icon::UPLOAD_SIMPLE,
            "WARNING" => icon::WARNING,
            "X" => icon::X,
            "X_CIRCLE" => icon::X_CIRCLE,
            _ => return None,
        };
        glyph.chars().next()
    }
}
