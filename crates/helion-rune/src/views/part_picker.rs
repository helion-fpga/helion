//! New Project "Default Part" page: board picker (`helion_device` boards)
//! and a searchable part list grouped by family, listing exactly the parts
//! a backend can implement (`helion_targets::implementable_parts`).
//! Picking a board selects its part and offers its pin constraints as an
//! XDC file for the new project.

use rune::{El, ScrollSpec, col, el, row};
use rune_kit::{Checkbox, Kit};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};

const PART_COLS: [ColSpec; 2] = [ColSpec::fixed("Part", 140.0), ColSpec::grow("Resources", 1.0)];

/// Parts and boards the wizard offers (loaded once per wizard).
#[derive(Clone, Debug, Default)]
pub struct PartCatalog {
    pub parts: Vec<helion_targets::TargetPart>,
    pub boards: Vec<helion_device::Board>,
}

impl PartCatalog {
    pub fn load() -> Self {
        let boards = helion_device::Device::list_boards()
            .unwrap_or_default()
            .iter()
            .filter_map(|b| helion_device::Device::load_board(b).ok())
            .collect();
        Self {
            parts: helion_targets::implementable_parts(),
            boards,
        }
    }

    pub fn board(&self, name: &str) -> Option<&helion_device::Board> {
        self.boards.iter().find(|b| b.board == name)
    }
}

fn matches(p: &helion_targets::TargetPart, q: &str) -> bool {
    let q = q.trim().to_ascii_lowercase();
    q.is_empty()
        || p.part.to_ascii_lowercase().contains(&q)
        || p.family.label().to_ascii_lowercase().contains(&q)
        || p.summary.to_ascii_lowercase().contains(&q)
}

/// What hardware has proven for `p`, shown beside its resources.
fn hw_status(p: &helion_targets::TargetPart) -> &'static str {
    use helion_targets::Family;
    match p.family {
        Family::Helion => "no silicon: Simulation cable only",
        Family::Ice40 | Family::Ecp5 => "not hardware-tested; Helion can't program it",
        Family::Xilinx7 if helion_hw::x7_board_hardware_tested(&p.part) => "board-tested",
        Family::Xilinx7 => "not hardware-tested",
    }
}

/// Why the 7-series parts are missing from the list, when they are: the
/// Project X-Ray database Helion reads them from was not found.
fn xilinx_hidden_note(parts: &[helion_targets::TargetPart]) -> Option<String> {
    let missing: Vec<&str> = ["xc7a35t", helion_targets::ZEDBOARD_PART]
        .into_iter()
        .filter(|m| !parts.iter().any(|p| p.part == *m))
        .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "Xilinx 7-series {} hidden: no Project X-Ray database with {} found. Set HELION_XILINX_DB to a prjxray-db checkout (default ~/.cache/helion/prjxray-db); see docs/xilinx-db.md.",
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" }
        )
    })
}

