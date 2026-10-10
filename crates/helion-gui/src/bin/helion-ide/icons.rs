//! Vector activity-rail icons (P0-1).
//!
//! Drawn with the egui painter — no emoji, no letter glyphs, no icon font
//! dependency. Each icon fits a 24×24 box centered in the given rect and
//! uses a single `color` so active/inactive states stay token-driven.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke};
use helion_gui::chrome::Activity;
use helion_gui::FlowStep;

/// Paint the vector glyph for an activity, centered in `rect`.
pub fn paint_activity(painter: &egui::Painter, rect: Rect, act: Activity, color: Color32) {
    let c = rect.center();
    let s = 24.0_f32;
    let r = Rect::from_center_size(c, egui::vec2(s, s));
    let sw = 1.8;
    let stroke = |col| Stroke::new(sw, col);
    match act {
        // Document with folded corner.
        Activity::Files => {
            let x0 = r.min.x + 6.0;
            let x1 = r.max.x - 6.0;
            let y0 = r.min.y + 3.0;
            let y1 = r.max.y - 3.0;
            let fold = 5.0;
            let pts = vec![
                Pos2::new(x0, y0),
                Pos2::new(x1 - fold, y0),
                Pos2::new(x1, y0 + fold),
                Pos2::new(x1, y1),
                Pos2::new(x0, y1),
                Pos2::new(x0, y0),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke(color)));
            painter.line_segment(
                [Pos2::new(x1 - fold, y0), Pos2::new(x1 - fold, y0 + fold)],
                stroke(color),
            );
            painter.line_segment(
                [Pos2::new(x1 - fold, y0 + fold), Pos2::new(x1, y0 + fold)],
                stroke(color),
            );
            // Text lines.
            for (i, y) in [10.0, 14.0, 18.0].iter().enumerate() {
                let w = if i == 2 { 5.0 } else { 9.0 };
                painter.line_segment(
                    [
                        Pos2::new(x0 + 3.0, r.min.y + y),
                        Pos2::new(x0 + 3.0 + w, r.min.y + y),
                    ],
                    Stroke::new(1.4, color),
                );
            }
        }
        // Chip: square body with pins on four sides.
        Activity::Device => {
            let b = Rect::from_center_size(c, egui::vec2(12.0, 12.0));
            painter.rect_stroke(b, 1.5, stroke(color), egui::StrokeKind::Inside);
            let inner = Rect::from_center_size(c, egui::vec2(6.0, 6.0));
            painter.rect_stroke(inner, 1.0, Stroke::new(1.2, color), egui::StrokeKind::Inside);
            for t in [-4.0, 0.0, 4.0] {
                // Top / bottom pins.
                painter.line_segment(
                    [
                        Pos2::new(c.x + t, b.min.y - 3.0),
                        Pos2::new(c.x + t, b.min.y),
                    ],
                    stroke(color),
                );
                painter.line_segment(
                    [
                        Pos2::new(c.x + t, b.max.y),
                        Pos2::new(c.x + t, b.max.y + 3.0),
                    ],
                    stroke(color),
                );
                // Left / right pins.
                painter.line_segment(
                    [
                        Pos2::new(b.min.x - 3.0, c.y + t),
                        Pos2::new(b.min.x, c.y + t),
                    ],
                    stroke(color),
                );
                painter.line_segment(
                    [
                        Pos2::new(b.max.x, c.y + t),
                        Pos2::new(b.max.x + 3.0, c.y + t),
                    ],
                    stroke(color),
                );
            }
        }
        // Clock: circle + hands.
        Activity::Timing => {
            painter.circle_stroke(c, 8.0, stroke(color));
            painter.line_segment([c, Pos2::new(c.x, c.y - 5.0)], stroke(color));
            painter.line_segment([c, Pos2::new(c.x + 3.5, c.y + 1.5)], stroke(color));
            painter.circle_filled(c, 1.4, color);
        }
        // Play triangle in a rounded frame.
        Activity::Simulate => {
            let b = Rect::from_center_size(c, egui::vec2(17.0, 17.0));
            painter.rect_stroke(b, 4.0, stroke(color), egui::StrokeKind::Inside);
            let tri = vec![
                Pos2::new(c.x - 3.0, c.y - 4.5),
                Pos2::new(c.x + 4.5, c.y),
                Pos2::new(c.x - 3.0, c.y + 4.5),
            ];
            painter.add(egui::Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        // Arrow down into a tray (program / flash).
        Activity::Program => {
            painter.line_segment(
                [Pos2::new(c.x, c.y - 8.0), Pos2::new(c.x, c.y + 2.0)],
                stroke(color),
            );
            let head = vec![
                Pos2::new(c.x - 4.0, c.y - 2.0),
                Pos2::new(c.x + 4.0, c.y - 2.0),
                Pos2::new(c.x, c.y + 3.0),
            ];
            painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
            painter.line_segment(
                [Pos2::new(c.x - 8.0, c.y + 7.0), Pos2::new(c.x + 8.0, c.y + 7.0)],
                stroke(color),
            );
            painter.line_segment(
                [Pos2::new(c.x - 8.0, c.y + 7.0), Pos2::new(c.x - 8.0, c.y + 4.0)],
                stroke(color),
            );
            painter.line_segment(
                [Pos2::new(c.x + 8.0, c.y + 7.0), Pos2::new(c.x + 8.0, c.y + 4.0)],
                stroke(color),
            );
        }
        // Bar chart: three bars + axis.
        Activity::Reports => {
            painter.line_segment(
                [Pos2::new(c.x - 8.0, c.y + 7.0), Pos2::new(c.x + 8.0, c.y + 7.0)],
                Stroke::new(1.4, color),
            );
            for (i, h) in [6.0, 11.0, 8.0].iter().enumerate() {
                let x = c.x - 6.0 + i as f32 * 6.0;
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(x - 1.8, c.y + 6.0 - h),
                        Pos2::new(x + 1.8, c.y + 6.0),
                    ),
                    1.0,
                    color,
                );
            }
        }
    }
}

