//! Device floorplanning: the Pblocks table (old `paint_pblocks_table`,
//! `helion-ide.rs:5742`) and the `create_pblock` / `resize_pblock` /
//! `add_cells_to_pblock` / `delete_pblocks` / `get_pblocks` verbs.
//!
//! Pblocks are helion-proj [`helion_proj::PrjPblock`]s, parsed by the real
//! `.prj` parser (`helion_proj::load_prj`, the same round trip helion-proj's
//! own Tcl `cmd_pblock` uses), validated against the HAD CLB grid, and
//! persisted by appending the verb to the open project's `.prj` (so the
//! project reopens with them). The die overlay lives in
//! [`crate::views::device_layers`].

use std::path::{Path, PathBuf};

use helion_proj::PrjPblock;
use rune::{El, col, row};

use crate::state::{AppState, CenterPane};
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::device::DeviceSel;

/// Verbs this module owns (registered in `tcl::COMMANDS`).
pub const VERBS: &[&str] = &[
    "create_pblock",
    "resize_pblock",
    "add_cells_to_pblock",
    "delete_pblocks",
    "get_pblocks",
    "report_pblocks",
    "select_pblock",
    "device_zoom_fit",
];

/// Open project `.prj` the pblocks persist to, if a project file is open.
pub fn project_prj(s: &AppState) -> Option<&Path> {
    s.runs
        .source
        .as_deref()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("prj")) && p.is_file())
}

/// Load the open project's floorplan state once per project (wired into
/// the app poll): its pblocks, its DFX partitions
/// ([`crate::views::dfx::load_from_project`]) and any saved ECO session
/// ([`crate::views::eco::restore_for_project`]). Returns whether state
/// changed.
pub fn sync_from_project(s: &mut AppState) -> bool {
    let cur: Option<PathBuf> = project_prj(s).map(Path::to_path_buf);
    if cur == s.device.pblocks_from {
        return false;
    }
    s.device.pblocks_from = cur.clone();
    let Some(path) = cur else {
        return false;
    };
    let restored = crate::views::eco::restore_for_project(s, &path);
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| crate::pickers::load_prj_lenient(&t))
    {
        Ok(prj) => {
            s.device.pblocks = prj.pblocks;
            s.device.pblock_sel = None;
            crate::views::dfx::load_from_project(s, prj.partitions, &path);
            true
        }
        Err(_) => restored,
    }
}

/// Append one verb line to the open project's `.prj` (when one is open).
/// `Ok(Some(path))` when saved.
pub fn persist_line(s: &AppState, line: &str) -> Result<Option<PathBuf>, String> {
    let Some(path) = project_prj(s).map(Path::to_path_buf) else {
        return Ok(None);
    };
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{line}"))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(path))
}

/// Pblock names a `delete_pblocks` argument list names
/// (`[get_pblocks a]`, `{a b}`, `-hier` skipped).
fn named_pblocks(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|a| a.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .map(|a| a.trim_matches(|c: char| "{}[]".contains(c)).to_string())
        .filter(|a| !a.is_empty() && !a.starts_with('-') && a != "get_pblocks")
        .collect()
}

/// `delete_pblocks name…`: drop the pblocks (their members are released,
/// so placement and `PBLK-1` stop constraining them), unbind DFX
/// partitions on them, and persist the line to the open `.prj`.
fn delete_pblocks(s: &mut AppState, args: &[String]) -> Result<String, String> {
    let names = named_pblocks(args);
    if names.is_empty() {
        return Err("delete_pblocks: need a pblock name".into());
    }
    if let Some(n) = names.iter().find(|n| !s.device.pblocks.iter().any(|p| &p.name == *n)) {
        return Err(format!("delete_pblocks: no pblock {n}"));
    }
    let line = format!("delete_pblocks {}", names.join(" "));
    let next = apply_line(&s.device.pblocks, &line)?;
    let released: usize = s
        .device
        .pblocks
        .iter()
        .filter(|p| names.contains(&p.name))
        .map(|p| p.cells.len())
        .sum();
    s.device.pblocks = next;
    if s.device.pblock_sel.as_ref().is_some_and(|n| names.contains(n)) {
        s.device.pblock_sel = None;
    }
    for n in &names {
        crate::views::dfx::release_pblock(s, n);
    }
    s.device.bottom = DeviceBottom::Pblocks;
    let mut out = format!("{line} released={released}");
    match persist_line(s, &line) {
        Ok(Some(path)) => out.push_str(&format!(" (saved to {})", path.display())),
        Ok(None) => {}
        Err(e) => return Err(format!("delete_pblocks: applied but not saved: {e}")),
    }
    Ok(out)
}

