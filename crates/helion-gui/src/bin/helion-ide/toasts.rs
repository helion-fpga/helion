//! Toast notifications (P1-5): job-completion toasts stacked above the
//! status bar, VS Code style.

use eframe::egui::{self, Color32};

/// Severity of a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub msg: String,
    pub kind: ToastKind,
    /// Frame counter when pushed; painted for ~4s of frames.
    pub born_frame: u64,
}

#[derive(Default)]
pub struct ToastQueue {
    toasts: Vec<Toast>,
    frame: u64,
}

impl ToastQueue {
    pub fn push(&mut self, kind: ToastKind, msg: impl Into<String>) {
        self.toasts.push(Toast {
            msg: msg.into(),
            kind,
            born_frame: self.frame,
        });
        // Keep the stack bounded; oldest first.
        while self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    fn color(&self, kind: ToastKind, theme: &super::theme::Theme) -> Color32 {
        match kind {
            ToastKind::Info => theme.info,
            ToastKind::Ok => theme.ok,
            ToastKind::Warn => theme.warn,
            ToastKind::Error => theme.danger,
        }
    }

    /// Paint stacked toasts bottom-right, above the status bar.
    pub fn paint(&mut self, ctx: &egui::Context, theme: &super::theme::Theme) {
        self.frame = self.frame.saturating_add(1);
        // Expire after ~240 frames (~4s at 60fps of active repaint; the IDE is
        // reactive so toasts linger while idle — acceptable, they dismiss on click).
        self.toasts
            .retain(|t| self.frame.saturating_sub(t.born_frame) < 1200);
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -36.0))
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    let mut dismiss = None;
                    for (i, toast) in self.toasts.iter().enumerate() {
                        let accent = self.color(toast.kind, theme);
                        egui::Frame::new()
                            .fill(theme.surface)
                            .stroke(egui::Stroke::new(1.0, theme.line))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.colored_label(accent, "●");
                                    ui.label(&toast.msg);
                                    if ui.small_button("✕").clicked() {
                                        dismiss = Some(i);
                                    }
                                });
                            });
                        ui.add_space(4.0);
                    }
                    if let Some(i) = dismiss {
                        self.toasts.remove(i);
                    }
                });
            });
    }
}
