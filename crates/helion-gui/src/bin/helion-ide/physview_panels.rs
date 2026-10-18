//! Physical View 2.0 panels: Display Control (left dock), Inspector (right
//! dock), Find palette, and ruler rendering for the Device view.
//!
//! Paint code only. All model logic (layer model, ramps, slack buckets,
//! find matching, property providers) lives in `helion_gui::physview` and
//! is unit-tested there without egui.

use eframe::egui::{self, Color32, RichText, Stroke};

use super::theme;
use helion_gui::chrome;
use super::IdeModel;
use super::WorkspaceTab;
use helion_gui::physview::{
    self, FindHit, FindKind, HeatMode, InspectorKind, LayerId, LayerModel,
    RulerMeasurement, ALL_LAYERS,
};

/// Per-session Physical View 2.0 state. Lives on the app (not the model)
/// so engine-thread jobs never disturb it.
#[derive(Clone, Debug)]
pub struct PhysViewState {
    pub layers: LayerModel,
    pub show_display_control: bool,
    pub show_inspector: bool,
    pub rulers: Vec<RulerMeasurement>,
    pub ruler_tool: bool,
    /// In-progress drag anchor (tile coords) while the ruler tool is armed.
    pub ruler_anchor: Option<(u32, u32)>,
    pub find_open: bool,
    pub find_query: String,
    pub find_selected: usize,
    /// Request keyboard focus for the find query box on open.
    pub find_needs_focus: bool,
    /// Shift+click additive selection (site/net ids).
    pub multi_select: Vec<String>,
    /// Ruler measurement selected for the inspector (click a ruler line).
    pub inspector_ruler: Option<usize>,
    /// Multi-mode floorplan heatmap overlay selection.
    pub heatmap_mode: physview::FloorplanHeatmapMode,
}

impl Default for PhysViewState {
    fn default() -> Self {
        Self {
            layers: LayerModel::default(),
            show_display_control: false,
            show_inspector: false,
            rulers: Vec::new(),
            ruler_tool: false,
            ruler_anchor: None,
            find_open: false,
            find_query: String::new(),
            find_selected: 0,
            find_needs_focus: false,
            multi_select: Vec::new(),
            inspector_ruler: None,
            heatmap_mode: physview::FloorplanHeatmapMode::Off,
        }
    }
}

/// Theme color for a layer's swatch row.
pub fn layer_color(layer: LayerId, theme: &theme::Theme) -> Color32 {
    match layer {
        LayerId::ClbSites => theme.ok,
        LayerId::IobSites => theme.cat_blue,
        LayerId::BramSites => theme.violet,
        LayerId::DspSites => theme.amber,
        LayerId::ClkSites => theme.faint,
        LayerId::CongestionHeat => theme.info,
        LayerId::CriticalityHeat => theme.warn,
        LayerId::ClockRegions => theme.violet,
        LayerId::Pblocks => theme.amber,
        LayerId::Routes => theme.ok,
        LayerId::Rulers => theme.text,
    }
}

