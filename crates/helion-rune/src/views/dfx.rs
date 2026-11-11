//! Partial Reconfiguration (DFX) view: reconfigurable partitions bound to
//! floorplan pblocks, their reconfigurable modules, and the partial
//! bitstream run.
//!
//! Engine: `helion_proj::dfx::run_dfx` (per-module synth + exclusive
//! partition placement + route, `helion_fabric` partition pins, HDPR-1..8
//! DRC and `bitgen_rm` partial bitstreams). The static design is the
//! run's source (the open `.prj`'s SystemVerilog sources, else the active
//! RTL file); each module lists the HDL files defining the partition's
//! module. Partitions live in the session (the `.prj` format has no DFX
//! commands); the `create_partition` / `add_reconfig_module` /
//! `launch_dfx` verbs script them.

use std::path::PathBuf;

use helion_proj::dfx::{DfxModule, DfxPartition, DfxResult};
use rune::{El, Tx, col, el, row, text};

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, table};
use crate::views::sources::pane_header;

const TASK: &str = "dfx";

/// One partition being configured in the UI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartitionDraft {
    pub name: String,
    pub pblock: Option<String>,
    pub instance: Option<String>,
    pub modules: Vec<DfxModule>,
}

#[derive(Clone, Debug, Default)]
pub struct DfxState {
    pub partitions: Vec<PartitionDraft>,
    pub selected: usize,
    pub result: Option<Result<DfxResult, String>>,
}

fn log(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine { text, sev });
}

/// Static RTL files: the open `.prj`'s SV/V sources, else the run source.
pub fn static_sources(s: &AppState) -> Result<Vec<PathBuf>, String> {
    let src = crate::runs::resolve_source(s)?;
    if src.extension().is_some_and(|e| e.eq_ignore_ascii_case("prj")) {
        let text = std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        let prj = helion_proj::load_prj(&text)?;
        let files: Vec<PathBuf> = prj
            .sources
            .iter()
            .map(|f| helion_proj::resolve_prj_path(&src, f))
            .filter(|p| p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "sv" | "v")))
            .collect();
        if files.is_empty() {
            return Err("the project has no SystemVerilog sources".into());
        }
        Ok(files)
    } else {
        Ok(vec![src])
    }
}

/// Top-level hierarchy instances of the last run's design.
pub fn instances(s: &AppState) -> Vec<String> {
    let Some(d) = s.runs.live_design.as_ref() else { return Vec::new() };
    let all: Vec<String> = d.instances.iter().map(|i| helion_ir::resolve(i.name).to_string()).collect();
    all.iter()
        .filter(|i| !all.iter().any(|p| p.len() < i.len() && i.starts_with(&format!("{p}_"))))
        .cloned()
        .collect()
}

fn ranged_pblocks(s: &AppState) -> Vec<String> {
    s.device.pblocks.iter().filter(|p| p.ranged).map(|p| p.name.clone()).collect()
}

fn current(s: &AppState) -> Option<&PartitionDraft> {
    s.dfx.partitions.get(s.dfx.selected)
}

fn current_mut(s: &mut AppState) -> Option<&mut PartitionDraft> {
    let i = s.dfx.selected;
    s.dfx.partitions.get_mut(i)
}

/// `create_partition name ?pblock? ?instance?`.
pub fn create_partition(s: &mut AppState, name: Option<String>, pblock: Option<String>, instance: Option<String>) -> Result<String, String> {
    let name = name.unwrap_or_else(|| {
        (0..).map(|i| format!("rp{i}")).find(|n| !s.dfx.partitions.iter().any(|p| &p.name == n)).unwrap_or_default()
    });
    if s.dfx.partitions.iter().any(|p| p.name == name) {
        return Err(format!("create_partition: {name} exists"));
    }
    if let Some(pb) = &pblock {
        if !ranged_pblocks(s).contains(pb) {
            return Err(format!("create_partition: no ranged pblock {pb}"));
        }
    }
    s.dfx.partitions.push(PartitionDraft {
        name: name.clone(),
        pblock: pblock.or_else(|| ranged_pblocks(s).first().cloned()),
        instance: instance.or_else(|| instances(s).first().cloned()),
        modules: Vec::new(),
    });
    s.dfx.selected = s.dfx.partitions.len() - 1;
    Ok(format!("create_partition {name}"))
}

