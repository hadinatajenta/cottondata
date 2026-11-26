use crate::theme::Palette;
use crate::ui_kit::{
    banner, busy_indicator, caption, icon, inset_frame, modal_shell, pill, primary_button, secondary_button,
    section_heading, ModalConfig,
};
use crate::updater::{ReleaseInfo, UpdateInstallMode, CURRENT_VERSION, GITHUB_RELEASES_PAGE};
use eframe::egui::{self, RichText};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate {
        checked_at: String,
    },
    Available(ReleaseInfo),
    Downloading {
        info: ReleaseInfo,
        downloaded: u64,
        total: u64,
    },
    Applying(ReleaseInfo),
    DownloadedPackage {
        path: PathBuf,
        install_command: String,
    },
    Failed(String),
}

pub struct UpdateModalState {
    pub open: bool,
    pub status: UpdateStatus,
    pub cancel_flag: Arc<AtomicBool>,
}

impl Default for UpdateModalState {
    fn default() -> Self {
        Self {
            open: false,
            status: UpdateStatus::Idle,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Debug, Clone)]
pub enum UpdateModalAction {
    None,
    CheckNow,
    StartDownload(Box<ReleaseInfo>),
    CancelDownload,
    OpenUrl(String),
    CopyCommand(String),
}

pub fn render_update_modal(
    ctx: &egui::Context,
    state: &mut UpdateModalState,
    p: &Palette,
) -> UpdateModalAction {
    if !state.open {
        return UpdateModalAction::None;
    }

    let palette = *p;
    let is_busy = matches!(
        state.status,
        UpdateStatus::Downloading { .. } | UpdateStatus::Applying(_)
    );
    let is_applying = matches!(state.status, UpdateStatus::Applying(_));
    let (icon_glyph, icon_color, accent_bar) = match &state.status {
        UpdateStatus::UpToDate { .. } | UpdateStatus::DownloadedPackage { .. } => {
            (icon::CHECK_CIRCLE, palette.success, Some(palette.success))
        }
        UpdateStatus::Failed(_) => (icon::WARNING, palette.danger, Some(palette.danger)),
        UpdateStatus::Available(_) | UpdateStatus::Downloading { .. } | UpdateStatus::Applying(_) => {
            (icon::ARROW_CIRCLE_UP, palette.accent, Some(palette.accent))
        }
        UpdateStatus::Idle | UpdateStatus::Checking => (icon::ARROW_CIRCLE_UP, palette.accent, None),
    };

    let subtitle = format!("Current version: v{CURRENT_VERSION}");
    let status_snapshot = state.status.clone();
    let mut action = UpdateModalAction::None;
    let mut close_clicked = false;

    let outcome = modal_shell(
        ctx,
        palette,
        ModalConfig {
            id: "software_update_modal",
            icon_glyph,
            icon_color,
            title: "Software Update",
            subtitle: Some(&subtitle),
            accent_bar,
            width: 520.0,
            max_height: None,
            dismiss_on_backdrop: !is_busy,
        },
        |ui| match &status_snapshot {
            UpdateStatus::Idle => {
                caption(
                    ui,
                    palette,
                    "Check GitHub Releases for the latest signed CottonData build.",
                );
            }
            UpdateStatus::Checking => {
                ui.horizontal(|ui| {
                    busy_indicator(ui, palette, "Checking GitHub Releases for updates…");
                });
            }
            UpdateStatus::UpToDate { checked_at } => {
                banner(
                    ui,
                    palette,
                    palette.success,
                    icon::CHECK_CIRCLE,
                    &format!("CottonData v{CURRENT_VERSION} is up to date."),
                    false,
                );
                if !checked_at.is_empty() {
                    ui.add_space(8.0);
                    caption(ui, palette, format!("Last checked: {checked_at}"));
                }
            }
            UpdateStatus::Available(info) => {
                render_version_header(ui, palette, info);
                ui.add_space(10.0);
                match &info.install_mode {
                    UpdateInstallMode::ManualOnly { reason } => {
                        banner(ui, palette, palette.warning, icon::INFO, reason, false);
                        ui.add_space(10.0);
                    }
                    UpdateInstallMode::LinuxSystemPackage { deb_arch } => {
                        banner(
                            ui,
                            palette,
                            palette.accent,
                            icon::SHIELD_CHECK,
                            &format!(
                                "Will download and SHA-256 verify the {deb_arch} .deb package for installation via apt."
                            ),
                            false,
                        );
                        ui.add_space(10.0);
                    }
                    _ => {
                        if info.asset_url.is_none() || info.checksums_url.is_none() {
                            banner(
                                ui,
                                palette,
                                palette.warning,
                                icon::WARNING,
                                "This release does not include a verified package for your platform. Download directly from GitHub Releases.",
                                false,
                            );
                            ui.add_space(10.0);
                        }
                    }
                }
                render_release_notes(ui, palette, &info.release_notes);
            }
            UpdateStatus::Downloading {
                info,
                downloaded,
                total,
            } => {
                render_version_header(ui, palette, info);
                ui.add_space(12.0);
                let progress = if *total > 0 {
                    (*downloaded as f32 / *total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let pct = (progress * 100.0).round() as u32;
                ui.add(
                    egui::ProgressBar::new(progress)
                        .desired_width(ui.available_width())
                        .desired_height(10.0)
                        .fill(palette.accent),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let progress_label = if *total > 0 {
                        format!(
                            "{} / {} ({pct}%)",
                            format_bytes_mb(*downloaded),
                            format_bytes_mb(*total)
                        )
                    } else {
                        format!("{} downloaded", format_bytes_mb(*downloaded))
                    };
                    ui.label(
                        RichText::new(progress_label)
                            .size(12.0)
                            .strong()
                            .color(palette.text_primary),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        caption(
                            ui,
                            palette,
                            info.asset_name.as_deref().unwrap_or("release package"),
                        );
                    });
                });
                ui.add_space(6.0);
                caption(
                    ui,
                    palette,
                    "SHA-256 checksum will be verified automatically before installation.",
                );
            }
            UpdateStatus::Applying(info) => {
                render_version_header(ui, palette, info);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    busy_indicator(
                        ui,
                        palette,
                        &format!("Applying {} and relaunching CottonData…", info.tag_name),
                    );
                });
            }
            UpdateStatus::DownloadedPackage {
                path,
                install_command,
            } => {
                banner(
                    ui,
                    palette,
                    palette.success,
                    icon::SHIELD_CHECK,
                    "Package downloaded and SHA-256 verified.",
                    false,
                );
                ui.add_space(10.0);
                caption(ui, palette, format!("Saved to: {}", path.display()));
                ui.add_space(8.0);
                section_heading(ui, palette, "Install Command");
                ui.add_space(4.0);
                inset_frame(palette).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        RichText::new(install_command)
                            .monospace()
                            .size(12.0)
                            .color(palette.text_primary),
                    );
                });
            }
            UpdateStatus::Failed(err) => {
                banner(ui, palette, palette.danger, icon::WARNING, err, false);
                ui.add_space(8.0);
                caption(
                    ui,
                    palette,
                    "You can retry the check or download the release directly from GitHub.",
                );
            }
        },
        |ui| match &status_snapshot {
            UpdateStatus::Idle | UpdateStatus::UpToDate { .. } => {
                if secondary_button(ui, palette, icon::ARROW_SQUARE_OUT, "Open on GitHub", true).clicked() {
                    action = UpdateModalAction::OpenUrl(GITHUB_RELEASES_PAGE.to_string());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, palette, icon::ARROW_CLOCKWISE, "Check Again", true).clicked() {
                        action = UpdateModalAction::CheckNow;
                    }
                    if secondary_button(ui, palette, icon::X, "Close", true).clicked() {
                        close_clicked = true;
                    }
                });
            }
            UpdateStatus::Checking => {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, palette, icon::X, "Close", true).clicked() {
                        close_clicked = true;
                    }
                });
            }
            UpdateStatus::Available(info) => {
                let can_install = info.asset_url.is_some()
                    && info.checksums_url.is_some()
                    && !matches!(info.install_mode, UpdateInstallMode::ManualOnly { .. });
                if can_install
                    && secondary_button(ui, palette, icon::ARROW_SQUARE_OUT, "Open on GitHub", true).clicked()
                {
                    action = UpdateModalAction::OpenUrl(info.html_url.clone());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if can_install {
                        let cta_label = match info.install_mode {
                            UpdateInstallMode::LinuxSystemPackage { .. } => "Download .deb",
                            _ => "Download & Update",
                        };
                        if primary_button(ui, palette, icon::DOWNLOAD_SIMPLE, cta_label, true).clicked() {
                            state.cancel_flag.store(false, Ordering::Relaxed);
                            action = UpdateModalAction::StartDownload(Box::new(info.clone()));
                        }
                    } else if primary_button(ui, palette, icon::ARROW_SQUARE_OUT, "Open on GitHub", true).clicked() {
                        action = UpdateModalAction::OpenUrl(info.html_url.clone());
                    }
                    if secondary_button(ui, palette, icon::X, "Close", true).clicked() {
                        close_clicked = true;
                    }
                });
            }
            UpdateStatus::Downloading { .. } => {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, palette, icon::X, "Cancel", true).clicked() {
                        state.cancel_flag.store(true, Ordering::Relaxed);
                        action = UpdateModalAction::CancelDownload;
                    }
                });
            }
            UpdateStatus::Applying(_) => {
                caption(ui, palette, "Please wait while the update finishes…");
            }
            UpdateStatus::DownloadedPackage { install_command, .. } => {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, palette, icon::COPY, "Copy apt command", true).clicked() {
                        action = UpdateModalAction::CopyCommand(install_command.clone());
                    }
                    if secondary_button(ui, palette, icon::X, "Close", true).clicked() {
                        close_clicked = true;
                    }
                });
            }
            UpdateStatus::Failed(_) => {
                if secondary_button(ui, palette, icon::ARROW_SQUARE_OUT, "Open on GitHub", true).clicked() {
                    action = UpdateModalAction::OpenUrl(GITHUB_RELEASES_PAGE.to_string());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, palette, icon::ARROW_CLOCKWISE, "Check Again", true).clicked() {
                        action = UpdateModalAction::CheckNow;
                    }
                    if secondary_button(ui, palette, icon::X, "Close", true).clicked() {
                        close_clicked = true;
                    }
                });
            }
        },
    );

    if !is_applying && (close_clicked || (outcome.closed && !is_busy)) {
        state.open = false;
    } else if outcome.closed && matches!(state.status, UpdateStatus::Downloading { .. }) {
        state.cancel_flag.store(true, Ordering::Relaxed);
        action = UpdateModalAction::CancelDownload;
    }

    action
}