/// Display Control: layer tree with visibility checkboxes + solo buttons,
/// heat-map section, and the ruler list.
pub fn paint_display_control(ui: &mut egui::Ui, pv: &mut PhysViewState, model: &IdeModel, theme: &theme::Theme) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Display Control").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("hide").clicked() {
                pv.show_display_control = false;
            }
        });
    });
    ui.separator();
    ui.label(RichText::new("Layers").small().color(theme.faint));
    let mut solo_toggle: Option<LayerId> = None;
    for layer in ALL_LAYERS {
        let mut on = pv.layers.is_visible(layer);
        let swatch = layer_color(layer, theme);
        let solo = pv.layers.solo() == Some(layer);
        ui.horizontal(|ui| {
            // Color swatch.
            let (r, _) = ui.allocate_exact_size(egui::vec2(chrome::STATUS_DOT_SIZE, chrome::STATUS_DOT_SIZE), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, swatch);
            if ui.checkbox(&mut on, layer.label()).changed() {
                pv.layers.set_visible(layer, on);
            }
            // Solo (isolate) button.
            let label = if solo { "[S]" } else { " S " };
            if ui
                .small_button(label)
                .on_hover_text("Solo: isolate this layer, dim everything else")
                .clicked()
            {
                solo_toggle = Some(layer);
            }
        });
    }
    if let Some(layer) = solo_toggle {
        pv.layers.toggle_solo(layer);
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new("Heat maps").small().color(theme.faint));
    ui.horizontal_wrapped(|ui| {
        let mut mode = pv.layers.heat_mode;
        if ui.radio(mode == HeatMode::Off && pv.heatmap_mode == physview::FloorplanHeatmapMode::Off, "Off").clicked() {
            mode = HeatMode::Off;
            pv.heatmap_mode = physview::FloorplanHeatmapMode::Off;
        }
        if ui.radio(mode == HeatMode::Congestion || pv.heatmap_mode == physview::FloorplanHeatmapMode::RoutingCongestion, "Congestion").clicked() {
            mode = HeatMode::Congestion;
            pv.heatmap_mode = physview::FloorplanHeatmapMode::RoutingCongestion;
        }
        if ui.radio(mode == HeatMode::Criticality || pv.heatmap_mode == physview::FloorplanHeatmapMode::SetupSlack, "Setup Slack").clicked() {
            mode = HeatMode::Criticality;
            pv.heatmap_mode = physview::FloorplanHeatmapMode::SetupSlack;
        }
        if ui.radio(pv.heatmap_mode == physview::FloorplanHeatmapMode::ThermalPowerDensity, "Thermal Power").clicked() {
            mode = HeatMode::Off;
            pv.heatmap_mode = physview::FloorplanHeatmapMode::ThermalPowerDensity;
        }
        if ui.radio(pv.heatmap_mode == physview::FloorplanHeatmapMode::ControlSetUtilization, "Control Sets").clicked() {
            mode = HeatMode::Off;
            pv.heatmap_mode = physview::FloorplanHeatmapMode::ControlSetUtilization;
        }
        pv.layers.heat_mode = mode;
    });
    let mut op = pv.layers.heat_opacity;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Opacity").small());
        if ui
            .add(egui::Slider::new(&mut op, 0.10..=0.90).show_value(false))
            .changed()
        {
            pv.layers.set_heat_opacity(op);
        }
    });
    // Heat-map legend strip.
    match pv.layers.heat_mode {
        HeatMode::Congestion => {
            let max_u = model.device.congestion.iter().copied().max().unwrap_or(0);
            paint_ramp_legend(ui, theme, &format!("0 ... {max_u} nets/tile"));
        }
        HeatMode::Criticality => {
            ui.horizontal(|ui| {
                for (label, rgb) in [
                    ("ok >1ns", physview::slack_color(physview::SlackBucket::Safe)),
                    ("0..1ns", physview::slack_color(physview::SlackBucket::Watch)),
                    ("<0 viol", physview::slack_color(physview::SlackBucket::Violating)),
                ] {
                    let c = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    let (r, _) = ui.allocate_exact_size(egui::vec2(chrome::STATUS_DOT_SIZE, chrome::STATUS_DOT_SIZE), egui::Sense::hover());
                    ui.painter().rect_filled(r, 2.0, c);
                    ui.label(RichText::new(label).small().color(theme.faint));
                }
            });
            if model.device.net_slack.is_empty() {
                ui.label(RichText::new("No timing data: run STA first.").small().color(theme.faint));
            }
        }
        HeatMode::Off => {}
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new("Rulers").small().color(theme.faint));
    if pv.rulers.is_empty() {
        ui.label(RichText::new("No measurements. Arm the Ruler tool and drag on the die.").small().color(theme.faint));
    }
    let mut delete: Option<usize> = None;
    for (i, r) in pv.rulers.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("#{} {}", i + 1, r.readout())).small());
            if ui.small_button("x").on_hover_text("Delete measurement").clicked() {
                delete = Some(i);
            }
        });
    }
    if let Some(i) = delete {
        pv.rulers.remove(i);
    }
}