/// Add a module with `files` to the selected partition.
pub fn add_module(s: &mut AppState, partition: Option<&str>, name: Option<String>, files: Vec<PathBuf>) -> Result<String, String> {
    if files.is_empty() {
        return Err("add_reconfig_module: need at least one HDL file".into());
    }
    if let Some(p) = partition {
        s.dfx.selected = s.dfx.partitions.iter().position(|x| x.name == p).ok_or_else(|| format!("add_reconfig_module: no partition {p}"))?;
    }
    let part = current_mut(s).ok_or("add_reconfig_module: create a partition first")?;
    let name = name.unwrap_or_else(|| {
        files[0].file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| format!("rm{}", part.modules.len()))
    });
    if part.modules.iter().any(|m| m.name == name) {
        return Err(format!("add_reconfig_module: {name} exists in {}", part.name));
    }
    let pn = part.name.clone();
    part.modules.push(DfxModule { name: name.clone(), sources: files });
    s.dfx.result = None;
    Ok(format!("add_reconfig_module {pn} {name}"))
}

/// Why the run is unavailable (`None` = ready).
pub fn run_unavailable(s: &AppState) -> Option<String> {
    if crate::bg::busy(s, TASK) {
        return Some("Running…".into());
    }
    if s.runs.is_active() {
        return Some("A run is in progress".into());
    }
    let p = current(s)?;
    if p.pblock.is_none() {
        return Some("Pick a ranged pblock (Device ▸ Pblocks)".into());
    }
    if p.instance.is_none() {
        return Some("Pick the reconfigurable instance".into());
    }
    if p.modules.is_empty() {
        return Some("Add at least one reconfigurable module".into());
    }
    None
}

/// Run the selected partition's DFX flow on a worker.
pub fn launch(s: &mut AppState) -> Result<String, String> {
    if current(s).is_none() {
        return Err("launch_dfx: create a partition first".into());
    }
    if let Some(r) = run_unavailable(s) {
        return Err(format!("launch_dfx: {r}"));
    }
    let draft = current(s).cloned().ok_or("launch_dfx: no partition")?;
    let pblock = s
        .device
        .pblocks
        .iter()
        .find(|p| Some(&p.name) == draft.pblock.as_ref())
        .cloned()
        .ok_or("launch_dfx: the pblock no longer exists")?;
    let part = DfxPartition {
        name: draft.name.clone(),
        pblock,
        instance: draft.instance.clone().unwrap_or_default(),
        modules: draft.modules.clone(),
    };
    let statics = static_sources(s)?;
    let part_req = if s.project.part.is_empty() { "HL10T-C32-1".to_string() } else { s.project.part.clone() };
    let out = crate::bg::output_dir(s).join("dfx");
    crate::bg::spawn(
        s,
        TASK,
        move || {
            let dev = crate::runs::resolve_device(&part_req, &|_, _| {})?;
            helion_proj::dfx::run_dfx(&dev, &statics, &part, &out)
        },
        |s, r: Result<DfxResult, String>| {
            match &r {
                Ok(res) => {
                    for l in &res.log {
                        log(s, l.clone(), MsgSev::Info);
                    }
                    log(s, format!("launch_dfx {} modules={} drc={}", res.partition, res.modules.len(), res.drc.len()), MsgSev::Info);
                }
                Err(e) => log(s, format!("ERROR: launch_dfx: {e}"), MsgSev::Error),
            }
            s.dfx.result = Some(r);
        },
    );
    Ok("launch_dfx running".into())
}