/// Apply one pblock verb line through the real `.prj` parser.
fn apply_line(pblocks: &[PrjPblock], line: &str) -> Result<Vec<PrjPblock>, String> {
    let prj = helion_proj::ProjectFile {
        sources: vec!["__pblock__.sv".into()],
        pblocks: pblocks.to_vec(),
        ..Default::default()
    };
    let mut text = helion_proj::format_prj(&prj);
    text.push_str(line);
    text.push('\n');
    Ok(helion_proj::load_prj(&text)?.pblocks)
}

/// HAD CLB sites inside a ranged pblock.
pub fn clb_sites_in(dev: &helion_device::Device, pb: &PrjPblock) -> usize {
    if !pb.ranged {
        return 0;
    }
    dev.clb_sites()
        .filter(|s| s.x >= pb.x0 && s.x <= pb.x1 && s.y >= pb.y0 && s.y <= pb.y1)
        .count()
}

/// Member cells placed inside the pblock range, out of placed members.
pub fn placed_inside(s: &AppState, pb: &PrjPblock) -> (usize, usize) {
    let Some(placed) = crate::live::live_placed(s) else {
        return (0, 0);
    };
    let mut inside = 0;
    let mut total = 0;
    for c in &pb.cells {
        if let Some(p) = placed.site_of(c) {
            total += 1;
            if pb.ranged
                && p.site_x >= pb.x0
                && p.site_x <= pb.x1
                && p.site_y >= pb.y0
                && p.site_y <= pb.y1
            {
                inside += 1;
            }
        }
    }
    (inside, total)
}

/// CLB bounding range of placed cells, as `CLB_XaYb:CLB_XcYd`.
pub fn clb_range_of(s: &AppState, cells: &[String]) -> Option<String> {
    let placed = crate::live::live_placed(s)?;
    let mut bb: Option<(u32, u32, u32, u32)> = None;
    for c in cells {
        let Some(p) = placed.site_of(c) else { continue };
        if p.site_kind != "CLB" {
            continue;
        }
        bb = Some(match bb {
            None => (p.site_x, p.site_y, p.site_x, p.site_y),
            Some((a, b, c2, d)) => (
                a.min(p.site_x),
                b.min(p.site_y),
                c2.max(p.site_x),
                d.max(p.site_y),
            ),
        });
    }
    bb.map(|(a, b, c, d)| format!("CLB_X{a}Y{b}:CLB_X{c}Y{d}"))
}

fn pblock_text(s: &AppState) -> String {
    let (dev, _) = crate::live::live_device(s);
    let mut out = format!("pblocks n={}", s.device.pblocks.len());
    for pb in &s.device.pblocks {
        let (inside, total) = placed_inside(s, pb);
        out.push_str(&format!(
            "\nNAME={} RANGE={} CELLS={} SITES={} PLACED={inside}/{total}",
            pb.name,
            pb.range_text(),
            pb.cells.len(),
            clb_sites_in(&dev, pb)
        ));
    }
    out
}