/// Blue to red gradient strip with min/max labels.
fn paint_ramp_legend(ui: &mut egui::Ui, theme: &theme::Theme, labels: &str) {
    ui.horizontal(|ui| {
        let steps = 24;
        let w = 120.0 / steps as f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(chrome::DISPLAY_OCCUPANCY_BAR_W, chrome::DISPLAY_OCCUPANCY_BAR_H), egui::Sense::hover());
        for i in 0..steps {
            let t = i as f32 / (steps - 1) as f32;
            let rgb = physview::congestion_ramp(t);
            let c = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left() + i as f32 * w, rect.top()),
                    egui::vec2(w + 0.5, 10.0),
                ),
                0.0,
                c,
            );
        }
        ui.label(RichText::new(labels).small().color(theme.faint));
    });
}

/// Inspector: OpenROAD-style Name/Value grid for the current selection.
pub fn paint_inspector(
    ui: &mut egui::Ui,
    model: &mut IdeModel,
    pv: &mut PhysViewState,
    theme: &theme::Theme,
    zoom_target: &mut Option<(u32, u32)>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Inspector").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("hide").clicked() {
                pv.show_inspector = false;
            }
        });
    });
    ui.separator();
    // A clicked ruler line takes precedence over object selection.
    if let Some(i) = pv.inspector_ruler {
        if let Some(m) = pv.rulers.get(i) {
            ui.label(RichText::new(format!("Ruler #{}", i + 1)).strong().monospace());
            ui.separator();
            egui::Grid::new("physview_inspector_ruler_grid")
                .num_columns(2)
                .spacing([10.0, 3.0])
                .show(ui, |ui| {
                    for (name, value) in physview::inspect_ruler(m) {
                        ui.label(RichText::new(name).small().color(theme.faint));
                        ui.label(RichText::new(value).small().monospace());
                        ui.end_row();
                    }
                });
            if ui.small_button("Delete").clicked() {
                pv.rulers.remove(i);
                pv.inspector_ruler = None;
            }
            return;
        }
        pv.inspector_ruler = None;
    }
    let kind = physview::inspector_kind_for(model);
    // Multi-selection summary takes precedence when shift+click was used.
    if !pv.multi_select.is_empty() {
        ui.label(RichText::new(format!("{} selected", pv.multi_select.len())).strong());
        for id in pv.multi_select.iter().take(12) {
            ui.label(RichText::new(id).small().monospace());
        }
        if pv.multi_select.len() > 12 {
            ui.label(RichText::new(format!("... and {}", pv.multi_select.len() - 12)).small().color(theme.faint));
        }
        if ui.small_button("Clear").clicked() {
            pv.multi_select.clear();
            model.selected = None;
        }
        return;
    }
    let header = match &kind {
        InspectorKind::Site(id) => format!("Site {id}"),
        InspectorKind::Net(net) => format!("Net {net}"),
        InspectorKind::Pblock(name) => format!("Pblock {name}"),
        InspectorKind::ClockRegion(name) => format!("Clock region {name}"),
        InspectorKind::Ruler(i) => format!("Ruler #{}", i + 1),
        InspectorKind::Empty => String::new(),
    };
    if matches!(kind, InspectorKind::Empty) {
        // Wrap so the hint is never truncated mid-word in a narrow dock.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.label(RichText::new("Click a site, net, pblock, or clock region.").color(theme.faint));
        return;
    }
    ui.label(RichText::new(header).strong().monospace());
    ui.separator();
    let rows = physview::inspect(model, &kind, &model.device.net_slack, &pv.rulers);
    egui::Grid::new("physview_inspector_grid")
        .num_columns(2)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            for (name, value) in rows {
                ui.label(RichText::new(name).small().color(theme.faint));
                ui.label(RichText::new(value).small().monospace());
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.separator();
    // Actions per kind.
    match &kind {
        InspectorKind::Site(id) => {
            let site_xy = model.device.sites.iter().find_map(|s| {
                (s.site_name() == *id
                    || s.occupant.as_deref() == Some(id.as_str())
                    || s.bels.iter().any(|b| b == id))
                .then_some((s.x, s.y))
            });
            let occupant = model
                .device
                .sites
                .iter()
                .find_map(|s| {
                    (s.site_name() == *id
                        || s.occupant.as_deref() == Some(id.as_str())
                        || s.bels.iter().any(|b| b == id))
                    .then(|| s.occupant.clone())
                })
                .flatten();
            ui.horizontal(|ui| {
                if ui.small_button("Zoom to").clicked() {
                    if let Some((x, y)) = site_xy {
                        model.device_zoom = 2.5;
                        *zoom_target = Some((x, y));
                    }
                }
                if ui.small_button("Show in Schematic").clicked() {
                    if let Some(cell) = occupant.clone() {
                        model.selected = Some(cell);
                        model.workspace = WorkspaceTab::Schematic;
                    }
                }
                if ui.small_button("Show in Hierarchy").clicked() {
                    // Grammar R15: the hierarchy tree is a Z3 sidebar section
                    // (Device activity). Selecting reveals the node there -
                    // the tree auto-expands to the selection - and the Z4
                    // inspector shows its properties.
                    if let Some(cell) = occupant {
                        model.selected = Some(cell);
                    }
                }
            });
        }
        InspectorKind::Net(net) => {
            ui.horizontal(|ui| {
                if ui.small_button("Highlight net").clicked() {
                    // Solo the route: selection drives the accent highlight
                    // through the existing highlight mechanism.
                    model.selected = Some(net.clone());
                    model.schematic.highlight_nets.insert(net.clone());
                    model.highlight_device_routes();
                }
                if ui.small_button("Cross-probe timing path").clicked() {
                    // Probe the worst-slack path through this net.
                    if let Some((idx, _)) = model
                        .timing_paths
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.nets.iter().any(|n| n == net))
                        .min_by_key(|(_, p)| p.slack_ps)
                    {
                        model.set_probe(helion_gui::ProbeSelection::Path(idx));
                    }
                }
            });
        }
        InspectorKind::Pblock(name) => {
            ui.horizontal(|ui| {
                if ui.small_button("Zoom to").clicked() {
                    if let Some(p) = model.pblocks.iter().find(|p| &p.name == name) {
                        if p.ranged {
                            model.device_zoom = 2.0;
                            let cx = (p.x0 + p.x1) / 2;
                            let cy = (p.y0 + p.y1) / 2;
                            *zoom_target = Some((cx, cy));
                        }
                    }
                }
            });
        }
        _ => {}
    }
}

