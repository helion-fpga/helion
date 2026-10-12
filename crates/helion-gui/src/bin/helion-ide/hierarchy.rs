//! Hierarchy tree view: Vivado Netlist-tree pattern.
//!
//! Replaces the treemap (unreadable noise past a few hundred blocks) with a
//! real expandable tree: chevron rows, indent guides, per-node cell counts,
//! 28px rows, full-width selection highlight, and a filter field. Virtualized
//! via `ScrollArea::show_rows` so 15k-block designs stay interactive - rows
//! are only materialized for expanded paths, never all at once.

use std::collections::{HashMap, HashSet};

use eframe::egui;

use super::theme;
use super::IdeModel;

const ROW_H: f32 = 28.0;
const INDENT: f32 = 16.0;
const MAX_FILTER_ROWS: usize = 5000;

#[derive(Clone, Debug)]
struct TreeNode {
    name: String,
    short: String,
    /// "module" | "instance" | leaf kind ("LUT6", "HFF", "IOB_OUT", …).
    kind: String,
    depth: usize,
    /// Leaf cells in this subtree (instances/modules only).
    leaf_count: usize,
    children: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
struct TreeState {
    /// Rebuild the tree when the design changes.
    top: Option<String>,
    nodes: Vec<TreeNode>,
    root: usize,
    /// Parent index per node (None for root) - drives the breadcrumb bar.
    parent: Vec<Option<usize>>,
    expanded: HashSet<String>,
    filter: String,
}

impl TreeState {
    fn is_instance_kind(kind: &str) -> bool {
        kind == "module" || kind == "instance" || kind.starts_with("instance:")
    }
}

/// Longest instance-prefix parent, mirroring `HierarchyView::layout_boxes`:
/// strip trailing `_segment`s until an instance name matches.
fn parent_of<'a>(name: &'a str, inst_set: &HashSet<&'a str>) -> Option<&'a str> {
    let mut rest = name;
    while let Some((pfx, _)) = rest.rsplit_once('_') {
        if inst_set.contains(pfx) {
            return Some(pfx);
        }
        if pfx.is_empty() {
            break;
        }
        rest = pfx;
    }
    None
}

fn build_tree(top: &str, nodes: &[(String, String)]) -> (Vec<TreeNode>, usize, Vec<Option<usize>>) {
    let inst_names: HashSet<&str> = nodes
        .iter()
        .filter(|(_, k)| k.starts_with("instance:"))
        .map(|(n, _)| n.as_str())
        .collect();

    // Index 0 is always the root (top module).
    let mut tree: Vec<TreeNode> = vec![TreeNode {
        name: top.to_string(),
        short: helion_gui::schematic_short_name(top),
        kind: "module".to_string(),
        depth: 0,
        leaf_count: 0,
        children: vec![],
    }];
    let mut idx_of: HashMap<&str, usize> = HashMap::new();

    // Pass 1: instances (parents before children via longest-prefix).
    // Sort instances by name length so parents are indexed before children.
    let mut insts: Vec<&(String, String)> = nodes
        .iter()
        .filter(|(_, k)| k.starts_with("instance:"))
        .collect();
    insts.sort_by_key(|(n, _)| n.len());
    for (name, _kind) in insts {
        if idx_of.contains_key(name.as_str()) {
            continue;
        }
        let parent = parent_of(name, &inst_names)
            .and_then(|p| idx_of.get(p).copied())
            .unwrap_or(0);
        let depth = tree[parent].depth + 1;
        let idx = tree.len();
        idx_of.insert(name.as_str(), idx);
        tree.push(TreeNode {
            name: name.clone(),
            short: helion_gui::schematic_short_name(name),
            kind: "instance".to_string(),
            depth,
            leaf_count: 0,
            children: vec![],
        });
        tree[parent].children.push(idx);
    }

    // Pass 2: leaves attach to the longest instance prefix, else root.
    for (name, kind) in nodes.iter() {
        if kind == "module" || kind.starts_with("instance:") {
            continue;
        }
        // Skip the top module node itself if it appears in the list.
        if name == top {
            continue;
        }
        let parent = parent_of(name, &inst_names)
            .and_then(|p| idx_of.get(p).copied())
            .unwrap_or(0);
        let depth = tree[parent].depth + 1;
        let idx = tree.len();
        tree.push(TreeNode {
            name: name.clone(),
            short: helion_gui::schematic_short_name(name),
            kind: kind.clone(),
            depth,
            leaf_count: 1,
            children: vec![],
        });
        tree[parent].children.push(idx);
    }

    // Bottom-up leaf counts. Children always have higher indices than parents
    // (both passes append after the parent exists), so reverse order works.
    for i in (0..tree.len()).rev() {
        let kids = tree[i].children.clone();
        if !kids.is_empty() {
            tree[i].leaf_count = kids.iter().map(|&c| tree[c].leaf_count).sum();
        }
    }
    // Parent index per node for the breadcrumb bar.
    let mut parent: Vec<Option<usize>> = vec![None; tree.len()];
    for (i, n) in tree.iter().enumerate() {
        for &c in &n.children {
            parent[c] = Some(i);
        }
    }
    (tree, 0, parent)
}

/// Visible row indices in pre-order, descending only into expanded nodes.
/// With a filter, shows matches plus their ancestors (auto-expanded).
fn visible_rows(state: &TreeState) -> Vec<usize> {
    let mut out = Vec::new();
    let filt = state.filter.trim().to_lowercase();
    if filt.is_empty() {
        let mut stack = vec![state.root];
        while let Some(i) = stack.pop() {
            out.push(i);
            let n = &state.nodes[i];
            if !n.children.is_empty() && (i == state.root || state.expanded.contains(&n.name)) {
                for &c in n.children.iter().rev() {
                    stack.push(c);
                }
            }
        }
        return out;
    }
    // Filter mode: match set + ancestors, pre-order.
    let mut matched = HashSet::new();
    for (i, n) in state.nodes.iter().enumerate() {
        if n.name.to_lowercase().contains(&filt) || n.short.to_lowercase().contains(&filt) {
            matched.insert(i);
        }
    }
    if matched.is_empty() {
        return out;
    }
    // child -> parent map.
    let mut parent_of_idx: HashMap<usize, usize> = HashMap::new();
    for (i, n) in state.nodes.iter().enumerate() {
        for &c in &n.children {
            parent_of_idx.insert(c, i);
        }
    }
    let mut with_ancestors = matched.clone();
    for &m in &matched {
        let mut p = m;
        while let Some(&par) = parent_of_idx.get(&p) {
            if !with_ancestors.insert(par) {
                break;
            }
            p = par;
        }
    }
    let mut stack = vec![state.root];
    while let Some(i) = stack.pop() {
        if with_ancestors.contains(&i) {
            out.push(i);
            if out.len() >= MAX_FILTER_ROWS {
                break;
            }
        }
        for &c in state.nodes[i].children.iter().rev() {
            stack.push(c);
        }
    }
    out
}

fn paint_chevron(p: &egui::Painter, center: egui::Pos2, expanded: bool, color: egui::Color32) {
    let s = 4.0;
    let pts = if expanded {
        vec![
            egui::pos2(center.x - s, center.y - s * 0.6),
            egui::pos2(center.x + s, center.y - s * 0.6),
            egui::pos2(center.x, center.y + s * 0.7),
        ]
    } else {
        vec![
            egui::pos2(center.x - s * 0.6, center.y - s),
            egui::pos2(center.x - s * 0.6, center.y + s),
            egui::pos2(center.x + s * 0.7, center.y),
        ]
    };
    p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

/// Vivado Netlist-tree hierarchy pane. `model.selected` is the cross-view
/// selection identity shared with Schematic/Device/Properties.
pub fn paint_hierarchy(ui: &mut egui::Ui, model: &mut IdeModel, theme: &theme::Theme) {
    ui.horizontal(|ui| {
        ui.heading("Hierarchy");
        ui.add_space(12.0);
        if ui.button("Show in Schematic").clicked() {
            let _ = model.exec("open_hierarchy_sheet");
        }
    });

    let top = model.hierarchy.top.clone();
    let Some(top_name) = top.clone() else {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(egui::RichText::new("No hierarchy yet.").color(theme.dim));
            ui.label(
                egui::RichText::new("Run Synthesis to build the design tree.")
                    .small()
                    .color(theme.faint),
            );
        });
        return;
    };