/// Run one pblock verb (`argv` without the verb name).
pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    let line = std::iter::once(name.to_string())
        .chain(args.iter().map(|a| {
            if a.contains(' ') {
                format!("{{{a}}}")
            } else {
                a.clone()
            }
        }))
        .collect::<Vec<_>>()
        .join(" ");
    match name {
        "get_pblocks" => {
            // Patterns (`get_pblocks pb*`) filter; none lists every pblock.
            let pats = named_pblocks(args);
            Ok(s.device
                .pblocks
                .iter()
                .filter(|p| pats.is_empty() || pats.iter().any(|g| helion_proj::tcl_glob_match(g, &p.name)))
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(" "))
        }
        "report_pblocks" => {
            s.device.bottom = DeviceBottom::Pblocks;
            s.navigate(CenterPane::Device);
            Ok(pblock_text(s))
        }
        "select_pblock" => {
            let n = args.first().ok_or("select_pblock: need a pblock name")?;
            if !s.device.pblocks.iter().any(|p| &p.name == n) {
                return Err(format!("select_pblock: no pblock {n}"));
            }
            s.device.pblock_sel = Some(n.clone());
            s.device.bottom = DeviceBottom::Pblocks;
            s.navigate(CenterPane::Device);
            Ok(format!("select_pblock {n}"))
        }
        "device_zoom_fit" => {
            s.navigate(CenterPane::Device);
            Ok(crate::views::device_layers::fit(s))
        }
        "delete_pblocks" => delete_pblocks(s, args),
        "create_pblock" | "resize_pblock" | "add_cells_to_pblock" => {
            if name != "create_pblock" {
                let target = args
                    .iter()
                    .map(|a| a.trim_matches(|c: char| "{}[]".contains(c)))
                    .find(|a| !a.is_empty() && !a.starts_with('-') && *a != "get_pblocks")
                    .ok_or_else(|| format!("{name}: need a pblock name"))?;
                if !s.device.pblocks.iter().any(|p| p.name == target) {
                    return Err(format!("{name}: no pblock {target}"));
                }
            }
            if name == "add_cells_to_pblock" {
                let live = crate::live::live_netlist(s).ok_or("add_cells_to_pblock: no design")?;
                for c in args.iter().skip(1) {
                    let c = c.trim_matches(|ch: char| "{}[]".contains(ch));
                    if !c.is_empty() && c != "get_cells" && live.cell(c).is_none() {
                        return Err(format!("add_cells_to_pblock: no cell {c}"));
                    }
                }
            }
            let before: Vec<String> = s.device.pblocks.iter().map(|p| p.name.clone()).collect();
            let next = apply_line(&s.device.pblocks, &line).map_err(|e| e.to_string())?;
            let (dev, _) = crate::live::live_device(s);
            if let Some(bad) = next.iter().find(|p| p.ranged && clb_sites_in(&dev, p) == 0) {
                return Err(format!("{name}: {} has no HAD CLB sites", bad.range_text()));
            }
            let touched = next
                .iter()
                .find(|p| !before.contains(&p.name))
                .or_else(|| {
                    args.iter().find_map(|a| {
                        next.iter()
                            .find(|p| &p.name == a.trim_matches(|c: char| "{}[]".contains(c)))
                    })
                })
                .map(|p| p.name.clone());
            s.device.pblocks = next;
            if touched.is_some() {
                s.device.pblock_sel = touched.clone();
            }
            s.device.bottom = DeviceBottom::Pblocks;
            let mut out = line.clone();
            match persist_line(s, &line) {
                Ok(Some(path)) => out.push_str(&format!(" (saved to {})", path.display())),
                Ok(None) => {}
                Err(e) => return Err(format!("{name}: applied but not saved: {e}")),
            }
            if let Some(pb) = touched.and_then(|n| s.device.pblocks.iter().find(|p| p.name == n)) {
                out.push_str(&format!(
                    " range={} cells={}",
                    pb.range_text(),
                    pb.cells.len()
                ));
            }
            Ok(out)
        }
        _ => Err(format!("invalid command name \"{name}\"")),
    }
}

/// Bottom panel tabs under the die.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceBottom {
    #[default]
    IoPorts,
    Pblocks,
}

const COLS: [ColSpec; 5] = [
    ColSpec::grow("Name", 1.0),
    ColSpec::fixed("Range", 176.0),
    ColSpec::num("Cells", 48.0),
    ColSpec::num("Sites", 48.0),
    ColSpec::num("Placed in", 64.0),
];

/// Why Delete is unavailable (`None` = enabled).
pub fn delete_reason(s: &AppState) -> Option<&'static str> {
    s.device.pblock_sel.is_none().then_some("Select a pblock row to delete")
}

/// Next free `pblock_N` name.
fn next_name(s: &AppState) -> String {
    (0..)
        .map(|i| format!("pblock_{i}"))
        .find(|n| !s.device.pblocks.iter().any(|p| &p.name == n))
        .unwrap_or_else(|| "pblock".into())
}

