//! Hardware Manager tree (Vivado's Hardware window): server → target (cable)
//! → device → debug cores / configuration memory. The selected node picks
//! which panel the Hardware Manager shows ([`crate::views::program`]).
//!
//! Rows only name what exists: the server is this process's `helion-hw`,
//! targets are the Simulation cable plus a board cable the last scan
//! resolved, debug cores come from the programmed target's hub, and the
//! configuration memory row appears once a part is added.

use helion_debug::DebugCoreKind;
use rune::{El, ScrollSpec, col, el, row, text};
use rune_kit::glyphs;

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::hw::CableChoice;

/// Tree column width.
pub const TREE_W: f32 = 232.0;
/// Indent per tree level.
const INDENT: f32 = 14.0;

/// Hardware tree node; the Hardware Manager shows its panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HwNode {
    Server,
    Target,
    #[default]
    Device,
    SetupDebug,
    Core(DebugCoreKind),
    CfgMem,
}

impl HwNode {
    pub fn key(self) -> String {
        match self {
            HwNode::Server => "hw-node-server".into(),
            HwNode::Target => "hw-node-target".into(),
            HwNode::Device => "hw-node-device".into(),
            HwNode::SetupDebug => "hw-node-setup".into(),
            HwNode::Core(k) => format!("hw-node-{}", k.as_str().replace('_', "-")),
            HwNode::CfgMem => "hw-node-cfgmem".into(),
        }
    }

    /// Short kind tag drawn before the row label.
    fn tag(self) -> &'static str {
        match self {
            HwNode::Server => "SRV",
            HwNode::Target => "TGT",
            HwNode::Device => "DEV",
            HwNode::SetupDebug => "SET",
            HwNode::Core(DebugCoreKind::Ila) => "ILA",
            HwNode::Core(DebugCoreKind::Vio) => "VIO",
            HwNode::Core(DebugCoreKind::JtagAxi) => "AXI",
            HwNode::Core(_) => "DBG",
            HwNode::CfgMem => "CFG",
        }
    }
}

/// Vivado-style hardware object name for a core (`hw_ila_1`).
pub fn hw_name(kind: DebugCoreKind, index: usize) -> String {
    let base = match kind {
        DebugCoreKind::Ila => "hw_ila",
        DebugCoreKind::Vio => "hw_vio",
        DebugCoreKind::JtagAxi => "hw_axi",
        DebugCoreKind::SysmonBridge => "hw_sysmon",
    };
    format!("{base}_{}", index + 1)
}

/// One visible tree row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub node: HwNode,
    pub level: u8,
    pub label: String,
    pub detail: String,
    /// Cable a target row selects.
    pub cable: Option<CableChoice>,
}

/// Rows for the current hardware state, in display order.
pub fn rows(s: &AppState) -> Vec<TreeRow> {
    let mut out = vec![TreeRow {
        node: HwNode::Server,
        level: 0,
        label: "localhost".into(),
        detail: "helion-hw".into(),
        cable: None,
    }];
    // Targets: the Simulation cable always; a board cable once resolved.
    let mut targets = vec![(CableChoice::Sim, "sim0".to_string(), "Simulation".to_string())];
    if s.program.cable != CableChoice::Sim {
        if let Some(info) = s.program.resolved() {
            targets.push((s.program.cable, info.id.clone(), info.backend.as_str().into()));
        } else {
            targets.push((
                s.program.cable,
                s.program.cable.spec().to_string(),
                "not resolved".into(),
            ));
        }
    }
    let part = crate::views::program::program_part(s);
    for (choice, id, detail) in targets {
        let active = s.program.cable == choice;
        out.push(TreeRow {
            node: HwNode::Target,
            level: 1,
            label: id,
            detail,
            cable: Some(choice),
        });
        if !active {
            continue;
        }
        let programmed = s.program.programmed_backend.is_some() || s.program.debug.programmed();
        out.push(TreeRow {
            node: HwNode::Device,
            level: 2,
            label: format!("{part}_0"),
            detail: if programmed {
                "programmed".into()
            } else {
                "idle".into()
            },
            cable: None,
        });
        out.push(TreeRow {
            node: HwNode::SetupDebug,
            level: 3,
            label: "Set Up Debug".into(),
            detail: if s.program.debug.build.is_some() {
                "built".into()
            } else {
                String::new()
            },
            cable: None,
        });
        if let Some(snap) = &s.program.debug.snap {
            for kind in [DebugCoreKind::Ila, DebugCoreKind::Vio, DebugCoreKind::JtagAxi] {
                for (i, c) in snap.cores.iter().filter(|c| c.kind == kind).enumerate() {
                    out.push(TreeRow {
                        node: HwNode::Core(kind),
                        level: 3,
                        label: hw_name(kind, i),
                        detail: c.name.clone(),
                        cable: None,
                    });
                }
            }
        }
        out.push(TreeRow {
            node: HwNode::CfgMem,
            level: 3,
            label: s
                .program
                .cfgmem
                .part
                .map(|p| p.name.to_string())
                .unwrap_or_else(|| "Add Configuration Memory…".into()),
            detail: String::new(),
            cable: None,
        });
    }
    out
}