fn pick_module_files(tx: &mut Tx) {
    let options = rune::FileDialogOptions {
        title: "Reconfigurable Module Sources".into(),
        ..Default::default()
    }
    .filter("HDL", &["sv", "v"]);
    tx.open_file(options, |s: &mut AppState, result, _| match result {
        Ok(Some(p)) => {
            let r = add_module(s, None, None, vec![p]);
            match r {
                Ok(v) => log(s, v, MsgSev::Info),
                Err(e) => log(s, format!("ERROR: {e}"), MsgSev::Error),
            }
        }
        Ok(None) => {}
        Err(e) => log(s, format!("ERROR: Add Module: {e}"), MsgSev::Error),
    });
}

fn chip(key: String, label: String, active: bool, t: &VivadoTheme, on: impl Fn(&mut AppState) + 'static) -> El {
    el().key(key)
        .role("button")
        .label(label.clone())
        .focusable(true)
        .h(22.0)
        .px(8.0)
        .center_y()
        .radius(3.0)
        .fill(if active { t.selection } else { t.panel_bg })
        .border(1.0, if active { t.accent } else { t.border })
        .hover(t.selection)
        .on_press(on)
        .child(text(label).font(VivadoTheme::FONT_UI).color(t.text))
}

fn picker_row(label: &'static str, chips: Vec<El>, empty: &str, t: &VivadoTheme) -> El {
    let mut r = row().w_full().items_center().gap(6.0).child(el().w(120.0).child(cad::dim(label, t)));
    if chips.is_empty() {
        r = r.child(cad::cut(empty.to_string(), VivadoTheme::FONT_UI, t.text_faint));
    }
    for c in chips {
        r = r.child(c);
    }
    r
}

fn partition_card(s: &AppState, p: &PartitionDraft, t: &VivadoTheme) -> El {
    let pbs: Vec<El> = ranged_pblocks(s)
        .into_iter()
        .map(|n| {
            let n2 = n.clone();
            chip(format!("dfx-pblock-{n}"), n.clone(), p.pblock.as_ref() == Some(&n), t, move |s: &mut AppState| {
                if let Some(p) = current_mut(s) {
                    p.pblock = Some(n2.clone());
                }
            })
        })
        .collect();
    let mut inst_names = instances(s);
    if let Some(sel) = &p.instance {
        if !inst_names.contains(sel) {
            inst_names.insert(0, sel.clone());
        }
    }
    let insts: Vec<El> = inst_names
        .into_iter()
        .map(|n| {
            let n2 = n.clone();
            chip(format!("dfx-inst-{n}"), n.clone(), p.instance.as_ref() == Some(&n), t, move |s: &mut AppState| {
                if let Some(p) = current_mut(s) {
                    p.instance = Some(n2.clone());
                }
            })
        })
        .collect();
    let head = header_row(t)
        .child(el().w(140.0).child(kit::header_cell("Module", t)))
        .child(kit::header_cell("Sources", t).grow(1.0).min_w(0.0))
        .child(el().w(64.0));
    let mut mods = col().w_full();
    if p.modules.is_empty() {
        mods = mods.child(kit::empty_state("dfx-modules-empty", "No reconfigurable modules — add the HDL that defines each variant.", None, t));
    }
    for (i, m) in p.modules.iter().enumerate() {
        let srcs = m.sources.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", ");
        mods = mods.child(
            plain_row(format!("dfx-module-{i}"))
                .child(kit::fixed_cell(m.name.clone(), 140.0, t.text))
                .child(kit::text_cell(srcs, 1.0, t.text_dim))
                .child(el().w(64.0).child(kit::link(format!("dfx-module-remove-{i}"), "Remove", t, move |s: &mut AppState| {
                    if let Some(p) = current_mut(s) {
                        if i < p.modules.len() {
                            p.modules.remove(i);
                        }
                    }
                    s.dfx.result = None;
                }))),
        );
    }
    let busy = crate::bg::busy(s, TASK);
    let mut add = el()
        .key("dfx-add-module")
        .role("button")
        .label("Add Module…")
        .h(kit::ROW_H)
        .px(kit::PAD_X)
        .center_y()
        .radius(3.0)
        .fill(t.panel_bg)
        .border(1.0, t.border);
    add = if busy {
        add.disabled(true).description("Running…").help("Running…").child(text("Add Module…").font(VivadoTheme::FONT_UI).color(t.text_faint))
    } else {
        add.focusable(true)
            .hover(t.selection)
            .on_press_tx(|_: &mut AppState, tx: &mut Tx| pick_module_files(tx))
            .child(text("Add Module…").font(VivadoTheme::FONT_UI).color(t.text))
    };
    let reason = run_unavailable(s);
    let run = kit::action_button("dfx-run", if busy { "Running…" } else { "Generate Partial Bitstreams" }, reason.as_deref(), t, |s: &mut AppState| {
        if let Err(e) = launch(s) {
            log(s, format!("ERROR: {e}"), MsgSev::Error);
            s.dfx.result = Some(Err(e));
        }
    });
    col()
        .key("dfx-partition")
        .w_full()
        .gap(8.0)
        .padding(8.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(picker_row("Pblock", pbs, "No ranged pblock — draw one in Device ▸ Pblocks.", t))
        .child(picker_row("Instance", insts, "No hierarchy instances — run Synthesis on a hierarchical top.", t))
        .child(row().w_full().items_center().gap(8.0).child(kit::section_title("Reconfigurable modules", t)).child(el().grow(1.0)).child(add))
        .child(table(head, mods, t))
        .child(row().w_full().items_center().child(el().grow(1.0)).child(run))
}

