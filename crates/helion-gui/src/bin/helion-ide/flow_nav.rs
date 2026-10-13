//! Flow Navigator sidebar (P0-3): Vivado-style stage tree.
//!
//! Five implementation stages (Synthesis / Opt / Place / Route / Bitstream),
//! each a collapsing header with Run / Settings / Open-results rows. Stage
//! state dots come from `IdeModel::step_state`; runs are queued on the engine
//! thread via `submit_job` - never executed synchronously inside a frame.

use eframe::egui::{self, Color32, RichText, Sense};

use super::icons;
use super::theme;
use super::HelionIde;
use helion_gui::chrome;
use helion_gui::surface::JobKind;
use helion_gui::{FlowStep, StepState, WorkspaceTab};

/// Paint the Flow Navigator tree: stage headers plus Run / Settings /
/// Open-results rows. All colors come from the theme tokens.
pub fn paint_flow_nav(ui: &mut egui::Ui, app: &mut HelionIde) {
    let theme = app.theme;
    ui.label(RichText::new("Flow Navigator").strong().size(14.0));
    ui.add_space(2.0);
    if app.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new(app.progress.as_str())
                    .small()
                    .color(theme.dim),
            );
        });
        ui.add_space(4.0);
    }
    for step in FlowStep::ALL {
        paint_stage_entry(ui, app, step);
    }
}

/// One stage: clickable header (chevron + glyph + label + state dot) with a
/// collapsible Run / Settings / Open-results body.
fn paint_stage_entry(ui: &mut egui::Ui, app: &mut HelionIde, step: FlowStep) {
    let theme = app.theme;
    let state = app.model.step_state(step);
    let id = egui::Id::new(("flow_nav_stage", step as u8));
    let mut open = ui.data_mut(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), chrome::HIT_SIDEBAR),
        Sense::click(),
    );
    let resp = resp.on_hover_text(stage_hover(app, step));
    if ui.is_rect_visible(rect) {
        paint_stage_header(ui, theme, rect, step, state, open, resp.hovered());
    }
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    if open {
        paint_stage_body(ui, app, step);
    }
}

/// Hover hint for a stage header: the prerequisite blocker, or the Tcl verb.
fn stage_hover(app: &HelionIde, step: FlowStep) -> String {
    match app.model.step_blocked(step) {
        Some(why) => why.to_string(),
        None => format!("{} - {}", step.label(), step.tcl()),
    }
}

/// Header row: chevron, stage glyph, label, state dot. Pure paint, no clicks.
fn paint_stage_header(
    ui: &mut egui::Ui,
    theme: theme::Theme,
    rect: egui::Rect,
    step: FlowStep,
    state: StepState,
    open: bool,
    hovered: bool,
) {
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, 4.0, theme.surface_hover);
    }
    let cc = egui::pos2(rect.left() + 10.0, rect.center().y);
    let tri = if open {
        vec![
            egui::pos2(cc.x - 4.0, cc.y - 2.5),
            egui::pos2(cc.x + 4.0, cc.y - 2.5),
            egui::pos2(cc.x, cc.y + 3.5),
        ]
    } else {
        vec![
            egui::pos2(cc.x - 2.5, cc.y - 4.0),
            egui::pos2(cc.x - 2.5, cc.y + 4.0),
            egui::pos2(cc.x + 3.5, cc.y),
        ]
    };
    p.add(egui::Shape::convex_polygon(
        tri,
        theme.dim,
        egui::Stroke::NONE,
    ));
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 30.0, rect.center().y),
        egui::vec2(20.0, 20.0),
    );
    icons::paint_stage(p, icon_rect, step, theme.dim);
    p.text(
        egui::pos2(rect.left() + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        step.label(),
        theme::font_ui(13.0),
        theme.text,
    );
    let (fill, ring) = stage_dot_colors(theme, state);
    icons::paint_stage_dot(
        p,
        egui::pos2(rect.right() - 12.0, rect.center().y),
        5.0,
        fill,
        ring,
    );
}

/// `(fill, ring)` for a stage state dot. `StepState` has no Running variant,
/// so in-flight work is shown by the spinner row at the top of the tree.
fn stage_dot_colors(theme: theme::Theme, state: StepState) -> (Color32, Option<Color32>) {
    match state {
        StepState::Pending => (Color32::TRANSPARENT, Some(theme.faint)),
        StepState::Done => (theme.ok, None),
        StepState::Failed => (theme.danger, None),
    }
}

/// One full-width row button in an expanded stage body. Returns true on click.
fn stage_row(ui: &mut egui::Ui, label: &str, hover: &str, enabled: bool) -> bool {
    let mut clicked = false;
    ui.add_enabled_ui(enabled, |ui| {
        if ui
            .add_sized([ui.available_width(), 22.0], egui::Button::new(label))
            .on_hover_text(hover)
            .clicked()
        {
            clicked = true;
        }
    });
    clicked
}

/// Expanded body: Run / Settings / Open-results rows, full width so they fit
/// narrow sidebars. Actions are collected first, applied after painting.
fn paint_stage_body(ui: &mut egui::Ui, app: &mut HelionIde, step: FlowStep) {
    let state = app.model.step_state(step);
    let blocked = app.model.step_blocked(step);
    let run_label = if state == StepState::Done {
        "Re-run"
    } else {
        "Run"
    };
    let run_hover = match blocked {
        Some(why) => why.to_string(),
        None => format!("Queue {} on the engine thread", step.tcl()),
    };
    let tab = results_tab(step);
    let mut run = false;
    let mut settings = false;
    let mut results = false;
    ui.horizontal(|ui| {
        ui.add_space(38.0);
        ui.vertical(|ui| {
            ui.add_space(2.0);
            run = stage_row(ui, run_label, &run_hover, !app.busy && blocked.is_none());
            settings = stage_row(ui, "Settings", "Open Project Settings", true);
            results = stage_row(ui, "Open results", &format!("Open {}", tab.label()), true);
            ui.add_space(4.0);
        });
    });
    if run {
        // Engine-thread queue: never run a stage synchronously inside a frame.
        app.submit_job(JobKind::Step(step));
    }
    if settings {
        super::open_more_workspace(app, WorkspaceTab::Settings);
    }
    if results {
        super::open_more_workspace(app, results_tab(step));
    }
}

/// Where "Open results" lands for each stage.
fn results_tab(step: FlowStep) -> WorkspaceTab {
    match step {
        // Grammar R15: the hierarchy tree is a Z3 sidebar section now. After
        // synthesis the netlist viewer (Schematic) is the results pane; the
        // Files sidebar shows the hierarchy tree beside it.
        FlowStep::Synthesis => WorkspaceTab::Schematic,
        FlowStep::Opt => WorkspaceTab::Reports,
        FlowStep::Place => WorkspaceTab::Device,
        FlowStep::Route => WorkspaceTab::Device,
        FlowStep::Bitstream => WorkspaceTab::Bitstream,
    }
}
