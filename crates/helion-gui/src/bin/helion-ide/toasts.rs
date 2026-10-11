//! Toast notifications: small auto-dismissing notices, bottom-right above the
//! status bar (VS Code style).
//!
//! Job *progress* lives in the status bar; toasts are only for completions,
//! warnings and errors. They dismiss on a wall-clock timer — never on frame
//! count, so they vanish even when the UI is idle.

use eframe::egui::{self, Color32};
use std::time::{Duration, Instant};

/// How long a toast stays on screen.
const TOAST_LIFETIME: Duration = Duration::from_secs(5);
/// Toasts never grow wider than this; long messages wrap.
const TOAST_MAX_WIDTH: f32 = 360.0;
/// Maximum stacked toasts; oldest are dropped first.
const TOAST_MAX_STACK: usize = 4;

/// Severity of a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Ok,
    Warn,
    Error,
}

#[derive(Debug)]
pub struct Toast {
    pub msg: String,
    pub kind: ToastKind,
    born: Instant,
}

#[derive(Default)]
pub struct ToastQueue {
    toasts: Vec<Toast>,
}

impl ToastQueue {
    pub fn push(&mut self, kind: ToastKind, msg: impl Into<String>) {
        let msg = msg.into();
        // Don't stack the same notice twice (e.g. double job completion).
        if self.toasts.iter().any(|t| t.msg == msg && t.kind == kind) {
            return;
        }
        self.toasts.push(Toast {
            msg,
            kind,
            born: Instant::now(),
        });
        while self.toasts.len() > TOAST_MAX_STACK {
            self.toasts.remove(0);
        }
    }

    fn color(&self, kind: ToastKind, theme: &super::theme::Theme) -> Color32 {
        match kind {
            ToastKind::Ok => theme.ok,
            ToastKind::Warn => theme.warn,
            ToastKind::Error => theme.danger,
        }
    }

    /// Paint small stacked toasts bottom-right, above the status bar.
    pub fn paint(&mut self, ctx: &egui::Context, theme: &super::theme::Theme) {
        self.toasts.retain(|t| t.born.elapsed() < TOAST_LIFETIME);
        if self.toasts.is_empty() {
            return;
        }
        // Keep the repaint loop alive while toasts are up so they dismiss
        // on the wall-clock timer even when the UI is otherwise idle.
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -36.0))
            .show(ctx, |ui| {
                ui.set_max_width(TOAST_MAX_WIDTH);
                ui.vertical(|ui| {
                    let mut dismiss = None;
                    for (i, toast) in self.toasts.iter().enumerate() {
                        let accent = self.color(toast.kind, theme);
                        // Thin remaining-lifetime bar under the toast.
                        let frac = 1.0
                            - (toast.born.elapsed().as_secs_f32()
                                / TOAST_LIFETIME.as_secs_f32())
                            .clamp(0.0, 1.0);
                        egui::Frame::new()
                            .fill(theme.surface)
                            .stroke(egui::Stroke::new(1.0, theme.line))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.set_max_width(TOAST_MAX_WIDTH - 24.0);
                                ui.horizontal(|ui| {
                                    ui.colored_label(accent, "●");
                                    // Wraps inside the max width instead of
                                    // stretching the toast across the screen.
                                    ui.label(&toast.msg);
                                    // ASCII "x": the bundled font has no
                                    // U+2715 glyph (it rendered as "?").
                                    if ui.small_button("x").clicked() {
                                        dismiss = Some(i);
                                    }
                                });
                                // Lifetime indicator.
                                let bar_w =
                                    (ui.available_width() * frac).max(0.0);
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 2.0),
                                    egui::Sense::hover(),
                                );
                                if bar_w > 0.0 {
                                    ui.painter().line_segment(
                                        [r.left_top(), r.left_top() + egui::vec2(bar_w, 0.0)],
                                        egui::Stroke::new(2.0, accent),
                                    );
                                }
                            });
                        ui.add_space(6.0);
                    }
                    if let Some(i) = dismiss {
                        self.toasts.remove(i);
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_queue_dedupes_and_bounds() {
        let mut q = ToastQueue::default();
        q.push(ToastKind::Ok, "done");
        q.push(ToastKind::Ok, "done");
        assert_eq!(q.toasts.len(), 1, "duplicate notice must not stack");
        for i in 0..10 {
            q.push(ToastKind::Warn, format!("msg {i}"));
        }
        assert!(
            q.toasts.len() <= TOAST_MAX_STACK,
            "stack must stay bounded"
        );
    }

    #[test]
    fn toast_expiry_is_wall_clock() {
        let mut q = ToastQueue::default();
        q.push(ToastKind::Ok, "fresh");
        // A brand-new toast must survive the retain pass (no frame counting).
        q.toasts.retain(|t| t.born.elapsed() < TOAST_LIFETIME);
        assert_eq!(q.toasts.len(), 1);
        // Simulate an aged toast.
        q.toasts[0].born =
            Instant::now() - TOAST_LIFETIME - Duration::from_secs(1);
        q.toasts.retain(|t| t.born.elapsed() < TOAST_LIFETIME);
        assert!(q.toasts.is_empty(), "aged toast must expire");
    }
}