fn results(s: &AppState, t: &VivadoTheme) -> El {
    match &s.dfx.result {
        None if crate::bg::busy(s, TASK) => kit::empty_state("dfx-result-empty", "Implementing each module configuration…", None, t),
        None => el(),
        Some(Err(e)) => kit::empty_state("dfx-result-error", e, None, t),
        Some(Ok(r)) => {
            let head = header_row(t)
                .child(el().w(140.0).child(kit::header_cell("Module", t)))
                .child(kit::header_num("RM cells", 72.0, t))
                .child(kit::header_num("Frames", 64.0, t))
                .child(kit::header_num("Bytes", 72.0, t))
                .child(kit::header_cell("Partial bitstream", t).grow(1.0).min_w(0.0));
            let mut body = col().w_full();
            for (i, m) in r.modules.iter().enumerate() {
                body = body.child(
                    plain_row(format!("dfx-result-{i}"))
                        .child(kit::fixed_cell(m.module.clone(), 140.0, t.text))
                        .child(num_cell(m.rm_cells.to_string(), 72.0, t.text))
                        .child(num_cell(m.frames.to_string(), 64.0, t.text))
                        .child(num_cell(m.bytes.to_string(), 72.0, t.text))
                        .child(kit::text_cell(m.pbits.display().to_string(), 1.0, t.link)),
                );
            }
            let drc_head = header_row(t)
                .child(el().w(80.0).child(kit::header_cell("Rule", t)))
                .child(el().w(140.0).child(kit::header_cell("Objects", t)))
                .child(kit::header_cell("Message", t).grow(1.0).min_w(0.0));
            let mut drc = col().w_full();
            if r.drc.is_empty() {
                drc = drc.child(plain_row("dfx-drc-clean").child(kit::text_cell("No HDPR violations — the partition is legal.", 1.0, t.pass)));
            }
            for (i, f) in r.drc.iter().enumerate() {
                drc = drc.child(
                    plain_row(format!("dfx-drc-{i}"))
                        .child(kit::fixed_cell(f.code.clone(), 80.0, t.fail))
                        .child(kit::fixed_cell(f.objects.clone(), 140.0, t.text_dim))
                        .child(kit::text_cell(f.message.clone(), 1.0, t.text)),
                );
            }
            col()
                .key("dfx-results")
                .w_full()
                .gap(8.0)
                .child(row().w_full().gap(8.0)
                    .child(cad::kpi("dfx-kpi-modules", "Partial bitstreams", r.modules.len().to_string(), t.accent, format!("partition {}", r.partition), t))
                    .child(cad::kpi("dfx-kpi-partpins", "Partition pins", r.partpins.to_string(), t.text, "locked boundary arcs", t))
                    .child(cad::kpi("dfx-kpi-drc", "DFX DRC", r.drc.len().to_string(), if r.drc.is_empty() { t.pass } else { t.fail }, "HDPR-1..8 findings", t)))
                .child(cad::kv("Full bitstream", kit::text_cell(r.full_bit.display().to_string(), 1.0, t.link), 110.0, t))
                .child(kit::section_title("Partial bitstreams", t))
                .child(table(head, body, t))
                .child(kit::section_title("DFX DRC", t))
                .child(table(drc_head, drc, t))
        }
    }
}