/// Small status dot for a flow stage (pending / running / done / failed).
pub fn paint_stage_dot(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    fill: Color32,
    ring: Option<Color32>,
) {
    painter.circle_filled(center, radius, fill);
    if let Some(rc) = ring {
        painter.circle_stroke(center, radius, Stroke::new(1.2, rc));
    }
}

/// Flow-step glyph for the Flow Navigator tree.
pub fn paint_stage(painter: &egui::Painter, rect: Rect, stage: FlowStep, color: Color32) {
    let c = rect.center();
    let sw = 1.6;
    let stroke = Stroke::new(sw, color);
    match stage {
        // Synthesis: gear-ish — hexagon with inner dot.
        FlowStep::Synthesis => {
            let r = 7.0;
            let pts: Vec<Pos2> = (0..6)
                .map(|i| {
                    let a = std::f32::consts::PI / 3.0 * i as f32 - std::f32::consts::FRAC_PI_2;
                    Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::closed_line(pts, stroke));
            painter.circle_filled(c, 2.0, color);
        }
        // Opt: magic-wand spark — diagonal line with sparkles.
        FlowStep::Opt => {
            painter.line_segment(
                [Pos2::new(c.x - 5.0, c.y + 5.0), Pos2::new(c.x + 3.0, c.y - 3.0)],
                stroke,
            );
            for p in [
                Pos2::new(c.x + 5.0, c.y - 6.0),
                Pos2::new(c.x + 1.0, c.y - 7.0),
                Pos2::new(c.x + 6.0, c.y - 1.0),
            ] {
                painter.circle_filled(p, 1.3, color);
            }
        }
        // Place: crosshair target.
        FlowStep::Place => {
            painter.circle_stroke(c, 6.5, stroke);
            painter.line_segment(
                [Pos2::new(c.x - 9.0, c.y), Pos2::new(c.x + 9.0, c.y)],
                Stroke::new(1.2, color),
            );
            painter.line_segment(
                [Pos2::new(c.x, c.y - 9.0), Pos2::new(c.x, c.y + 9.0)],
                Stroke::new(1.2, color),
            );
            painter.circle_filled(c, 1.6, color);
        }
        // Route: path with nodes.
        FlowStep::Route => {
            let pts = vec![
                Pos2::new(c.x - 8.0, c.y + 6.0),
                Pos2::new(c.x - 2.0, c.y + 6.0),
                Pos2::new(c.x - 2.0, c.y - 2.0),
                Pos2::new(c.x + 5.0, c.y - 2.0),
                Pos2::new(c.x + 5.0, c.y - 7.0),
            ];
            let dots = [pts[0], pts[2], pts[4]];
            painter.add(egui::Shape::line(pts, stroke));
            for p in dots {
                painter.circle_filled(p, 1.8, color);
            }
        }
        // Bitstream: stacked frames.
        FlowStep::Bitstream => {
            for (i, w) in [12.0, 9.0, 6.0].iter().enumerate() {
                let y = c.y - 5.0 + i as f32 * 5.0;
                painter.rect_filled(
                    Rect::from_center_size(Pos2::new(c.x, y), egui::vec2(*w, 2.6)),
                    1.0,
                    color,
                );
            }
        }
    }
}