/// Select a node (and, for a target row, its cable).
pub fn select(s: &mut AppState, node: HwNode, cable: Option<CableChoice>) {
    if let Some(c) = cable {
        if s.program.busy().is_none() {
            s.program.cable = c;
        }
    }
    s.program.node = node;
}

fn tree_row(i: usize, r: &TreeRow, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.program.node == r.node
        && (r.cable.is_none() || r.cable == Some(s.program.cable));
    let node = r.node;
    let cable = r.cable;
    let key = if r.node == HwNode::Target {
        format!("{}-{i}", r.node.key())
    } else {
        r.node.key()
    };
    let has_children = matches!(r.node, HwNode::Server | HwNode::Device)
        || (r.node == HwNode::Target && cable == Some(s.program.cable));
    let mut line = cad::table_row(key, format!("{} {}", r.label, r.detail), selected, t)
        .gap(4.0)
        .px(0.0)
        .on_press(move |s: &mut AppState| select(s, node, cable))
        .child(el().w(6.0 + INDENT * f32::from(r.level)))
        .child(if has_children {
            rune::icon(&glyphs::TRIANGLE_DOWN, 9.0)
                .color(t.text_dim)
                .a11y_hidden()
        } else {
            el().w(9.0)
        })
        .child(
            el().w(26.0).child(
                text(r.node.tag())
                    .font(cad::FONT_PIN.mono())
                    .color(t.text_faint),
            ),
        )
        .child(
            text(r.label.clone())
                .font(VivadoTheme::FONT_UI)
                .color(if r.label.ends_with('…') { t.link } else { t.text })
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .help_when_truncated(r.label.clone()),
        );
    if !r.detail.is_empty() {
        line = line.child(
            el().max_w(96.0).pr(6.0).child(
                text(r.detail.clone())
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim)
                    .truncate()
                    .help_when_truncated(r.detail.clone()),
            ),
        );
    } else {
        line = line.child(el().w(6.0));
    }
    line
}

/// The Hardware tree column.
pub fn tree(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().key("hw-tree-rows").w_full().gap(0.0);
    for (i, r) in rows(s).iter().enumerate() {
        body = body.child(tree_row(i, r, s, t));
    }
    col()
        .key("hw-tree")
        .role("tree")
        .label("Hardware")
        .w(TREE_W)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border_light)
        .child(
            row()
                .w_full()
                .h(cad::HEAD_H)
                .items_center()
                .px(8.0)
                .fill(t.header_bg)
                .child(cad::caps("Hardware", t)),
        )
        .child(
            col()
                .key("hw-tree-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tree_names_only_real_objects() {
        let mut s = AppState::demo();
        s.program.cable = CableChoice::Sim;
        let r = rows(&s);
        let nodes: Vec<HwNode> = r.iter().map(|r| r.node).collect();
        assert_eq!(
            nodes,
            vec![
                HwNode::Server,
                HwNode::Target,
                HwNode::Device,
                HwNode::SetupDebug,
                HwNode::CfgMem
            ]
        );
        assert_eq!(r[1].label, "sim0");
        assert_eq!(r[2].detail, "idle");
        assert!(r[4].label.starts_with("Add Configuration Memory"));
    }

    #[test]
    fn board_cable_adds_its_target_and_moves_the_device() {
        let mut s = AppState::demo();
        s.program.cable = CableChoice::Usb;
        let r = rows(&s);
        let targets: Vec<&TreeRow> = r.iter().filter(|r| r.node == HwNode::Target).collect();
        assert_eq!(targets.len(), 2);
        // The device hangs under the active (board) target, which is last.
        let dev = r.iter().position(|r| r.node == HwNode::Device).unwrap();
        let usb = r.iter().position(|r| r.cable == Some(CableChoice::Usb)).unwrap();
        assert!(dev > usb);
        select(&mut s, HwNode::Target, Some(CableChoice::Sim));
        assert_eq!(s.program.cable, CableChoice::Sim);
    }

    #[test]
    fn hw_names_follow_vivado() {
        assert_eq!(hw_name(DebugCoreKind::Ila, 0), "hw_ila_1");
        assert_eq!(hw_name(DebugCoreKind::JtagAxi, 1), "hw_axi_2");
    }
}
