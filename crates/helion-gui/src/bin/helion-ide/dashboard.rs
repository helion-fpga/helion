//! Reports dashboard (P1-2): Vivado Project-Summary style dashboard composing
//! the existing UtilizationReport / TimingSummary / DRC structs. Also owns the
//! timing report catalog regroup + path histogram (P1-4).
//! Implemented by the feature agent.

use eframe::egui;

use super::HelionIde;

/// QoR cards (WNS/TNS, util bars), run history, DRC/methodology badges,
/// one-click drill into each report.
pub fn paint_dashboard(_ui: &mut egui::Ui, _app: &mut HelionIde) {
    // TODO(feature): full implementation.
}

/// Timing Analysis report catalog tree (regroups the 8 timing tabs) +
/// path histogram widget. Replaces `paint_report_catalog` content.
pub fn paint_timing_catalog(_ui: &mut egui::Ui, _app: &mut HelionIde) {
    // TODO(feature): full implementation.
}