/// Find palette (`Ctrl+K`): fuzzy over sites, cells, nets. Enter selects,
/// zooms to the hit, and populates the inspector.
pub fn paint_find_dialog(
    ctx: &egui::Context,
    model: &mut IdeModel,
    pv: &mut PhysViewState,
    theme: &theme::Theme,
    zoom_target: &mut Option<(u32, u32)>,
) {
    if !pv.find_open {
        return;
    }
    let mut close = false;
    let mut activate: Option<FindHit> = None;
    egui::Window::new("Find in Device")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
        .show(ctx, |ui| {
            ui.set_min_width(chrome::PROPS_GRID_MIN_W);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut pv.find_query)
                    .hint_text("site, cell, or net name")
                    .desired_width(chrome::FIND_DIALOG_QUERY_W),
            );
            // Focus the query box when the dialog opens.
            if pv.find_needs_focus {
                resp.request_focus();
                pv.find_needs_focus = false;
            }
            let hits = physview::find_matches(model, &pv.find_query, 12);
            if !hits.is_empty() {
                pv.find_selected = pv.find_selected.min(hits.len() - 1);
            }
            // Keyboard navigation.
            if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !hits.is_empty() {
                pv.find_selected = (pv.find_selected + 1) % hits.len();
            }
            if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !hits.is_empty() {
                pv.find_selected = (pv.find_selected + hits.len() - 1) % hits.len();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !hits.is_empty() {
                activate = Some(hits[pv.find_selected].clone());
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
            ui.separator();
            for (i, hit) in hits.iter().enumerate() {
                let on = i == pv.find_selected;
                let kind_label = match hit.kind {
                    FindKind::Site => "site",
                    FindKind::Cell => "cell",
                    FindKind::Net => "net",
                };
                let row = format!("[{}] {}  {}", kind_label, hit.label, hit.detail);
                if ui.selectable_label(on, RichText::new(row).small().monospace()).clicked() {
                    activate = Some(hit.clone());
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new("No matches.").small().color(theme.faint));
            }
        });
    if let Some(hit) = activate {
        apply_find_hit(model, pv, &hit, zoom_target);
        close = true;
    }
    if close {
        pv.find_open = false;
        pv.find_query.clear();
        pv.find_selected = 0;
    }
}