fn render_version_header(ui: &mut egui::Ui, palette: Palette, info: &ReleaseInfo) {
    ui.horizontal(|ui| {
        pill(ui, &format!("v{CURRENT_VERSION}"), palette.text_muted);
        ui.label(RichText::new("→").size(13.0).color(palette.text_muted));
        pill(ui, &info.tag_name, palette.accent);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut meta = Vec::new();
            if info.asset_size > 0 {
                meta.push(format_bytes_mb(info.asset_size));
            }
            if let Some(date) = info.published_at.split('T').next().filter(|d| !d.is_empty()) {
                meta.push(date.to_string());
            }
            if !meta.is_empty() {
                caption(ui, palette, meta.join(" · "));
            }
        });
    });
}

fn render_release_notes(ui: &mut egui::Ui, palette: Palette, notes: &str) {
    section_heading(ui, palette, "Release Notes");
    ui.add_space(6.0);
    inset_frame(palette).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        egui::ScrollArea::vertical()
            .id_salt("update_release_notes_scroll")
            .max_height(220.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let trimmed = notes.trim();
                if trimmed.is_empty() {
                    caption(ui, palette, "No release notes provided for this version.");
                    return;
                }
                for line in trimmed.lines() {
                    let s = line.trim();
                    if s.is_empty() {
                        ui.add_space(4.0);
                    } else if let Some(heading) = s
                        .strip_prefix("### ")
                        .or_else(|| s.strip_prefix("## "))
                        .or_else(|| s.strip_prefix("# "))
                    {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(heading.trim())
                                .size(12.5)
                                .strong()
                                .color(palette.text_primary),
                        );
                    } else if let Some(item) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(format!("• {}", item.trim()))
                                    .size(12.0)
                                    .color(palette.text_primary),
                            );
                        });
                    } else {
                        ui.label(RichText::new(s).size(12.0).color(palette.text_primary));
                    }
                }
            });
    });
}

fn format_bytes_mb(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}
