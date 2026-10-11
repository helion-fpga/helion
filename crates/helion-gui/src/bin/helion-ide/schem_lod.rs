//! Schematic level-of-detail: hierarchical block treemap at low zoom.
//!
//! At fit zoom on a 15k-cell design, drawing every gate and wire produces
//! visual noise (a wire grid with no readable gates). Below [`BLOCK_ZOOM`]
//! the sheet renders a treemap of hierarchical instance blocks — one box per
//! module, sized and labeled by cell count — instead of the flat gate layout
//! (whose kind-column placement interleaves hierarchy, so world-space boxes
//! would all coincide). Zooming in past the threshold, or double-clicking a
//! block (Expand Inside), restores the gate-level sheet.

use std::collections::{HashMap, HashSet};

use eframe::egui;

use super::theme;
use helion_gui::SchematicDrawing;

/// Below this camera zoom the sheet renders blocks, not gates.
pub const BLOCK_ZOOM: f32 = 0.5;

#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub name: String,
    pub short: String,
    pub cells: usize,
    /// True for the synthetic top-level group (not a real instance).
    pub is_top: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BlockLayer {
    pub blocks: Vec<BlockInfo>,
}

fn longest_prefix<'a>(name: &'a str, inst_set: &HashSet<&'a str>) -> Option<&'a str> {
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

/// Group the drawing's symbols by hierarchical instance (longest prefix
/// match against `instances`; unmatched symbols land in the top group).
/// Deterministic: instances keep first-seen order.
pub fn build_block_layer(
    drawing: &SchematicDrawing,
    instances: &[String],
    top_name: &str,
) -> BlockLayer {
    let inst_set: HashSet<&str> = instances.iter().map(|s| s.as_str()).collect();
    let mut order: Vec<String> = Vec::new();
    let mut idx_of: HashMap<&str, usize> = HashMap::new();
    let mut cells = Vec::new();

    for sy in &drawing.symbols {
        if sy.kind.starts_with("PORT") {
            continue;
        }
        let inst = longest_prefix(&sy.name, &inst_set).unwrap_or("");
        let bi = match idx_of.get(inst) {
            Some(&i) => i,
            None => {
                let i = order.len();
                order.push(inst.to_string());
                idx_of.insert(inst, i);
                cells.push(0usize);
                i
            }
        };
        cells[bi] += 1;
    }

    let mut blocks: Vec<BlockInfo> = Vec::with_capacity(order.len());
    for (i, inst) in order.iter().enumerate() {
        if cells[i] == 0 {
            continue;
        }
        let is_top = inst.is_empty();
        blocks.push(BlockInfo {
            name: if is_top {
                top_name.to_string()
            } else {
                inst.clone()
            },
            short: if is_top {
                top_name.to_string()
            } else {
                helion_gui::schematic_short_name(inst)
            },
            cells: cells[i],
            is_top,
        });
    }
    // Biggest first: stable treemap and paint order.
    blocks.sort_by_key(|b| std::cmp::Reverse(b.cells));
    BlockLayer { blocks }
}

/// Slice-and-dice treemap of the blocks over `view`, area ∝ cell count.
/// Returns view-space rects parallel to `blocks`.
pub fn treemap_rects(view: egui::Rect, blocks: &[BlockInfo]) -> Vec<egui::Rect> {
    let n = blocks.len();
    let mut rects = vec![view; n];
    if n == 0 {
        return rects;
    }
    fn dice(
        rects: &mut [egui::Rect],
        order: &[usize],
        blocks: &[BlockInfo],
        r: egui::Rect,
    ) {
        if order.len() == 1 {
            rects[order[0]] = r;
            return;
        }
        let total: usize = order.iter().map(|&i| blocks[i].cells.max(1)).sum();
        let mut acc = 0usize;
        let mut split = 1usize;
        for (k, &i) in order.iter().enumerate() {
            acc += blocks[i].cells.max(1);
            if k + 1 < order.len() && acc * 2 >= total {
                split = k + 1;
                break;
            }
        }
        let left: usize = order[..split].iter().map(|&i| blocks[i].cells.max(1)).sum();
        let frac = (left as f32 / total as f32).clamp(0.05, 0.95);
        // Split along the longer axis for squarer boxes.
        let (r1, r2) = if r.width() >= r.height() {
            let w = r.width() * frac;
            (
                egui::Rect::from_min_size(r.min, egui::vec2(w, r.height())),
                egui::Rect::from_min_size(
                    egui::pos2(r.min.x + w, r.min.y),
                    egui::vec2(r.width() - w, r.height()),
                ),
            )
        } else {
            let h = r.height() * frac;
            (
                egui::Rect::from_min_size(r.min, egui::vec2(r.width(), h)),
                egui::Rect::from_min_size(
                    egui::pos2(r.min.x, r.min.y + h),
                    egui::vec2(r.width(), r.height() - h),
                ),
            )
        };
        dice(rects, &order[..split], blocks, r1);
        dice(rects, &order[split..], blocks, r2);
    }
    let order: Vec<usize> = (0..n).collect();
    dice(&mut rects, &order, blocks, view);
    for r in &mut rects {
        *r = r.shrink(4.0);
    }
    rects
}

/// Block index at view-space `pos`, or `None`.
pub fn hit_block(layer: &BlockLayer, view: egui::Rect, pos: egui::Pos2) -> Option<usize> {
    treemap_rects(view, &layer.blocks)
        .iter()
        .position(|r| r.contains(pos))
}

/// Paint the treemap: one labeled box per hierarchical block.
pub fn paint_block_layer(
    p: &egui::Painter,
    view: egui::Rect,
    layer: &BlockLayer,
    theme: &theme::Theme,
    selected: Option<&str>,
) {
    let rects = treemap_rects(view, &layer.blocks);
    for (b, r) in layer.blocks.iter().zip(rects.iter()) {
        if !r.intersects(view) {
            continue;
        }
        let on = selected == Some(b.name.as_str());
        p.rect_filled(*r, 6.0, theme.surface);
        p.rect_stroke(
            *r,
            6.0,
            egui::Stroke::new(
                if on { 2.0 } else { 1.0 },
                if on { theme.gold } else { theme.line },
            ),
            egui::StrokeKind::Inside,
        );
        // Labels only when they fit inside the box — never overflow.
        let name_need = b.short.len() as f32 * 7.0 + 16.0;
        if r.width() >= 80.0 && r.height() >= 48.0 && r.width() >= name_need {
            p.text(
                egui::pos2(r.center().x, r.center().y - 9.0),
                egui::Align2::CENTER_CENTER,
                &b.short,
                theme::font_ui_semibold(13.0),
                theme.text,
            );
            p.text(
                egui::pos2(r.center().x, r.center().y + 10.0),
                egui::Align2::CENTER_CENTER,
                format!("{} cells", b.cells),
                egui::FontId::proportional(11.0),
                theme.dim,
            );
        } else {
            // Single-line short name only when it fits inside the box.
            let need = b.short.len() as f32 * 6.5 + 16.0;
            if r.width() >= need && r.height() >= 20.0 {
                p.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    &b.short,
                    egui::FontId::proportional(10.0),
                    theme.dim,
                );
            }
        }
    }
}
