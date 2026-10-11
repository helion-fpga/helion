//! Welcome / first-run page (P0-6): VS Code Welcome + Vivado Start Page.
//! Shown in the center pane when no project or source is open.
//!
//! Layout: one centered column, top-anchored, strict section rhythm.
//! Hero -> single primary-actions row (gold-flow CTA emphasized) ->
//! Examples as a real card grid -> Recent as a clean list -> footer pinned
//! near the bottom. No orphan buttons, no ragged rows.

use eframe::egui::{self, RichText};

use super::theme::{font_ui, font_ui_medium, Theme};
use super::HelionIde;

/// True when the IDE launched empty: no sources, no project.
pub fn should_show(app: &HelionIde) -> bool {
    !app.welcome_dismissed
        && app.model.tree.sources.is_empty()
        && app.model.last_project_path.is_none()
}

/// (file name, one-line description) for the Examples card grid.
const EXAMPLE_CARDS: [(&str, &str); 5] = [
    ("counter.sv", "4-bit incrementer, LED = cnt[3]"),
    ("blinky.sv", "Registered inverter, LED = Q"),
    ("hier.sv", "Hierarchical multi-module blinky"),
    ("complex.sv", "16x incrementers, XOR reduce"),
    ("ysyx_ibex.sv", "Ibex RISC-V core"),
];

/// Section header: small, dim, consistent across the page.
fn section_head(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .font(font_ui_medium(13.0))
            .color(theme.dim),
    );
    ui.add_space(12.0);
}

/// One example card. Fixed size; hover raises the surface, click opens it.
fn example_card(ui: &mut egui::Ui, theme: &Theme, name: &str, desc: &str, w: f32, h: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        8.0,
        if resp.hovered() {
            theme.surface_hover
        } else {
            theme.surface
        },
    );
    p.rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(
            1.0,
            if resp.hovered() {
                theme.accent_dim
            } else {
                theme.line
            },
        ),
        egui::StrokeKind::Outside,
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(14.0, 12.0)))
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
        |ui| {
            ui.label(
                RichText::new(name)
                    .font(font_ui_medium(13.5))
                    .color(theme.text),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(desc).font(font_ui(11.5)).color(theme.dim));
        },
    );
    let resp = resp.on_hover_text(format!("Open examples/{name}"));
    resp.clicked()
}

/// One recent-project row: file name + dimmed parent path, full-width hover.
fn recent_row(ui: &mut egui::Ui, theme: &Theme, name: &str, parent: &str, w: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, theme.surface_hover);
    }
    let inner = rect.shrink2(egui::vec2(10.0, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).font(font_ui(13.0)).color(theme.text));
                if !parent.is_empty() {
                    ui.label(RichText::new(parent).font(font_ui(11.5)).color(theme.faint));
                }
            });
        },
    );
    resp.clicked()
}