pub fn dfx_pane(s: &AppState, t: &VivadoTheme) -> El {
    let mut tabs = row().key("dfx-partitions").w_full().gap(1.0).fill(t.border);
    for (i, p) in s.dfx.partitions.iter().enumerate() {
        tabs = tabs.child(cad::tab(format!("dfx-tab-{i}"), &p.name, s.dfx.selected == i, t, move |s: &mut AppState| s.dfx.selected = i));
    }
    let new_reason = crate::bg::busy(s, TASK).then_some("Running…");
    tabs = tabs.child(el().grow(1.0).h(24.0).fill(t.header_bg));
    let new_btn = kit::action_button("dfx-new", "New Partition", new_reason, t, |s: &mut AppState| {
        match create_partition(s, None, None, None) {
            Ok(v) => log(s, v, MsgSev::Info),
            Err(e) => log(s, format!("ERROR: {e}"), MsgSev::Error),
        }
    });
    let mut body = col().w_full().gap(10.0).padding(8.0).child(
        row().w_full().h(28.0).items_center().gap(8.0)
            .child(cad::cut("Reconfigurable partitions are pblocks whose instance swaps between module variants.", VivadoTheme::FONT_UI, t.text_dim).grow(1.0).min_w(0.0))
            .child(new_btn),
    );
    match current(s) {
        Some(p) => body = body.child(tabs).child(partition_card(s, p, t)).child(results(s, t)),
        None => body = body.child(kit::empty_state("dfx-empty", "No reconfigurable partitions yet.", None, t)),
    }
    col()
        .key("dfx-pane")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Partial Reconfiguration", "dfx", t))
        .child(col().key("dfx-scroll").grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(body))
}

pub const VERBS: &[&str] = &["create_partition", "add_reconfig_module", "launch_dfx", "report_partitions"];

pub fn help(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "create_partition" => ("create_partition ?name? ?pblock? ?instance?", "make a pblock + instance a reconfigurable partition"),
        "add_reconfig_module" => ("add_reconfig_module partition name files…", "add a reconfigurable module variant (its HDL files)"),
        "launch_dfx" => ("launch_dfx ?partition?", "implement every module and write partial bitstreams"),
        "report_partitions" => ("report_partitions", "list partitions, pblocks, instances and modules"),
        _ => return None,
    })
}

pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    s.navigate(CenterPane::Dfx);
    match name {
        "create_partition" => create_partition(s, args.first().cloned(), args.get(1).cloned(), args.get(2).cloned()),
        "add_reconfig_module" => {
            let p = args.first().ok_or("add_reconfig_module: need partition name files…")?.clone();
            let n = args.get(1).ok_or("add_reconfig_module: need a module name")?.clone();
            add_module(s, Some(&p), Some(n), args.iter().skip(2).map(PathBuf::from).collect())
        }
        "launch_dfx" => {
            if let Some(p) = args.first() {
                s.dfx.selected = s.dfx.partitions.iter().position(|x| &x.name == p).ok_or_else(|| format!("launch_dfx: no partition {p}"))?;
            }
            launch(s)
        }
        "report_partitions" => Ok(s
            .dfx
            .partitions
            .iter()
            .map(|p| format!(
                "{} pblock={} instance={} modules={}",
                p.name,
                p.pblock.as_deref().unwrap_or("-"),
                p.instance.as_deref().unwrap_or("-"),
                p.modules.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(",")
            ))
            .collect::<Vec<_>>()
            .join("\n")),
        _ => Err(format!("{name}: not a DFX verb")),
    }
}

