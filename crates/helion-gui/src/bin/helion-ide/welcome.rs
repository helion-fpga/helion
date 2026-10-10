//! Welcome / first-run page (P0-6): VS Code Welcome + Vivado Start Page.
//! Shown in the center pane when no project or source is open.

use eframe::egui::{self, RichText};

use super::theme::{font_ui, font_ui_medium, Theme};
use super::HelionIde;

/// True when the IDE launched empty: no sources, no project.
pub fn should_show(app: &HelionIde) -> bool {
    !app.welcome_dismissed
        && app.model.tree.sources.is_empty()
        && app.model.last_project_path.is_none()
}

pub fn paint_welcome(ui: &mut egui::Ui, app: &mut HelionIde, theme: &Theme) {
    // Dismiss on any open action.
    let mut dismiss = false;
    let mut open_path: Option<std::path::PathBuf> = None;
    let mut run_gold = false;

    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        // Wordmark.
        ui.label(
            RichText::new("HELION")
                .font(font_ui_medium(34.0))
                .color(theme.accent),
        );
        ui.label(RichText::new("FPGA CAD, from RTL to bitstream").font(font_ui(14.0)).color(theme.dim));
        ui.add_space(8.0);
        ui.label(
            RichText::new("Pack · PathFinder place & route · STA · bitstream · simulate · program")
                .font(font_ui(12.0))
                .color(theme.faint),
        );
        ui.add_space(28.0);

        // Primary actions.
        ui.horizontal(|ui| {
            if ui
                .add_sized(
                    [180.0, 40.0],
                    egui::Button::new(RichText::new("Open file…").font(font_ui(14.0))),
                )
                .clicked()
            {
                super::native_open(app);
                dismiss = true;
            }
            if ui
                .add_sized(
                    [180.0, 40.0],
                    egui::Button::new(RichText::new("New project…").font(font_ui(14.0))),
                )
                .clicked()
            {
                app.show_create_project = true;
                dismiss = true;
            }
        });
        ui.add_space(8.0);
        if ui
            .add_sized(
                [372.0, 40.0],
                egui::Button::new(
                    RichText::new("▶  Run the gold flow (counter.sv → bitstream)")
                        .font(font_ui(14.0))
                        .color(theme.accent),
                ),
            )
            .clicked()
        {
            run_gold = true;
            dismiss = true;
        }
        ui.add_space(24.0);

        // Examples row.
        ui.label(RichText::new("Examples").font(font_ui(12.0)).color(theme.dim));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (label, file) in helion_gui::chrome::RAIL_OPEN_SOURCES {
                if ui.small_button(label).clicked() {
                    open_path =
                        Some(helion_device::Device::examples_dir().join(file));
                    dismiss = true;
                }
            }
        });
        ui.add_space(20.0);

        // Recent projects.
        if !app.recent.is_empty() {
            ui.label(RichText::new("Recent").font(font_ui(12.0)).color(theme.dim));
            ui.add_space(4.0);
            for path in app.recent.iter().take(5).cloned().collect::<Vec<_>>() {
                let label = path.display().to_string();
                if ui
                    .small_button(RichText::new(label).color(theme.accent_dim))
                    .clicked()
                {
                    open_path = Some(path);
                    dismiss = true;
                }
            }
            ui.add_space(16.0);
        }

        ui.label(
            RichText::new("Docs: helion-fpga.github.io/helion")
                .font(font_ui(12.0))
                .color(theme.faint),
        );
        ui.label(
            RichText::new("Press Ctrl+P for the command palette · ? for shortcuts")
                .font(font_ui(12.0))
                .color(theme.faint),
        );
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