/// Pblocks table: toolbar (Create, Add Selected Cell, Fit to Cells, Delete) over
/// Name / Range / Cells / Sites / Placed-in rows.
pub fn pblocks_table(s: &AppState, t: &VivadoTheme) -> El {
    let (dev, _) = crate::live::live_device(s);
    let sel_pb = s.device.pblock_sel.clone();
    let sel_cell = match &s.device.selected {
        Some(DeviceSel::Cell(c)) => Some(c.clone()),
        _ => None,
    };
    let has_design = crate::live::live_design(s).is_some();
    let create_reason = (!has_design).then_some("Synthesize a design first");
    let add_reason = match (&sel_pb, &sel_cell) {
        (None, _) => Some("Select a pblock row"),
        (_, None) => Some("Select a cell on the die"),
        (Some(pb), Some(c))
            if s.device
                .pblocks
                .iter()
                .any(|p| &p.name == pb && p.cells.contains(c)) =>
        {
            Some("Cell is already in this pblock")
        }
        _ => None,
    };
    let fit_cells: Option<String> = sel_pb.as_ref().and_then(|n| {
        let pb = s.device.pblocks.iter().find(|p| &p.name == n)?;
        clb_range_of(s, &pb.cells)
    });
    let fit_reason = match (&sel_pb, &fit_cells) {
        (None, _) => Some("Select a pblock row"),
        (_, None) => Some("No member cell is placed on a CLB"),
        _ => None,
    };
    let del_reason = delete_reason(s);
    let d_pb = sel_pb.clone();
    let name = next_name(s);
    let seed = sel_cell.clone();
    let (a_pb, a_cell) = (sel_pb.clone(), sel_cell.clone());
    let (f_pb, f_range) = (sel_pb.clone(), fit_cells.clone());
    let bar = row()
        .key("device-pblocks-bar")
        .w_full()
        .h(cad::TOOL_H)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .child(cad::title(
            format!("Pblocks ({})", s.device.pblocks.len()),
            t,
        ))
        .child(el_grow())
        .child(cad::btn(
            "device-pblock-create",
            "Create Pblock",
            t,
            create_reason,
            move |s: &mut AppState| {
                // Seed the range from the selected cell's CLB when there is one.
                let range = seed
                    .as_ref()
                    .and_then(|c| clb_range_of(s, std::slice::from_ref(c)));
                let verb = match range {
                    Some(r) => format!("create_pblock {name} -add {r}"),
                    None => format!("create_pblock {name}"),
                };
                crate::tcl::eval_command(s, &verb);
            },
        ))
        .child(cad::btn(
            "device-pblock-add-cell",
            "Add Selected Cell",
            t,
            add_reason,
            move |s: &mut AppState| {
                if let (Some(pb), Some(c)) = (&a_pb, &a_cell) {
                    crate::tcl::eval_command(s, &format!("add_cells_to_pblock {pb} {c}"));
                }
            },
        ))
        .child(cad::btn(
            "device-pblock-fit",
            "Fit to Cells",
            t,
            fit_reason,
            move |s: &mut AppState| {
                if let (Some(pb), Some(r)) = (&f_pb, &f_range) {
                    crate::tcl::eval_command(s, &format!("resize_pblock {pb} -add {r}"));
                }
            },
        ))
        .child(cad::btn(
            "device-pblock-delete",
            "Delete",
            t,
            del_reason,
            move |s: &mut AppState| {
                if let Some(pb) = &d_pb {
                    crate::tcl::eval_command(s, &format!("delete_pblocks {pb}"));
                }
            },
        ));
    let mut body = col().w_full();
    if s.device.pblocks.is_empty() {
        body = body.child(cad::empty_state(
            "device-pblocks-empty",
            "No pblocks yet. Select a placed cell and Create Pblock to floorplan around it.",
            None,
            t,
        ));
    }
    for (i, pb) in s.device.pblocks.iter().enumerate() {
        let on = sel_pb.as_deref() == Some(pb.name.as_str());
        let (inside, total) = placed_inside(s, pb);
        let n = pb.name.clone();
        body = body.child(
            cad::table_row(
                format!("device-pblock-row-{i}"),
                format!("Pblock {}", pb.name),
                on,
                t,
            )
            .on_press(move |s: &mut AppState| {
                crate::tcl::eval_command(s, &format!("select_pblock {n}"));
            })
            .child(cad::cell(
                &COLS[0],
                cad::cut(pb.name.clone(), cad::mono_style(), t.text),
            ))
            .child(cad::cell(
                &COLS[1],
                cad::cut(
                    if pb.ranged {
                        pb.range_text()
                    } else {
                        "unranged".into()
                    },
                    cad::mono_style(),
                    if pb.ranged { t.text } else { t.text_dim },
                ),
            ))
            .child(cad::cell(
                &COLS[2],
                cad::mono(pb.cells.len().to_string(), t),
            ))
            .child(cad::cell(
                &COLS[3],
                cad::mono(clb_sites_in(&dev, pb).to_string(), t),
            ))
            .child(cad::cell(
                &COLS[4],
                cad::mono(
                    if total == 0 {
                        "-".into()
                    } else {
                        format!("{inside}/{total}")
                    },
                    t,
                ),
            )),
        );
    }
    col()
        .key("device-pblocks")
        .w_full()
        .child(bar)
        .child(cad::header("device-pblocks-head", &COLS, t))
        .child(
            col()
                .w_full()
                .max_h(110.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
}

fn el_grow() -> El {
    rune::el().grow(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> AppState {
        let mut s = crate::init();
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        s
    }

    #[test]
    fn pblock_verbs_create_resize_and_add_cells() {
        let mut s = fixture();
        crate::tcl::eval_command(&mut s, "create_pblock pb0");
        assert_eq!(s.device.pblocks.len(), 1);
        assert!(!s.device.pblocks[0].ranged);
        let cell = crate::live::live_placed(&s)
            .unwrap()
            .cells
            .iter()
            .find(|c| c.site_kind == "CLB")
            .unwrap()
            .cell
            .clone();
        crate::tcl::eval_command(&mut s, &format!("add_cells_to_pblock pb0 {cell}"));
        assert_eq!(s.device.pblocks[0].cells, vec![cell.clone()]);
        let range = clb_range_of(&s, &[cell]).unwrap();
        crate::tcl::eval_command(&mut s, &format!("resize_pblock pb0 -add {range}"));
        let pb = &s.device.pblocks[0];
        assert!(pb.ranged);
        assert_eq!(placed_inside(&s, pb), (1, 1));
        let (dev, _) = crate::live::live_device(&s);
        assert_eq!(clb_sites_in(&dev, pb), 1);
        assert_eq!(s.device.pblock_sel.as_deref(), Some("pb0"));
    }

    #[test]
    fn pblock_verbs_refuse_bad_input() {
        let mut s = fixture();
        crate::tcl::eval_command(&mut s, "resize_pblock nope -add CLB_X1Y1:CLB_X2Y2");
        crate::tcl::eval_command(&mut s, "create_pblock pb1 -add CLB_X900Y900:CLB_X901Y901");
        crate::tcl::eval_command(&mut s, "create_pblock pb2");
        crate::tcl::eval_command(&mut s, "add_cells_to_pblock pb2 not_a_cell");
        let errs: Vec<&str> = s
            .console
            .lines
            .iter()
            .filter(|l| l.text.starts_with("ERROR"))
            .map(|l| l.text.as_str())
            .collect();
        assert!(
            errs.iter().any(|e| e.contains("no pblock nope")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|e| e.contains("has no HAD CLB sites")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|e| e.contains("no cell not_a_cell")),
            "{errs:?}"
        );
        assert_eq!(s.device.pblocks.len(), 1, "only the valid pblock lands");
        assert_eq!(
            crate::views::device_floor::tcl_verb(&mut s, "get_pblocks", &[]).unwrap(),
            "pb2"
        );
    }

    /// PBLK-1 rows for the current pblocks over the fixture placement
    /// (the same check `analysis::compute_drc` runs).
    fn pblk1(s: &AppState) -> usize {
        let ranged: Vec<PrjPblock> = s.device.pblocks.iter().filter(|p| p.ranged).cloned().collect();
        let regions = helion_proj::pblock_regions(crate::live::live_design(s).unwrap(), &ranged);
        helion_drc::check_pblocks(s.runs.live_placed.as_ref().unwrap(), &regions).items.len()
    }

    #[test]
    fn delete_pblocks_releases_members_persists_and_clears_drc() {
        let dir = std::env::temp_dir().join(format!("helion-pblock-del-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prj = dir.join("p.prj");
        std::fs::write(&prj, "part HL10T-C32-1\nread_sv top.sv\n").unwrap();
        let mut s = fixture();
        s.runs.source = Some(prj.clone());
        sync_from_project(&mut s);
        let clbs: Vec<(String, u32, u32)> = crate::live::live_placed(&s)
            .unwrap()
            .cells
            .iter()
            .filter(|c| c.site_kind == "CLB")
            .map(|c| (c.cell.clone(), c.site_x, c.site_y))
            .collect();
        let (cell, cx, cy) = clbs[0].clone();
        let (_, ox, oy) = clbs
            .iter()
            .find(|(_, x, y)| (*x, *y) != (cx, cy))
            .cloned()
            .expect("two CLB sites");
        // A pblock around another site that claims `cell`: PBLK-1 fires.
        crate::tcl::eval_command(&mut s, &format!("create_pblock pa -add CLB_X{ox}Y{oy}:CLB_X{ox}Y{oy}"));
        crate::tcl::eval_command(&mut s, &format!("add_cells_to_pblock pa {cell}"));
        crate::tcl::eval_command(&mut s, "create_pblock pb");
        assert_eq!(pblk1(&s), 1, "member outside its pblock");
        crate::views::dfx::create_partition(&mut s, Some("rp0".into()), Some("pa".into()), None).unwrap();
        assert_eq!(tcl_verb(&mut s, "get_pblocks", &["p*".into()]).unwrap(), "pa pb");
        assert_eq!(tcl_verb(&mut s, "get_pblocks", &["pa".into()]).unwrap(), "pa");
        s.device.pblock_sel = Some("pa".into());
        let out = tcl_verb(&mut s, "delete_pblocks", &["[get_pblocks".into(), "pa]".into()]).unwrap();
        assert!(out.contains("released=1") && out.contains("saved to"), "{out}");
        assert_eq!(tcl_verb(&mut s, "get_pblocks", &[]).unwrap(), "pb");
        assert_eq!(s.device.pblock_sel, None);
        assert_eq!(pblk1(&s), 0, "no PBLK-1 once the pblock is gone");
        assert_eq!(s.dfx.partitions[0].pblock, None, "the partition is unbound");
        assert!(tcl_verb(&mut s, "delete_pblocks", &["nope".into()]).unwrap_err().contains("no pblock nope"));
        assert!(tcl_verb(&mut s, "delete_pblocks", &[]).is_err());
        // A restart reloads the project without the deleted pblock.
        let mut s2 = fixture();
        s2.runs.source = Some(prj.clone());
        assert!(sync_from_project(&mut s2));
        let names: Vec<&str> = s2.device.pblocks.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["pb"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_button_needs_a_selection_and_deletes_on_tap() {
        let mut s = fixture();
        crate::tcl::eval_command(&mut s, "create_pblock pz");
        s.device.pblock_sel = None;
        assert_eq!(delete_reason(&s), Some("Select a pblock row to delete"));
        s.center = CenterPane::Device;
        s.device.bottom = DeviceBottom::Pblocks;
        let spec = crate::app_spec();
        let env = rune::Env { system_dark: true, scale: 2.0 };
        let mut rt = rune::Runtime::new(s, crate::view, spec.size, env);
        let mut now = 0.1;
        let mut tap = |rt: &mut rune::Runtime<AppState>, key: &str| {
            for _ in 0..4 {
                now += 1.0 / 60.0;
                rt.frame(now);
            }
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            now += 1.0 / 60.0;
            rt.frame(now);
        };
        tap(&mut rt, "device-pblock-delete");
        assert_eq!(rt.state.device.pblocks.len(), 1, "disabled Delete does nothing");
        rt.state.device.pblock_sel = Some("pz".into());
        rt.invalidate();
        assert_eq!(delete_reason(&rt.state), None);
        tap(&mut rt, "device-pblock-delete");
        assert!(rt.state.device.pblocks.is_empty(), "Delete removed the selected pblock");
    }

    #[test]
    fn project_pblocks_load_and_new_verbs_persist() {
        let dir = std::env::temp_dir().join(format!("helion-pblock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prj = dir.join("p.prj");
        std::fs::write(&prj, "part HL10T-C32-1\nread_sv top.sv\ncreate_pblock pa\nresize_pblock pa -add {CLB_X2Y2:CLB_X3Y3}\n").unwrap();
        let mut s = fixture();
        s.runs.source = Some(prj.clone());
        assert!(sync_from_project(&mut s));
        assert_eq!(s.device.pblocks.len(), 1);
        assert!(s.device.pblocks[0].ranged);
        assert!(!sync_from_project(&mut s), "loads once per project");
        crate::tcl::eval_command(&mut s, "create_pblock pb");
        let text = std::fs::read_to_string(&prj).unwrap();
        assert!(text.ends_with("create_pblock pb\n"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