/// The download offer under the hidden-parts note: what it fetches and a
/// Download button (the click is the consent), then progress and Cancel.
fn xray_row(s: &AppState, t: &VivadoTheme, kit: &Kit) -> El {
    use crate::views::dialogs::dlg_button;
    let c = &s.dialogs.create;
    let mut r = col().key("create-part-xray-row").w_full().gap(4.0);
    if let Some(job) = &c.xray {
        r = r.child(
            row().w_full().items_center().gap(8.0).child(cad::dim(job.line(), t).key("create-part-xray-progress")).child(dlg_button(
                "create-part-xray-cancel",
                "Cancel",
                false,
                kit,
                |s: &mut AppState| {
                    if let Some(j) = &s.dialogs.create.xray {
                        j.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                },
            )),
        );
    } else if let Some(root) = helion_update::xray_db::install_root() {
        let missing = crate::xray::missing(&root);
        if !missing.is_empty() {
            r = r
                .child(cad::dim(format!("{}.", crate::xray::consent(&root, &missing)), t).wrap().w_full().key("create-part-xray-consent"))
                .child(dlg_button("create-part-xray-download", "Download database", true, kit, start_xray));
        }
    }
    if let Some(m) = &c.xray_msg {
        r = r.child(cad::dim(m.clone(), t).wrap().w_full().key("create-part-xray-msg"));
    }
    r
}

/// Start the X-Ray download on a worker. When it ends the part list is
/// reloaded, keeping the picked part picked.
pub fn start_xray(s: &mut AppState) {
    let Some(root) = helion_update::xray_db::install_root() else {
        s.dialogs.create.xray_msg = Some("No home directory to put the database in: set HELION_XILINX_DB to a directory.".into());
        return;
    };
    let families = crate::xray::missing(&root);
    let job = std::sync::Arc::new(crate::xray::Job::default());
    let worker_job = job.clone();
    let started = crate::bg::spawn(
        s,
        "xray-db",
        move || crate::xray::fetch(&root, &families, &worker_job),
        |s: &mut AppState, r: Result<String, String>| {
            let c = &mut s.dialogs.create;
            let picked = c.part_name().map(str::to_string);
            c.xray = None;
            c.xray_msg = Some(r.unwrap_or_else(|e| e));
            c.catalog = PartCatalog::load();
            c.part = picked.and_then(|p| c.catalog.parts.iter().position(|q| q.part == p)).unwrap_or(0);
        },
    );
    if started {
        s.dialogs.create.xray = Some(job);
        s.dialogs.create.xray_msg = None;
    }
}

/// Pick `board` (or none): selects the board's part.
pub fn pick_board(s: &mut AppState, board: Option<String>) {
    let c = &mut s.dialogs.create;
    if let Some(b) = board.as_deref().and_then(|b| c.catalog.board(b))
        && let Some(i) = c.catalog.parts.iter().position(|p| p.part == b.part)
    {
        c.part = i;
    }
    c.board_xdc = board
        .as_deref()
        .and_then(|b| c.catalog.board(b))
        .is_some_and(|b| !b.pins.is_empty());
    c.board = board;
    c.error = None;
}

fn chip(key: String, label: String, active: bool, t: &VivadoTheme, on: impl Fn(&mut AppState) + 'static) -> El {
    el().key(key)
        .role("radio")
        .label(label.clone())
        .focus_selected(active)
        .focusable(true)
        .h(22.0)
        .px(10.0)
        .center()
        .radius(11.0)
        .border(1.0, if active { t.accent } else { t.border })
        .fill(if active { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(on)
        .child(cad::cut(label, VivadoTheme::FONT_UI, t.text))
}

/// The page body.
pub fn part_page(s: &AppState, t: &VivadoTheme, kit: &Kit) -> El {
    let c = &s.dialogs.create;
    let mut boards = row().key("create-boards").w_full().items_center().gap(6.0).child(
        chip("create-board-none".into(), "No board".into(), c.board.is_none(), t, |s: &mut AppState| {
            pick_board(s, None)
        }),
    );
    for b in &c.catalog.boards {
        let name = b.board.clone();
        boards = boards.child(
            chip(
                format!("create-board-{}", b.board),
                format!("{} · {}", b.board, b.part),
                c.board.as_deref() == Some(b.board.as_str()),
                t,
                move |s: &mut AppState| pick_board(s, Some(name.clone())),
            )
            .help(if b.description.is_empty() { b.board.clone() } else { b.description.clone() }),
        );
    }
    let board_part = c.board.as_deref().and_then(|b| c.catalog.board(b)).map(|b| b.part.clone());
    let mut list = col().w_full();
    let mut shown = 0;
    let mut family = None;
    for (i, p) in c.catalog.parts.iter().enumerate() {
        if !matches(p, &c.part_filter.text) {
            continue;
        }
        if family != Some(p.family) {
            family = Some(p.family);
            let (ext, what) = p.family.artifact();
            list = list.child(
                row()
                    .key(format!("create-part-group-{}", p.family.label().replace(' ', "-")))
                    .w_full()
                    .h(cad::ROW_H)
                    .px(8.0)
                    .gap(8.0)
                    .items_center()
                    .fill(t.header_bg)
                    .child(cad::title(p.family.label(), t))
                    .child(cad::dim(format!("bitstream: .{ext} — {what}"), t).truncate()),
            );
        }
        shown += 1;
        let locked = board_part.as_ref().is_some_and(|bp| bp != &p.part);
        let mut r = cad::table_row(format!("create-part-row-{}", p.part), p.part.clone(), c.part == i, t)
            .child(cad::cell(&PART_COLS[0], cad::cut(p.part.clone(), cad::mono_style(), if locked { t.text_faint } else { t.text })))
            .child(cad::cell(&PART_COLS[1], cad::cut(format!("{} · {}", p.summary, hw_status(p)), VivadoTheme::FONT_UI, t.text_dim)));
        if locked {
            r = r
                .disabled(true)
                .description("The selected board fixes the part; choose No board to pick another.".to_string());
        } else {
            r = r.on_press(move |s: &mut AppState| {
                s.dialogs.create.part = i;
                s.dialogs.create.error = None;
            });
        }
        list = list.child(r);
    }
    if shown == 0 {
        list = list.child(row().h(cad::ROW_H).px(8.0).items_center().child(cad::dim("No implementable part matches the search.", t)));
    }
    let mut page = col()
        .key("create-part-page")
        .w_full()
        .gap(6.0)
        .child(cad::caps("Board", t))
        .child(boards)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(cad::caps("Parts", t))
                .child(cad::dim(format!("{shown} of {} implementable", c.catalog.parts.len()), t).key("create-part-count"))
                .child(el().grow(1.0))
                .child(
                    cad::search_input("create-part-search", &c.part_filter, "Search parts", t, |s: &mut AppState| {
                        &mut s.dialogs.create.part_filter
                    })
                    .w(200.0),
                ),
        )
        .child(
            cad::table_frame("create-part-table", t)
                .child(cad::header("create-part-head", &PART_COLS, t))
                .child(col().w_full().max_h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(list)),
        );
    if let Some(note) = xilinx_hidden_note(&c.catalog.parts) {
        page = page.child(cad::dim(note, t).wrap().w_full().key("create-part-xray")).child(xray_row(s, t, kit));
    } else if let Some(m) = &c.xray_msg {
        page = page.child(cad::dim(m.clone(), t).wrap().w_full().key("create-part-xray-msg"));
    }
    if let Some(b) = c.board.as_deref().and_then(|b| c.catalog.board(b)) {
        if b.pins.is_empty() {
            page = page.child(cad::dim(format!("{} declares no pin constraints.", b.board), t));
        } else {
            let on = c.board_xdc;
            page = page.child(
                row()
                    .key("create-board-xdc")
                    .gap(8.0)
                    .items_center()
                    .child(
                        Checkbox::new(on)
                            .label("Add board pin constraints")
                            .on_toggle::<AppState>(|s: &mut AppState| {
                                s.dialogs.create.board_xdc = !s.dialogs.create.board_xdc
                            })
                            .build(kit),
                    )
                    .child(cad::dim(
                        format!(
                            "{}_pins.xdc · {}",
                            b.board,
                            b.pins.iter().map(|p| format!("{}→{}", p.port, p.site)).collect::<Vec<_>>().join(", ")
                        ),
                        t,
                    )),
            );
        }
    }
    page
}

/// Write the board's XDC next to the new project; returns its path.
pub fn write_board_xdc(dir: &std::path::Path, board: &helion_device::Board) -> Result<std::path::PathBuf, String> {
    let path = dir.join(format!("{}_pins.xdc", board.board));
    std::fs::write(&path, board.constraints_xdc()).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_only_implementable_parts_and_board_selects_part() {
        let cat = PartCatalog::load();
        for p in &cat.parts {
            assert_eq!(helion_targets::family_of(&p.part), Some(p.family), "{}", p.part);
        }
        assert!(cat.parts.iter().any(|p| p.part == "ice40-hx8k"));
        let mut s = crate::init();
        s.dialogs.create.part = cat.parts.iter().position(|p| p.part == "LFE5U-85F").unwrap();
        pick_board(&mut s, Some("HB1".into()));
        let c = &s.dialogs.create;
        assert_eq!(c.catalog.parts[c.part].part, "HL10T-C32-1");
        assert!(c.board_xdc, "HB1 has pins, so the XDC is offered");
        assert!(!matches(&c.catalog.parts[0], "zzz"));
    }

    #[test]
    fn missing_xray_parts_are_named_with_the_fix() {
        let note = xilinx_hidden_note(&[]).expect("both hidden");
        assert!(note.contains("xc7a35t, xc7z020clg484-1") && note.contains("HELION_XILINX_DB") && note.contains("docs/xilinx-db.md"), "{note}");
        let part = |p: &str| helion_targets::TargetPart { part: p.into(), family: helion_targets::Family::Xilinx7, summary: String::new() };
        let both = [part("xc7a35t"), part(helion_targets::ZEDBOARD_PART)];
        assert_eq!(xilinx_hidden_note(&both), None);
        assert_eq!((hw_status(&both[0]), hw_status(&both[1])), ("not hardware-tested", "board-tested"));
    }
}

