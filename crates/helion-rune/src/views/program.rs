//! Hardware Manager: open target, cable/target pickers, Program Device, progress + results.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/lab3-Fig14.png` PROGRAM AND DEBUG rail section with
//! Generate Bitstream; `lab3-Fig15.png` unconnected banner: "HARDWARE MANAGER
//! - unconnected" plus the "No hardware target is open. Open target" prompt).
//! The center pane pairs a connection column (server, cable picker, target
//! picker, bitstream file) with a run column (device table, progress bar,
//! results card). All colors come from [`VivadoTheme`]; selection is the
//! typed [`CableId`]/[`TargetId`]/[`ProgramPhase`] enums (no stringly status,
//! no hardcoded hex, no manual geometry).
//!
//! Data shapes mirror `helion-gui` read-only: Tcl verbs echo to the shared
//! console (`open_hw_target`, `program_hw_devices`, `launch_runs -to_step
//! write_bitstream`, matching the rail/menu verbs in `chrome.rs`).
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()`.
//! - `crates/rune/src/el/layout.rs`: `.w_frac()` progress fill, `.h_full()`.
//! - `crates/rune/src/el/text.rs`: `.truncate()`/`.truncate_middle()`.
//! - `crates/rune/src/el/handlers.rs`: `.on_press()` dispatch.
//! - `crates/rune-kit/src/glyphs.rs`: `TRIANGLE_RIGHT`, `CHECK`, `CIRCLE_X`.

use rune::{Align, El, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::{AppState, ConsoleLine, MsgSev, ToastKind};
use crate::theme::VivadoTheme;
use crate::views::sources::pane_header;

/// Connection-column width (the run column takes the rest).
const LEFT_W: f32 = 320.0;
/// Label/value row height.
const ROW_H: f32 = 20.0;
/// Picker option row height.
const OPT_H: f32 = 24.0;
/// Device-table header band height.
const TABLE_HEADER_H: f32 = 22.0;
/// Device-table row height (compact CAD density).
const TABLE_ROW_H: f32 = 20.0;
/// Progress bar height.
const BAR_H: f32 = 14.0;
/// Device status column width.
const STATUS_W: f32 = 130.0;
/// Results `label:` width.
const LABEL_W: f32 = 92.0;

/// Demo hardware server, shown in the banner and connection card.
pub const HW_SERVER: &str = "localhost:3121";

/// JTAG cable pick, in Vivado Open Target order. Typed, never a string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CableId {
    #[default]
    Auto,
    PlatformCable,
    Hs3,
}

impl CableId {
    pub const ALL: [CableId; 3] = [CableId::Auto, CableId::PlatformCable, CableId::Hs3];

    pub fn label(self) -> &'static str {
        match self {
            CableId::Auto => "Auto Connect",
            CableId::PlatformCable => "Platform Cable USB II",
            CableId::Hs3 => "Digilent JTAG-HS3",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            CableId::Auto => "localhost server, first cable",
            CableId::PlatformCable => "esn 000012345678",
            CableId::Hs3 => "210299A4D6C1",
        }
    }
}

/// Open hardware target (device on the JTAG chain).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetId {
    #[default]
    Fpga0,
    ArmDap,
}

impl TargetId {
    pub const ALL: [TargetId; 2] = [TargetId::Fpga0, TargetId::ArmDap];

    pub fn name(self) -> &'static str {
        match self {
            TargetId::Fpga0 => "xc7z007s_0",
            TargetId::ArmDap => "arm_dap_0",
        }
    }

    pub fn part(self) -> &'static str {
        match self {
            TargetId::Fpga0 => "XC7Z007S",
            TargetId::ArmDap => "ARM DAP",
        }
    }
}

/// Program-run phase. A real enum, never stringly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgramPhase {
    #[default]
    Idle,
    Programming,
    Done,
}

/// Hardware Manager UI state: connection, cable/target picks, bitstream, run.
#[derive(Clone, Debug)]
pub struct ProgramState {
    pub connected: bool,
    pub cable: CableId,
    pub target: TargetId,
    /// Empty until Generate Bitstream produces one (never a sentinel string).
    pub bitstream: String,
    pub phase: ProgramPhase,
    /// 0-100 progress of the program run.
    pub progress: u8,
}

impl ProgramState {
    /// Demo bitstream path (`impl_1` run directory, Vivado layout).
    pub fn default_bitstream() -> String {
        "C:/EE214/project_1/project_1.runs/impl_1/project1_demo.bit".into()
    }

    pub fn has_bitstream(&self) -> bool {
        !self.bitstream.is_empty()
    }

    pub fn can_program(&self) -> bool {
        self.connected && self.has_bitstream() && self.phase != ProgramPhase::Programming
    }

    /// File name of the bitstream for the results card and toast.
    pub fn basename(&self) -> &str {
        self.bitstream
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.bitstream)
    }

    pub fn program_tcl(&self) -> String {
        format!("program_hw_devices [get_hw_devices {}]", self.target.name())
    }

    pub fn progress_label(&self) -> String {
        match self.phase {
            ProgramPhase::Idle if !self.connected => "No hardware target open".into(),
            ProgramPhase::Idle => "Ready".into(),
            ProgramPhase::Programming => format!("Programming device... {}%", self.progress),
            ProgramPhase::Done => format!("Programming complete ({}%)", self.progress),
        }
    }

    pub fn done_detail(&self) -> String {
        format!("{} programmed with {}", self.target.name(), self.basename())
    }

    /// Device-table status cell: only the programmed target reads Programmed.
    pub fn device_status(&self, id: TargetId) -> &'static str {
        if self.phase == ProgramPhase::Done && self.target == id {
            "Programmed"
        } else {
            "Not programmed"
        }
    }
}

impl Default for ProgramState {
    fn default() -> Self {
        Self {
            connected: false,
            cable: CableId::Auto,
            target: TargetId::Fpga0,
            bitstream: Self::default_bitstream(),
            phase: ProgramPhase::Idle,
            progress: 0,
        }
    }
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: String) {
    s.console.lines.push(ConsoleLine {
        text: tcl,
        sev: MsgSev::Info,
    });
}

fn open_target(s: &mut AppState) {
    s.program.connected = true;
    echo(s, "open_hw_target".into());
}

fn close_target(s: &mut AppState) {
    s.program.connected = false;
    s.program.phase = ProgramPhase::Idle;
    s.program.progress = 0;
    echo(s, "close_hw_target".into());
}

/// Program Device completes synchronously (the headless harness has no time):
/// progress jumps to 100 and the results card mounts.
fn start_program(s: &mut AppState) {
    if !s.program.can_program() {
        return;
    }
    let tcl = s.program.program_tcl();
    s.program.phase = ProgramPhase::Done;
    s.program.progress = 100;
    echo(s, tcl);
    let detail = s.program.done_detail();
    s.toasts
        .push(ToastKind::Success, "Device programmed", &detail);
}

fn cancel_program(s: &mut AppState) {
    s.program.phase = ProgramPhase::Idle;
    s.program.progress = 0;
    let tcl = format!(
        "refresh_hw_device [get_hw_devices {}]",
        s.program.target.name()
    );
    echo(s, tcl);
}

/// Titled card: bold title over a bordered body.
fn card(key: &'static str, title: &'static str, body: El, t: &VivadoTheme) -> El {
    col()
        .key(key)
        .w_full()
        .gap(2.0)
        .child(text(title).font(VivadoTheme::FONT_TITLE).color(t.text))
        .child(
            col()
                .w_full()
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .padding(8.0)
                .gap(2.0)
                .child(body),
        )
}

/// Small toolbar button echoing a Tcl verb (or running a UI action).
fn tool_btn(
    key: &'static str,
    label_text: &'static str,
    t: &VivadoTheme,
    action: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label_text)
        .focusable(true)
        .h(ROW_H)
        .px(8.0)
        .center_y()
        .hover(t.selection)
        .on_press(action)
        .child(text(label_text).font(VivadoTheme::FONT_UI).color(t.text))
}

/// A toolbar button that raises a native dialog (the bitstream Browse button).
fn tool_btn_tx(
    key: &'static str,
    label_text: &'static str,
    t: &VivadoTheme,
    action: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label_text)
        .focusable(true)
        .h(ROW_H)
        .px(8.0)
        .center_y()
        .hover(t.selection)
        .on_press_tx(action)
        .child(text(label_text).font(VivadoTheme::FONT_UI).color(t.text))
}

/// Blue link dispatching a UI action.
fn link_btn(
    key: &'static str,
    label_text: &'static str,
    t: &VivadoTheme,
    action: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label_text)
        .focusable(true)
        .h(ROW_H)
        .px(2.0)
        .center_y()
        .hover(t.selection)
        .on_press(action)
        .child(text(label_text).font(VivadoTheme::FONT_UI).color(t.link))
}

/// Green/red status banner (`lab3-Fig15.png` unconnected wording verbatim).
fn banner(s: &AppState, t: &VivadoTheme) -> El {
    let title = if s.program.connected {
        format!("HARDWARE MANAGER - {HW_SERVER}")
    } else {
        "HARDWARE MANAGER - unconnected".into()
    };
    let mut detail = row().w_full().h(ROW_H).items_center().gap(4.0);
    if s.program.connected {
        detail = detail
            .child(icon(&glyphs::CHECK, 12.0).color(t.pass).a11y_hidden())
            .child(
                dim_label(
                    &format!(
                        "Target {} on {}",
                        s.program.target.name(),
                        s.program.cable.label()
                    ),
                    t,
                )
                .truncate(),
            )
            .child(link_btn(
                "program-close-target",
                "Close target",
                t,
                close_target,
            ));
    } else {
        detail = detail
            .child(
                icon(&glyphs::CIRCLE_X, 12.0)
                    .color(t.text_dim)
                    .a11y_hidden(),
            )
            .child(dim_label("No hardware target is open.", t))
            .child(link_btn(
                "program-open-target",
                "Open target",
                t,
                open_target,
            ));
    }
    col()
        .key("program-banner")
        .w_full()
        .gap(0.0)
        .child(
            text(title)
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .key("program-banner-title"),
        )
        .child(detail.key("program-banner-detail"))
}

/// One cable picker option: check mark plus name and cable detail.
fn cable_row(index: usize, cable: CableId, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.program.cable == cable;
    let mut content = row().w_full().h(OPT_H).items_center().gap(6.0).px(8.0);
    content = content.child(if selected {
        icon(&glyphs::CHECK, 12.0).color(t.accent).a11y_hidden()
    } else {
        el().size(12.0, 12.0)
    });
    content = content
        .child(ui_label(cable.label(), t).grow(1.0).min_w(0.0).truncate())
        .child(dim_label(cable.detail(), t).grow(1.0).min_w(0.0).truncate());
    let mut item = el()
        .key(format!("program-cable-opt-{index}"))
        .role("button")
        .label(format!("Cable: {}", cable.label()))
        .focusable(true)
        .w_full()
        .h(OPT_H)
        .center_y()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.program.cable = cable)
        .child(content);
    if selected {
        item = item.fill(t.selection);
    }
    item
}

/// One target picker option: check mark plus device name and part.
fn target_row(index: usize, target: TargetId, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.program.target == target;
    let mut content = row().w_full().h(OPT_H).items_center().gap(6.0).px(8.0);
    content = content.child(if selected {
        icon(&glyphs::CHECK, 12.0).color(t.accent).a11y_hidden()
    } else {
        el().size(12.0, 12.0)
    });
    content = content
        .child(ui_label(target.name(), t).grow(1.0).min_w(0.0).truncate())
        .child(dim_label(target.part(), t).grow(1.0).min_w(0.0).truncate());
    let mut item = el()
        .key(format!("program-target-opt-{index}"))
        .role("button")
        .label(format!("Target: {}", target.name()))
        .focusable(true)
        .w_full()
        .h(OPT_H)
        .center_y()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.program.target = target)
        .child(content);
    if selected {
        item = item.fill(t.selection);
    }
    item
}

fn connection_card(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().w_full().gap(2.0);
    body = body.child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .gap(6.0)
            .child(dim_label("Server:", t))
            .child(ui_label(HW_SERVER, t).key("program-server")),
    );
    let mut cables = col()
        .key("program-cable")
        .w_full()
        .gap(0.0)
        .child(dim_label("Cable:", t).key("program-cable-label"));
    for (index, cable) in CableId::ALL.into_iter().enumerate() {
        cables = cables.child(cable_row(index, cable, s, t));
    }
    body = body.child(cables);
    let mut targets = col()
        .key("program-target")
        .w_full()
        .gap(0.0)
        .child(dim_label("Target:", t).key("program-target-label"));
    for (index, target) in TargetId::ALL.into_iter().enumerate() {
        targets = targets.child(target_row(index, target, s, t));
    }
    body = body.child(targets);
    card("program-connection", "Connection", body, t)
}

fn bitstream_card(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().w_full().gap(4.0);
    if s.program.has_bitstream() {
        body = body.child(
            text(s.program.bitstream.clone())
                .font(VivadoTheme::FONT_UI)
                .color(t.text)
                .truncate_middle()
                .key("program-bitstream-path"),
        );
    } else {
        body = body
            .child(
                dim_label("(not generated - run Generate Bitstream)", t)
                    .key("program-bitstream-path"),
            )
            .child(link_btn(
                "program-generate",
                "Generate Bitstream",
                t,
                |s: &mut AppState| {
                    s.program.bitstream = ProgramState::default_bitstream();
                    echo(s, "launch_runs -to_step write_bitstream".into());
                },
            ));
    }
    body = body.child(
        row()
            .w_full()
            .h(ROW_H)
            .items_center()
            .gap(6.0)
            .child(tool_btn_tx(
                "program-browse",
                "Browse...",
                t,
                |_: &mut AppState, tx: &mut Tx| {
                    crate::pickers::bitstream(tx);
                },
            )),
    );
    card("program-bitstream", "Bitstream File", body, t)
}

/// One device-table row: name, part, program status.
fn device_row(index: usize, id: TargetId, s: &AppState, t: &VivadoTheme) -> El {
    let programmed = s.program.device_status(id) == "Programmed";
    let status_color = if programmed { t.pass } else { t.text_dim };
    row()
        .key(format!("program-device-row-{index}"))
        .w_full()
        .h(TABLE_ROW_H)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .child(
            ui_label(id.name(), t)
                .grow(2.0)
                .min_w(0.0)
                .truncate()
                .key(format!("program-device-name-{index}")),
        )
        .child(dim_label(id.part(), t).grow(2.0).min_w(0.0).truncate())
        .child(
            el().w(STATUS_W).child(
                text(s.program.device_status(id))
                    .font(VivadoTheme::FONT_UI)
                    .color(status_color)
                    .align(Align::End)
                    .key(format!("program-device-status-{index}")),
            ),
        )
}

fn devices_card(s: &AppState, t: &VivadoTheme) -> El {
    let header_cell = |content: &'static str| -> El {
        text(content)
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .truncate()
    };
    let mut table = col().w_full().fill(t.panel_bg).border(1.0, t.border).child(
        row()
            .w_full()
            .h(TABLE_HEADER_H)
            .items_center()
            .gap(8.0)
            .px(8.0)
            .fill(t.header_bg)
            .child(header_cell("Name").grow(2.0).min_w(0.0))
            .child(header_cell("Part").grow(2.0).min_w(0.0))
            .child(el().w(STATUS_W).child(header_cell("Status"))),
    );
    if s.program.connected {
        for (index, id) in TargetId::ALL.into_iter().enumerate() {
            table = table.child(device_row(index, id, s, t));
        }
    } else {
        table = table.child(
            row()
                .key("program-no-devices")
                .w_full()
                .h(TABLE_ROW_H)
                .items_center()
                .px(8.0)
                .child(dim_label("No devices (open a hardware target)", t)),
        );
    }
    col()
        .key("program-devices")
        .w_full()
        .gap(2.0)
        .child(text("Devices").font(VivadoTheme::FONT_TITLE).color(t.text))
        .child(table)
}

fn progress_card(s: &AppState, t: &VivadoTheme) -> El {
    let frac = f32::from(s.program.progress) / 100.0;
    let mut body = col().w_full().gap(4.0);
    body = body
        .child(
            el().key("program-progress-bar")
                .w_full()
                .h(BAR_H)
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .child(
                    el().key("program-progress-fill")
                        .w_frac(frac)
                        .h_full()
                        .fill(t.accent),
                ),
        )
        .child(dim_label(&s.program.progress_label(), t).key("program-progress-label"));
    let mut actions = row().w_full().h(ROW_H).items_center().gap(6.0);
    if s.program.can_program() {
        actions = actions.child(
            el().key("program-program")
                .role("button")
                .label("Program Device")
                .focusable(true)
                .h(ROW_H)
                .px(8.0)
                .center_y()
                .fill(t.header_bg)
                .border(1.0, t.border)
                .hover(t.selection)
                .on_press(start_program)
                .child(
                    row()
                        .items_center()
                        .gap(4.0)
                        .child(
                            icon(&glyphs::TRIANGLE_RIGHT, 10.0)
                                .color(t.pass)
                                .a11y_hidden(),
                        )
                        .child(ui_label("Program Device", t)),
                ),
        );
    } else {
        actions = actions.child(
            el().key("program-program")
                .h(ROW_H)
                .px(8.0)
                .center_y()
                .fill(t.header_bg)
                .border(1.0, t.border)
                .child(dim_label("Program Device", t)),
        );
    }
    if s.program.phase == ProgramPhase::Programming {
        actions = actions.child(tool_btn("program-cancel", "Cancel", t, cancel_program));
    }
    body = body.child(actions);
    card("program-progress", "Program", body, t)
}

fn kv(label_text: &'static str, value: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(6.0)
        .child(el().w(LABEL_W).child(dim_label(label_text, t).truncate()))
        .child(value)
}

/// Results card: green verdict plus device/bitstream/elapsed rows.
fn results_card(s: &AppState, t: &VivadoTheme) -> Option<El> {
    if s.program.phase != ProgramPhase::Done {
        return None;
    }
    let body = col()
        .key("program-results-detail")
        .w_full()
        .gap(2.0)
        .child(kv("Device:", ui_label(s.program.target.name(), t), t))
        .child(kv(
            "Bitstream:",
            ui_label(s.program.basename(), t).truncate(),
            t,
        ))
        .child(kv(
            "Result:",
            ui_label("program_hw_devices completed successfully", t).truncate(),
            t,
        ));
    Some(
        col()
            .key("program-results")
            .w_full()
            .gap(2.0)
            .child(
                text("Device programmed successfully.")
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.pass)
                    .key("program-verdict"),
            )
            .child(
                col()
                    .w_full()
                    .fill(t.panel_bg)
                    .border(1.0, t.border)
                    .padding(8.0)
                    .child(body),
            ),
    )
}

/// The center Hardware Manager pane wired into the shell.
pub fn program_pane(s: &AppState, t: &VivadoTheme) -> El {
    let left = col()
        .w(LEFT_W)
        .gap(8.0)
        .child(connection_card(s, t))
        .child(bitstream_card(s, t));
    let mut right = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(8.0)
        .child(devices_card(s, t))
        .child(progress_card(s, t));
    if let Some(results) = results_card(s, t) {
        right = right.child(results);
    }
    col()
        .key("program")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Hardware Manager", "program", t))
        .child(
            row()
                .key("program-toolbar")
                .role("toolbar")
                .w_full()
                .h(26.0)
                .items_center()
                .gap(2.0)
                .px(8.0)
                .child(tool_btn(
                    "program-auto-connect",
                    "Auto Connect",
                    t,
                    |s: &mut AppState| {
                        s.program.cable = CableId::Auto;
                        s.program.connected = true;
                        echo(s, format!("connect_hw_server -url {HW_SERVER}"));
                        echo(s, "open_hw_target".into());
                    },
                ))
                .child(tool_btn(
                    "program-refresh",
                    "Refresh",
                    t,
                    |s: &mut AppState| {
                        echo(s, "get_hw_targets".into());
                    },
                )),
        )
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(8.0)
                .padding(8.0)
                .child(banner(s, t))
                .child(
                    col()
                        .key("program-scroll")
                        .w_full()
                        .grow(1.0)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(row().w_full().gap(8.0).child(left).child(right)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn program_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Program;
        s
    }

    #[test]
    fn program_tree_exposes_pickers_progress_and_banner() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        for key in [
            "program",
            "program-toolbar",
            "program-auto-connect",
            "program-refresh",
            "program-banner",
            "program-banner-title",
            "program-open-target",
            "program-connection",
            "program-server",
            "program-cable",
            "program-cable-opt-0",
            "program-cable-opt-1",
            "program-cable-opt-2",
            "program-target",
            "program-target-opt-0",
            "program-target-opt-1",
            "program-bitstream",
            "program-bitstream-path",
            "program-browse",
            "program-devices",
            "program-no-devices",
            "program-progress",
            "program-progress-bar",
            "program-progress-label",
            "program-program",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        for key in [
            "program-close-target",
            "program-device-row-0",
            "program-results",
            "program-verdict",
            "program-cancel",
            "program-generate",
        ] {
            assert!(rt.find(key).is_none(), "should be unmounted: {key}");
        }
    }

    #[test]
    fn open_target_connects_and_mounts_devices() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        tap(&mut rt, "program-open-target");
        assert!(rt.state.program.connected);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "open_hw_target")
        );
        for key in [
            "program-close-target",
            "program-device-row-0",
            "program-device-row-1",
            "program-device-name-0",
            "program-device-status-0",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        assert!(rt.find("program-open-target").is_none());
        assert!(rt.find("program-no-devices").is_none());
        tap(&mut rt, "program-close-target");
        assert!(!rt.state.program.connected);
        assert!(rt.find("program-open-target").is_some());
    }

    #[test]
    fn cable_and_target_picks_update_state() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        tap(&mut rt, "program-cable-opt-2");
        assert_eq!(rt.state.program.cable, CableId::Hs3);
        tap(&mut rt, "program-target-opt-1");
        assert_eq!(rt.state.program.target, TargetId::ArmDap);
        tap(&mut rt, "program-auto-connect");
        assert_eq!(rt.state.program.cable, CableId::Auto);
        assert!(rt.state.program.connected);
    }

    #[test]
    fn program_completes_with_results_and_toast() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        tap(&mut rt, "program-open-target");
        tap(&mut rt, "program-program");
        assert_eq!(rt.state.program.phase, ProgramPhase::Done);
        assert_eq!(rt.state.program.progress, 100);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "program_hw_devices [get_hw_devices xc7z007s_0]")
        );
        for key in [
            "program-results",
            "program-verdict",
            "program-results-detail",
            "toast-0",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        assert_eq!(
            rt.state.program.device_status(TargetId::Fpga0),
            "Programmed"
        );
        assert_eq!(
            rt.state.program.device_status(TargetId::ArmDap),
            "Not programmed"
        );
    }

    #[test]
    fn program_requires_an_open_target() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        tap(&mut rt, "program-program");
        assert_eq!(rt.state.program.phase, ProgramPhase::Idle);
        assert!(rt.find("program-results").is_none());
    }

    #[test]
    fn missing_bitstream_prompts_generate() {
        let mut s = program_state();
        s.program.bitstream.clear();
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("program-generate").is_some());
        tap(&mut rt, "program-program");
        assert_eq!(rt.state.program.phase, ProgramPhase::Idle);
        tap(&mut rt, "program-generate");
        assert!(rt.state.program.has_bitstream());
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs -to_step write_bitstream")
        );
        assert!(rt.find("program-generate").is_none());
    }

    #[test]
    fn bitstream_browse_consumes_the_picked_file() {
        use rune::FileDialogKind;
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        tap(&mut rt, "program-browse");
        let (kind, opts) = rt
            .file_dialog()
            .expect("Browse must raise a native file dialog");
        assert_eq!(kind, FileDialogKind::Open);
        assert!(
            opts.filters
                .iter()
                .any(|(n, exts)| n == "Bitstream" && exts.contains(&"bit".to_string())),
            "filter must accept .bit: {:?}",
            opts.filters
        );
        let picked =
            std::env::temp_dir().join(format!("helion-pick-{}-top.bit", std::process::id()));
        rt.resolve_file_dialog(Some(picked.clone()))
            .expect("answer the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.program.bitstream, picked.display().to_string());
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("set_property PROGRAM.FILE")),
            "browse must echo the PROGRAM.FILE verb"
        );
    }

    #[test]
    fn bitstream_browse_cancel_keeps_the_current_path() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        let before = rt.state.program.bitstream.clone();
        tap(&mut rt, "program-browse");
        rt.resolve_file_dialog(None).expect("cancel the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.program.bitstream, before);
    }

    #[test]
    fn programming_midflight_shows_cancel_and_resets() {
        let mut s = program_state();
        s.program.connected = true;
        s.program.phase = ProgramPhase::Programming;
        s.program.progress = 45;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("program-cancel").is_some());
        assert!(rt.find("program-results").is_none());
        tap(&mut rt, "program-cancel");
        assert_eq!(rt.state.program.phase, ProgramPhase::Idle);
        assert_eq!(rt.state.program.progress, 0);
        assert!(rt.state.program.connected);
        assert!(rt.find("program-cancel").is_none());
    }
}