pub fn paint_welcome(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    // Deferred actions (applied after the paint borrows end).
    let mut dismiss = false;
    let mut open_path: Option<std::path::PathBuf> = None;
    let mut run_gold = false;

    // Centered column; everything aligns to it. Vertical scroll keeps the
    // footer reachable on short windows.
    egui::ScrollArea::vertical()
        .id_salt("welcome_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let avail_w = ui.available_width();
            let col_w = 620.0_f32.min(avail_w - 64.0).max(320.0);
            let side = ((avail_w - col_w) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_min_width(col_w);
                    ui.set_max_width(col_w);
                    ui.add_space(48.0);

                    // ---- Hero (tight, centered) ----
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("HELION")
                                .font(font_ui_medium(40.0))
                                .color(theme.accent),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("FPGA CAD, from RTL to bitstream")
                                .font(font_ui(15.0))
                                .color(theme.dim),
                        );
                        ui.add_space(6.0);
                        ui.label(
                    RichText::new(
                        "Pack · PathFinder place & route · STA · bitstream · simulate · program",
                    )
                    .font(font_ui(12.0))
                    .color(theme.faint),
                );
                    });
                    ui.add_space(28.0);

                    // ---- ONE primary-actions row; the gold flow is the CTA ----
                    let gap = 12.0;
                    let bw = (col_w - 2.0 * gap) / 3.0;
                    let bh = 44.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        if ui
                            .add_sized(
                                [bw, bh],
                                egui::Button::new(RichText::new("Open file…").font(font_ui(14.0))),
                            )
                            .on_hover_text("Open an HDL source file or project")
                            .clicked()
                        {
                            super::native_open(app);
                            dismiss = true;
                        }
                        if ui
                            .add_sized(
                                [bw, bh],
                                egui::Button::new(
                                    RichText::new("New project…").font(font_ui(14.0)),
                                ),
                            )
                            .on_hover_text("Create a new Helion project")
                            .clicked()
                        {
                            app.show_create_project = true;
                            dismiss = true;
                        }
                        // Emphasized CTA: accent fill, dark label.
                        if ui
                            .add_sized(
                                [bw, bh],
                                egui::Button::new(
                                    RichText::new("▶  Run gold flow")
                                        .font(font_ui_medium(14.0))
                                        .color(theme.bg),
                                )
                                .fill(theme.accent),
                            )
                            .on_hover_text("counter.sv → synth → place → route → bitstream")
                            .clicked()
                        {
                            run_gold = true;
                            dismiss = true;
                        }
                    });
                    ui.add_space(32.0);

                    // ---- Examples: a real card grid ----
                    section_head(ui, theme, "Examples");
                    let cols = if col_w < 480.0 { 2 } else { 3 };
                    let card_gap = 12.0;
                    let card_w = (col_w - (cols as f32 - 1.0) * card_gap) / cols as f32;
                    let card_h = 84.0;
                    ui.spacing_mut().item_spacing = egui::vec2(card_gap, card_gap);
                    let mut idx = 0;
                    while idx < EXAMPLE_CARDS.len() {
                        ui.horizontal(|ui| {
                            for (file, desc) in
                                EXAMPLE_CARDS[idx..(idx + cols).min(EXAMPLE_CARDS.len())].iter()
                            {
                                // Card title: stem without the extension.
                                let title = file.trim_end_matches(".sv");
                                if example_card(ui, theme, title, desc, card_w, card_h) {
                                    open_path =
                                        Some(helion_device::Device::examples_dir().join(file));
                                    dismiss = true;
                                }
                            }
                        });
                        idx += cols;
                    }
                    // Restore default item spacing for the rows below.
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
                    ui.add_space(28.0);

                    // ---- Recent: a clean list ----
                    let recents: Vec<std::path::PathBuf> =
                        app.recent.iter().take(5).cloned().collect();
                    if !recents.is_empty() {
                        section_head(ui, theme, "Recent");
                        for path in &recents {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            let parent = path
                                .parent()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default();
                            let parent = if parent.len() > 52 {
                                format!("…{}", &parent[parent.len() - 51..])
                            } else {
                                parent
                            };
                            if recent_row(ui, theme, &name, &parent, col_w) {
                                open_path = Some(path.clone());
                                dismiss = true;
                            }
                        }
                        ui.add_space(8.0);
                    }

                    // ---- Footer pinned near the bottom; sections breathe above it ----
                    let rest = ui.available_height();
                    if rest > 72.0 {
                        ui.add_space(rest - 72.0);
                    }
                    ui.vertical_centered(|ui| {
                        ui.hyperlink_to(
                            RichText::new("Docs: helion-fpga.github.io/helion")
                                .font(font_ui(12.0))
                                .color(theme.faint),
                            "https://helion-fpga.github.io/helion",
                        );
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new("Ctrl+P command palette   ·   ? keyboard shortcuts")
                                .font(font_ui(12.0))
                                .color(theme.faint),
                        );
                    });
                    ui.add_space(24.0);
                });
            });
        });

    // Apply deferred actions outside the closure borrows.
    if dismiss {
        app.welcome_dismissed = true;
    }
    if let Some(p) = open_path {
        if p.is_file() {
            app.open_path(&p);
        }
    }
    if run_gold {
        let p = helion_device::Device::examples_dir().join("counter.sv");
        if p.is_file() {
            app.open_path(&p);
        }
        super::run_implement(app);
    }
}