    // Tree state lives in egui temp data, keyed per design.
    let state_id = egui::Id::new("hierarchy_tree_state");
    let mut state: TreeState = ui
        .ctx()
        .data_mut(|d| d.get_temp::<TreeState>(state_id))
        .unwrap_or_default();
    if state.top.as_deref() != Some(top_name.as_str()) || state.nodes.is_empty() {
        let (nodes, root, parent) = build_tree(&top_name, &model.hierarchy.nodes);
        let root_name = nodes[root].name.clone();
        state = TreeState {
            top: Some(top_name.clone()),
            nodes,
            root,
            parent,
            expanded: HashSet::from([root_name]),
            filter: String::new(),
        };
    }

    let n_instances = state
        .nodes
        .iter()
        .filter(|n| n.kind == "instance")
        .count();
    let n_leaves = state
        .nodes
        .iter()
        .filter(|n| !TreeState::is_instance_kind(&n.kind))
        .count();
    ui.label(
        egui::RichText::new(format!(
            "{top_name} · {n_instances} instances · {n_leaves} leaf cells"
        ))
        .small()
        .color(theme.faint),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Filter").small().color(theme.dim));
        ui.text_edit_singleline(&mut state.filter);
        if !state.filter.is_empty() && ui.small_button("✕").clicked() {
            state.filter.clear();
        }
    });
    ui.add_space(2.0);

    let visible = visible_rows(&state);
    let filt_active = !state.filter.trim().is_empty();
    if filt_active {
        ui.label(
            egui::RichText::new(format!(
                "{} match{}",
                visible.len(),
                if visible.len() == 1 { "" } else { "es" }
            ))
            .small()
            .color(theme.faint),
        );
    }

    // Breadcrumb: root > ... > selected node (Vivado hierarchy-tab pattern).
    // Segments select their ancestor; keeps orientation in deep trees.
    let selected = model.selected.clone();
    let sel_idx = selected
        .as_deref()
        .and_then(|s| state.nodes.iter().position(|n| n.name == s));
    if let Some(mut i) = sel_idx {
        let mut path = vec![i];
        while let Some(p) = state.parent[i] {
            path.push(p);
            i = p;
        }
        path.reverse();
        let mut crumb_pick: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            for (k, &ni) in path.iter().enumerate() {
                if k > 0 {
                    ui.label(egui::RichText::new("›").small().color(theme.faint));
                }
                let last = k + 1 == path.len();
                let lbl = egui::RichText::new(&state.nodes[ni].short)
                    .small()
                    .color(if last { theme.text } else { theme.info });
                if ui.link(lbl).clicked() {
                    crumb_pick = Some(state.nodes[ni].name.clone());
                }
            }
        });
        if let Some(id) = crumb_pick {
            model.select(&id);
        }
        ui.add_space(2.0);
    }

    let mut pick: Option<String> = None;
    let mut toggle: Option<String> = None;
    let mut open_sheet: Option<String> = None;

    egui::ScrollArea::vertical()
        .id_salt("hierarchy_tree_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, visible.len(), |ui, range| {
            let painter = ui.painter().clone();
            for i in range {
                let ni = visible[i];
                let n = &state.nodes[ni];
                let full_w = ui.available_width();
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(full_w, ROW_H), egui::Sense::click());
                let is_sel = selected.as_deref() == Some(n.name.as_str());
                if is_sel {
                    painter.rect_filled(rect, 4.0, theme.sel);
                    painter.rect_filled(
                        egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height())),
                        1.0,
                        theme.accent,
                    );
                } else if resp.hovered() {
                    painter.rect_filled(rect, 4.0, theme.surface_hover);
                }
                // Indent guides (VS Code pattern).
                for d in 0..n.depth {
                    let gx = rect.min.x + 8.0 + d as f32 * INDENT;
                    painter.line_segment(
                        [
                            egui::pos2(gx, rect.min.y + 5.0),
                            egui::pos2(gx, rect.max.y - 5.0),
                        ],
                        egui::Stroke::new(1.0, theme.line.gamma_multiply(0.55)),
                    );
                }
                let has_kids = !n.children.is_empty();
                let expanded =
                    has_kids && (filt_active || ni == state.root || state.expanded.contains(&n.name));
                let chev_x = rect.min.x + 8.0 + n.depth as f32 * INDENT + 6.0;
                if has_kids {
                    paint_chevron(
                        &painter,
                        egui::pos2(chev_x, rect.center().y),
                        expanded,
                        theme.dim,
                    );
                }
                let label_x = rect.min.x + 8.0 + n.depth as f32 * INDENT + 18.0;
                let is_internal = TreeState::is_instance_kind(&n.kind);
                painter.text(
                    egui::pos2(label_x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &n.short,
                    egui::FontId::proportional(13.0),
                    if is_internal { theme.text } else { theme.dim },
                );
                // Right-aligned count (instances) or kind (leaves).
                let right = if is_internal {
                    format!("({})", n.leaf_count)
                } else {
                    n.kind.clone()
                };
                painter.text(
                    egui::pos2(rect.max.x - 10.0, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    right,
                    egui::FontId::proportional(11.0),
                    theme.faint,
                );
                // 1px divider.
                painter.line_segment(
                    [
                        egui::pos2(rect.min.x + 8.0, rect.max.y - 0.5),
                        egui::pos2(rect.max.x - 8.0, rect.max.y - 0.5),
                    ],
                    egui::Stroke::new(1.0, theme.line.gamma_multiply(0.35)),
                );

                if resp.double_clicked() {
                    if has_kids {
                        toggle = Some(n.name.clone());
                    }
                    open_sheet = Some(n.name.clone());
                } else if resp.clicked() {
                    // Clicks on the chevron gutter toggle; elsewhere selects.
                    if has_kids {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            if pos.x < label_x {
                                toggle = Some(n.name.clone());
                            } else {
                                pick = Some(n.name.clone());
                            }
                            continue;
                        }
                    }
                    pick = Some(n.name.clone());
                }
            }
        });

    if let Some(name) = toggle {
        if state.expanded.contains(&name) {
            state.expanded.remove(&name);
        } else {
            state.expanded.insert(name);
        }
    }
    if let Some(id) = pick {
        model.select(&id);
    }
    if let Some(name) = open_sheet {
        // Keep the treemap-era double-click contract: open the sheet.
        let _ = model.exec(&format!("open_hierarchy_sheet {name}"));
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(state_id, state));
}