fn apply_find_hit(
    model: &mut IdeModel,
    pv: &mut PhysViewState,
    hit: &FindHit,
    zoom_target: &mut Option<(u32, u32)>,
) {
    pv.multi_select.clear();
    match hit.kind {
        FindKind::Site => {
            model.selected = Some(hit.label.clone());
            if let Some(s) = model.device.sites.iter().find(|s| s.site_name() == hit.label) {
                model.device_zoom = 2.5;
                *zoom_target = Some((s.x, s.y));
            }
        }
        FindKind::Cell => {
            model.selected = Some(hit.label.clone());
            if let Some(s) = model.device.sites.iter().find(|s| {
                s.occupant.as_deref() == Some(hit.label.as_str())
                    || s.bels.iter().any(|b| b == &hit.label)
            }) {
                model.device_zoom = 2.5;
                *zoom_target = Some((s.x, s.y));
            }
        }
        FindKind::Net => {
            model.selected = Some(hit.label.clone());
            if let Some(r) = model.device.routes.iter().find(|r| r.net == hit.label) {
                if let Some(&(x, y)) = r.tiles.first() {
                    model.device_zoom = 2.0;
                    *zoom_target = Some((x, y));
                }
            }
        }
    }
}

/// Paint ruler lines + readouts over the die painter.
pub fn paint_rulers(
    p: &egui::Painter,
    rulers: &[RulerMeasurement],
    draft: Option<((u32, u32), (u32, u32))>,
    to_px: &impl Fn(u32, u32) -> egui::Pos2,
    theme: &theme::Theme,
) {
    let draw = |m: &RulerMeasurement, col: Color32| {
        let a = to_px(m.x0, m.y0);
        let b = to_px(m.x1, m.y1);
        p.line_segment([a, b], Stroke::new(1.6, col));
        p.circle_filled(a, 3.0, col);
        p.circle_filled(b, 3.0, col);
        p.text(
            egui::pos2((a.x + b.x) / 2.0 + 6.0, (a.y + b.y) / 2.0 - 6.0),
            egui::Align2::LEFT_CENTER,
            m.readout(),
            egui::FontId::monospace(9.0),
            col,
        );
    };
    for m in rulers {
        draw(m, theme.text);
    }
    if let Some(((x0, y0), (x1, y1))) = draft {
        draw(
            &RulerMeasurement { x0, y0, x1, y1 },
            theme.accent,
        );
    }
}