/// Scene fixture: a real two-module DFX run over generated temp sources.
pub fn scene_fill(s: &mut AppState) {
    let dir = std::env::temp_dir().join("helion-rune-scenes").join("dfx");
    let _ = std::fs::create_dir_all(&dir);
    let top = dir.join("top.v");
    let inv = dir.join("rm_inv.v");
    let buf = dir.join("rm_buf.v");
    let _ = std::fs::write(&top, "module top(input clk, output led);\n  reg [3:0] cnt = 0;\n  always @(posedge clk) cnt <= cnt + 1;\n  wire y;\n  rm u_rm (.clk(clk), .a(cnt[3]), .y(y));\n  assign led = y;\nendmodule\n");
    let _ = std::fs::write(&inv, "module rm(input clk, input a, output reg y);\n  always @(posedge clk) y <= ~a;\nendmodule\n");
    let _ = std::fs::write(&buf, "module rm(input clk, input a, output reg y);\n  reg t;\n  always @(posedge clk) begin t <= a; y <= t; end\nendmodule\n");
    let pb = helion_proj::PrjPblock { name: "pblock_rp0".into(), x0: 6, y0: 5, x1: 8, y1: 8, ranged: true, cells: vec![] };
    s.device.pblocks = vec![pb.clone()];
    let modules = vec![
        DfxModule { name: "rm_inv".into(), sources: vec![inv] },
        DfxModule { name: "rm_buf".into(), sources: vec![buf] },
    ];
    s.dfx.partitions = vec![PartitionDraft { name: "rp0".into(), pblock: Some(pb.name.clone()), instance: Some("u_rm".into()), modules: modules.clone() }];
    if let Ok(dev) = helion_device::Device::load_part("HL10T-C32-1") {
        let part = DfxPartition { name: "rp0".into(), pblock: pb, instance: "u_rm".into(), modules };
        s.dfx.result = Some(helion_proj::dfx::run_dfx(&dev, &[top], &part, &dir.join("out")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dfx_verbs_build_a_partition_and_run_it_off_thread() {
        let mut s = AppState::demo();
        scene_fill(&mut s);
        let dir = std::env::temp_dir().join("helion-rune-scenes").join("dfx");
        s.dfx = DfxState::default();
        s.runs.source = Some(dir.join("top.v"));
        assert!(launch(&mut s).is_err(), "no partition yet");
        tcl_verb(&mut s, "create_partition", &["rp0".into(), "pblock_rp0".into(), "u_rm".into()]).unwrap();
        assert!(run_unavailable(&s).unwrap().contains("module"));
        tcl_verb(&mut s, "add_reconfig_module", &["rp0".into(), "inv".into(), dir.join("rm_inv.v").display().to_string()]).unwrap();
        tcl_verb(&mut s, "add_reconfig_module", &["rp0".into(), "buf".into(), dir.join("rm_buf.v").display().to_string()]).unwrap();
        assert!(tcl_verb(&mut s, "add_reconfig_module", &["rp0".into(), "buf".into(), "x.v".into()]).is_err());
        assert!(tcl_verb(&mut s, "report_partitions", &[]).unwrap().contains("modules=inv,buf"));
        tcl_verb(&mut s, "launch_dfx", &[]).unwrap();
        crate::bg::wait_all(&mut s);
        let r = s.dfx.result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(r.modules.len(), 2);
        assert!(r.modules.iter().all(|m| m.frames > 0));
        assert_eq!(s.center, CenterPane::Dfx);
    }
}
