//! ECO view: edit the implemented netlist and write an incremental
//! bitstream (UG904 ECO flow).
//!
//! Engine: `helion_proj::eco` — [`helion_proj::eco::EcoOp`] edits
//! (LUT `INIT`, pin reconnect, buffer insertion, add/remove cell) applied
//! by `Session::apply_eco`, legalized by `Session::eco_implement`
//! (incremental place + route, or route only), and written by
//! `Session::write_eco_bitstream` (full image plus the frame diff versus
//! the pre-ECO image as a `.pbits` partial). The session is built from the
//! last implementation run's routed design; every engine step runs on a
//! [`crate::bg`] worker that takes the session and hands it back.

use std::path::PathBuf;

use helion_proj::eco::{EcoImpl, EcoOp};
use rune::{El, TextEdit, col, el, row, text};

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::report_kit::{self as kit, header_row, num_cell, plain_row, table};
use crate::views::sources::pane_header;

const TASK: &str = "eco";

/// Which edit the form builds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EcoKind {
    #[default]
    LutInit,
    Reconnect,
    Buffer,
    AddLut,
    AddFf,
    Remove,
}

impl EcoKind {
    pub const ALL: [EcoKind; 6] = [
        EcoKind::LutInit,
        EcoKind::Reconnect,
        EcoKind::Buffer,
        EcoKind::AddLut,
        EcoKind::AddFf,
        EcoKind::Remove,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EcoKind::LutInit => "LUT INIT",
            EcoKind::Reconnect => "Reconnect Pin",
            EcoKind::Buffer => "Insert Buffer",
            EcoKind::AddLut => "Add LUT",
            EcoKind::AddFf => "Add Flip-Flop",
            EcoKind::Remove => "Remove Cell",
        }
    }

    fn key(self) -> &'static str {
        match self {
            EcoKind::LutInit => "eco-kind-init",
            EcoKind::Reconnect => "eco-kind-reconnect",
            EcoKind::Buffer => "eco-kind-buffer",
            EcoKind::AddLut => "eco-kind-addlut",
            EcoKind::AddFf => "eco-kind-addff",
            EcoKind::Remove => "eco-kind-remove",
        }
    }
}

/// One applied change.
#[derive(Clone, Debug)]
pub struct EcoRow {
    pub op: EcoOp,
    pub result: Result<String, String>,
}

/// Files written by Write ECO Bitstream.
#[derive(Clone, Debug)]
pub struct EcoOutput {
    pub bit: PathBuf,
    pub pbits: PathBuf,
    pub full_frames: usize,
    pub changed_frames: usize,
}

#[derive(Clone, Debug)]
pub struct EcoState {
    pub session: Option<Box<helion_proj::Session>>,
    pub part: String,
    /// Pre-ECO bitstream (the incremental diff base).
    pub base: Option<helion_bits::Bitstream>,
    pub ops: Vec<EcoRow>,
    /// Structural changes not yet placed/routed.
    pub pending: bool,
    pub kind: EcoKind,
    pub cell: TextEdit,
    pub pin: TextEdit,
    pub net: TextEdit,
    pub value: TextEdit,
    pub status: Option<Result<String, String>>,
    pub output: Option<Result<EcoOutput, String>>,
}

impl Default for EcoState {
    fn default() -> Self {
        Self {
            session: None,
            part: String::new(),
            base: None,
            ops: Vec::new(),
            pending: false,
            kind: EcoKind::default(),
            cell: TextEdit::new(""),
            pin: TextEdit::new(""),
            net: TextEdit::new(""),
            value: TextEdit::new(""),
            status: None,
            output: None,
        }
    }
}

fn log(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine { text, sev });
}

fn note(s: &mut AppState, r: Result<String, String>) {
    match &r {
        Ok(v) => log(s, v.clone(), MsgSev::Info),
        Err(e) => log(s, format!("ERROR: {e}"), MsgSev::Error),
    }
    s.eco.status = Some(r);
}

/// Why the ECO session cannot be opened (`None` = ready).
pub fn open_unavailable(s: &AppState) -> Option<&'static str> {
    if crate::bg::busy(s, TASK) {
        Some("Working…")
    } else if s.runs.is_active() {
        Some("A run is in progress")
    } else if s.runs.analysis.as_ref().is_none_or(|a| a.routed.is_none()) {
        Some("Run Implementation first")
    } else {
        None
    }
}

/// Open the ECO session on the last implementation run (worker: rebuild
/// the session and its pre-ECO bitstream).
pub fn open(s: &mut AppState) -> Result<String, String> {
    if let Some(r) = open_unavailable(s) {
        return Err(format!("eco_open: {r}"));
    }
    let ctx = s.runs.analysis.clone().ok_or("eco_open: no implementation run")?;
    let routed = ctx.routed.ok_or("eco_open: the last run is not routed")?;
    let design = s.runs.live_design.clone().ok_or("eco_open: the last run kept no design")?;
    let part = ctx.part;
    let started = crate::bg::spawn(
        s,
        TASK,
        move || -> Result<(helion_proj::Session, helion_bits::Bitstream, String), String> {
            let dev = crate::runs::resolve_device(&part, &|_, _| {})?;
            let mut session = helion_proj::Session::new(helion_proj::Mode::Project);
            session.part = dev.part.clone();
            session.synth_design(design);
            session.packed = Some(routed.placed.packed.clone());
            session.placed = Some(routed.placed.clone());
            session.impl_checkpoint = Some(routed.placed.clone());
            session.routed = Some(routed);
            let base = session.write_bitstream(&dev)?.clone();
            Ok((session, base, dev.part))
        },
        |s, r| match r {
            Ok((session, base, part)) => {
                let cells = session.design.as_ref().map(|d| d.cells.len()).unwrap_or(0);
                s.eco = EcoState {
                    kind: s.eco.kind,
                    session: Some(Box::new(session)),
                    part,
                    ops: Vec::new(),
                    output: None,
                    pending: false,
                    status: None,
                    ..std::mem::take(&mut s.eco)
                };
                note(
                    s,
                    Ok(format!(
                        "eco_open cells={cells} base_frames={}",
                        base.frames.len()
                    )),
                );
                s.eco.base = Some(base);
            }
            Err(e) => note(s, Err(format!("eco_open: {e}"))),
        },
    );
    if started { Ok("eco_open running".into()) } else { Err("eco: busy".into()) }
}

fn parse_init(v: &str) -> Result<u64, String> {
    let t = v.trim().trim_start_matches("64'h").trim_start_matches("0x").trim_start_matches("0X").replace('_', "");
    u64::from_str_radix(&t, 16).map_err(|_| format!("eco: INIT {v:?} is not a hex value"))
}

/// The op the form describes, or what is missing.
pub fn form_op(s: &AppState) -> Result<EcoOp, String> {
    let e = &s.eco;
    let need = |f: &TextEdit, what: &str| {
        let v = f.text.trim().to_string();
        if v.is_empty() { Err(format!("Enter the {what}")) } else { Ok(v) }
    };
    Ok(match e.kind {
        EcoKind::LutInit => EcoOp::SetLutInit { cell: need(&e.cell, "LUT cell")?, init: parse_init(&need(&e.value, "INIT (hex)")?)? },
        EcoKind::Reconnect => EcoOp::ReconnectPin { cell: need(&e.cell, "cell")?, pin: need(&e.pin, "pin")?, net: need(&e.net, "net")? },
        EcoKind::Buffer => EcoOp::InsertBuffer { net: need(&e.net, "net")?, name: need(&e.cell, "buffer name")? },
        EcoKind::AddLut => EcoOp::AddLut { name: need(&e.cell, "LUT name")?, init: parse_init(&need(&e.value, "INIT (hex)")?)? },
        EcoKind::AddFf => EcoOp::AddFf { name: need(&e.cell, "flip-flop name")?, clk: need(&e.net, "clock net")? },
        EcoKind::Remove => EcoOp::RemoveCell { cell: need(&e.cell, "cell")? },
    })
}

/// Apply `op` on the worker.
pub fn apply(s: &mut AppState, op: EcoOp) -> Result<String, String> {
    if crate::bg::busy(s, TASK) {
        return Err("eco: busy".into());
    }
    let mut session = s.eco.session.take().ok_or("eco: open the implemented design first (eco_open)")?;
    let part = s.eco.part.clone();
    let op2 = op.clone();
    crate::bg::spawn(
        s,
        TASK,
        move || {
            let r = crate::runs::resolve_device(&part, &|_, _| {}).and_then(|dev| session.apply_eco(&dev, &op2));
            (session, r)
        },
        move |s, (session, r): (Box<helion_proj::Session>, Result<String, String>)| {
            s.eco.session = Some(session);
            if r.is_ok() && op.is_structural() {
                s.eco.pending = true;
            }
            if r.is_ok() {
                s.eco.output = None;
            }
            s.eco.ops.push(EcoRow { op, result: r.clone() });
            note(s, r);
        },
    );
    Ok("eco running".into())
}

/// Legalize (incremental place + route, or route only) on the worker.
pub fn implement(s: &mut AppState, how: EcoImpl) -> Result<String, String> {
    if crate::bg::busy(s, TASK) {
        return Err("eco: busy".into());
    }
    let mut session = s.eco.session.take().ok_or("eco: open the implemented design first (eco_open)")?;
    let part = s.eco.part.clone();
    crate::bg::spawn(
        s,
        TASK,
        move || {
            let r = crate::runs::resolve_device(&part, &|_, _| {}).and_then(|dev| session.eco_implement(&dev, how));
            (session, r)
        },
        move |s, (session, r): (Box<helion_proj::Session>, Result<Option<helion_proj::ReuseReport>, String>)| {
            s.eco.session = Some(session);
            let verb = if how == EcoImpl::RouteOnly { "route_design -eco" } else { "place_design -incremental; route_design" };
            let r = r.map(|rep| match rep {
                Some(rep) => format!("{verb} {}", rep.text()),
                None => format!("{verb} done"),
            });
            if r.is_ok() {
                s.eco.pending = false;
            }
            note(s, r);
        },
    );
    Ok("eco implement running".into())
}

/// Write the post-ECO full bitstream and the incremental `.pbits`.
pub fn write_bitstream(s: &mut AppState) -> Result<String, String> {
    if crate::bg::busy(s, TASK) {
        return Err("eco: busy".into());
    }
    if s.eco.pending {
        return Err("write_eco_bitstream: place and route the netlist changes first".into());
    }
    let base = s.eco.base.clone().ok_or("write_eco_bitstream: open the implemented design first")?;
    let mut session = s.eco.session.take().ok_or("write_eco_bitstream: open the implemented design first")?;
    let part = s.eco.part.clone();
    let dir = crate::bg::output_dir(s).join("eco");
    let n = s.eco.ops.iter().filter(|o| o.result.is_ok()).count();
    crate::bg::spawn(
        s,
        TASK,
        move || {
            let r = (|| {
                let dev = crate::runs::resolve_device(&part, &|_, _| {})?;
                let out = session.write_eco_bitstream(&dev, &base)?;
                let top = session.design.as_ref().map(|d| helion_ir::resolve(d.name).to_string()).unwrap_or_else(|| "design".into());
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                let bit = dir.join(format!("{top}_eco{n}.bit"));
                let pbits = dir.join(format!("{top}_eco{n}.pbits"));
                std::fs::write(&bit, &out.full.packets).map_err(|e| format!("{}: {e}", bit.display()))?;
                std::fs::write(&pbits, &out.incremental.packets).map_err(|e| format!("{}: {e}", pbits.display()))?;
                Ok(EcoOutput {
                    bit,
                    pbits,
                    full_frames: out.full.frames.len(),
                    changed_frames: out.incremental.frames.len(),
                })
            })();
            (session, r)
        },
        |s, (session, r): (Box<helion_proj::Session>, Result<EcoOutput, String>)| {
            s.eco.session = Some(session);
            let line = r.as_ref().map(|o| {
                format!(
                    "write_eco_bitstream {} changed_frames={}/{} incremental={}",
                    o.bit.display(),
                    o.changed_frames,
                    o.full_frames,
                    o.pbits.display()
                )
            });
            note(s, line.map_err(|e| e.clone()));
            s.eco.output = Some(r);
        },
    );
    Ok("write_eco_bitstream running".into())
}

fn busy_or(s: &AppState, other: Option<&'static str>) -> Option<&'static str> {
    if crate::bg::busy(s, TASK) { Some("Working…") } else { other }
}

fn field(
    key: &'static str,
    label: &'static str,
    edit: &TextEdit,
    placeholder: &'static str,
    t: &VivadoTheme,
    on_edit: impl for<'a> Fn(&'a mut AppState) -> &'a mut TextEdit + 'static,
) -> El {
    col()
        .gap(2.0)
        .grow(1.0)
        .min_w(0.0)
        .child(cad::caps(label, t))
        .child(
            row()
                .h(22.0)
                .items_center()
                .px(6.0)
                .radius(3.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .child(
                    rune::text_input(edit, VivadoTheme::FONT_MONO.mono())
                        .single_line()
                        .key(key)
                        .label(label)
                        .grow(1.0)
                        .min_w(0.0)
                        .color(t.text)
                        .placeholder(placeholder, t.text_faint)
                        .caret(t.text, t.selection)
                        .on_edit(on_edit),
                ),
        )
}

fn current_init(s: &AppState) -> Option<String> {
    let d = s.eco.session.as_ref()?.design.as_ref()?;
    let c = d.cell(s.eco.cell.text.trim())?;
    match c.kind {
        helion_ir::CellKind::Lut6 { init } => Some(format!("current INIT 64'h{init:016X}")),
        _ => Some("not a LUT".into()),
    }
}

fn form(s: &AppState, t: &VivadoTheme) -> El {
    let e = &s.eco;
    let mut tabs = row().key("eco-kinds").w_full().gap(1.0).fill(t.border);
    for k in EcoKind::ALL {
        tabs = tabs.child(cad::tab(k.key(), k.label(), e.kind == k, t, move |s: &mut AppState| s.eco.kind = k));
    }
    tabs = tabs.child(el().grow(1.0).h(24.0).fill(t.header_bg));
    let cell = |label, ph| field("eco-field-cell", label, &e.cell, ph, t, |s: &mut AppState| &mut s.eco.cell);
    let pin = field("eco-field-pin", "Pin", &e.pin, "I0", t, |s: &mut AppState| &mut s.eco.pin);
    let net = |label, ph| field("eco-field-net", label, &e.net, ph, t, |s: &mut AppState| &mut s.eco.net);
    let value = field("eco-field-value", "INIT (hex)", &e.value, "64'h0000000000000002", t, |s: &mut AppState| &mut s.eco.value);
    let fields = match e.kind {
        EcoKind::LutInit => row().gap(8.0).child(cell("LUT cell", "u_lut0")).child(value),
        EcoKind::Reconnect => row().gap(8.0).child(cell("Cell", "u_lut1")).child(pin).child(net("New net", "q2")),
        EcoKind::Buffer => row().gap(8.0).child(net("Net", "q0")).child(cell("Buffer name", "eco_buf")),
        EcoKind::AddLut => row().gap(8.0).child(cell("LUT name", "eco_lut")).child(value),
        EcoKind::AddFf => row().gap(8.0).child(cell("Flip-flop name", "eco_ff")).child(net("Clock net", "clk")),
        EcoKind::Remove => row().gap(8.0).child(cell("Cell", "u_lut3")),
    };
    let op = form_op(s);
    let reason = busy_or(s, e.session.is_none().then_some("Open the implemented design first"))
        .map(str::to_string)
        .or_else(|| op.as_ref().err().cloned());
    let hint = match e.kind {
        EcoKind::LutInit => current_init(s).unwrap_or_else(|| "Patched straight into the bitstream — no place or route.".into()),
        EcoKind::Reconnect | EcoKind::Buffer | EcoKind::AddLut | EcoKind::AddFf | EcoKind::Remove => {
            "Changes connectivity: place and route before writing the bitstream.".into()
        }
    };
    let apply_btn = kit::action_button("eco-apply", "Apply Change", reason.as_deref(), t, |s: &mut AppState| {
        if let Ok(op) = form_op(s) {
            let _ = apply(s, op);
        }
    });
    col()
        .key("eco-form")
        .w_full()
        .gap(8.0)
        .padding(8.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(tabs)
        .child(fields.w_full())
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(cad::cut(hint, VivadoTheme::FONT_UI, t.text_dim).grow(1.0).min_w(0.0).key("eco-hint"))
                .child(apply_btn),
        )
}

fn changes(s: &AppState, t: &VivadoTheme) -> El {
    let head = header_row(t)
        .child(kit::header_num("#", 28.0, t))
        .child(kit::header_cell("Change", t).grow(1.0).min_w(0.0))
        .child(el().w(220.0).child(kit::header_cell("Result", t)));
    let mut body = col().w_full();
    if s.eco.ops.is_empty() {
        body = body.child(kit::empty_state("eco-changes-empty", "No changes yet.", None, t));
    }
    for (i, r) in s.eco.ops.iter().enumerate() {
        let (res, color) = match &r.result {
            Ok(_) if !r.op.is_structural() => ("Bitstream patched".to_string(), t.pass),
            Ok(_) => ("Netlist edited".to_string(), t.text_dim),
            Err(e) => (e.clone(), t.fail),
        };
        body = body.child(
            plain_row(format!("eco-change-{i}"))
                .child(num_cell((i + 1).to_string(), 28.0, t.text_dim))
                .child(kit::text_cell(r.op.text(), 1.0, t.text))
                .child(kit::fixed_cell(res, 220.0, color)),
        );
    }
    col().w_full().gap(2.0).child(kit::section_title("Changes", t)).child(table(head, body, t).key("eco-changes"))
}

fn actions(s: &AppState, t: &VivadoTheme) -> El {
    let none = s.eco.session.is_none().then_some("Open the implemented design first");
    let pr = kit::action_button("eco-place-route", "Place & Route (incremental)", busy_or(s, none.or((!s.eco.pending).then_some("No netlist changes to place"))), t, |s: &mut AppState| {
        let _ = implement(s, EcoImpl::PlaceAndRoute);
    });
    let ro = kit::action_button("eco-route-only", "Route Only", busy_or(s, none), t, |s: &mut AppState| {
        let _ = implement(s, EcoImpl::RouteOnly);
    });
    let wb = kit::action_button("eco-write-bit", "Write ECO Bitstream", busy_or(s, none.or(s.eco.pending.then_some("Place and route the netlist changes first"))), t, |s: &mut AppState| {
        let _ = write_bitstream(s);
    });
    row().key("eco-actions").w_full().h(28.0).items_center().gap(8.0).child(pr).child(ro).child(el().grow(1.0)).child(wb)
}

fn output(s: &AppState, t: &VivadoTheme) -> El {
    let mut c = col().w_full().gap(4.0);
    if let Some(Err(e)) | Some(Ok(e)) = s.eco.status.as_ref().map(|r| r.as_ref()) {
        let color = if matches!(s.eco.status, Some(Err(_))) { t.fail } else { t.text_dim };
        c = c.child(cad::cut(e.clone(), VivadoTheme::FONT_MONO.mono(), color).key("eco-status"));
    }
    match &s.eco.output {
        Some(Ok(o)) => {
            c = c
                .child(row().w_full().gap(8.0)
                    .child(cad::kpi("eco-kpi-changed", "Changed frames", o.changed_frames.to_string(), t.accent, format!("of {} configured", o.full_frames), t))
                    .child(cad::kpi("eco-kpi-changes", "Changes", s.eco.ops.iter().filter(|r| r.result.is_ok()).count().to_string(), t.text, "applied", t)))
                .child(cad::kv("Full bitstream", kit::text_cell(o.bit.display().to_string(), 1.0, t.link), 110.0, t))
                .child(cad::kv("Incremental", kit::text_cell(o.pbits.display().to_string(), 1.0, t.link).key("eco-pbits"), 110.0, t));
        }
        Some(Err(e)) => c = c.child(kit::empty_state("eco-output-error", e, None, t)),
        None => {}
    }
    c
}

pub fn eco_pane(s: &AppState, t: &VivadoTheme) -> El {
    let open_reason = open_unavailable(s);
    let open_btn = kit::action_button(
        "eco-open",
        if s.eco.session.is_some() { "Reload Implemented Design" } else { "Open Implemented Design" },
        open_reason,
        t,
        |s: &mut AppState| {
            let _ = open(s);
        },
    );
    let summary = match s.eco.session.as_ref().and_then(|x| x.design.as_ref()) {
        Some(d) => format!(
            "{} · {} cells · {} nets · {}",
            helion_ir::resolve(d.name),
            d.cells.len(),
            d.nets.len(),
            if s.eco.pending { "netlist changed — place & route pending" } else { "implementation in sync" }
        ),
        None if crate::bg::busy(s, TASK) => "Working…".into(),
        None => "Edit the routed netlist of the last implementation run.".into(),
    };
    let mut body = col().w_full().gap(10.0).padding(8.0).child(
        row()
            .w_full()
            .h(28.0)
            .items_center()
            .gap(8.0)
            .child(cad::cut(summary, VivadoTheme::FONT_UI, if s.eco.pending { t.warn } else { t.text_dim }).grow(1.0).min_w(0.0).key("eco-summary"))
            .child(open_btn),
    );
    if s.eco.session.is_some() || !s.eco.ops.is_empty() {
        body = body.child(form(s, t)).child(changes(s, t)).child(actions(s, t)).child(output(s, t));
    } else if let Some(Err(e)) = &s.eco.status {
        body = body.child(kit::empty_state("eco-empty", e, None, t));
    } else {
        body = body.child(kit::empty_state(
            "eco-empty",
            open_reason.map(|r| format!("{r} to start an ECO.")).as_deref().unwrap_or("Open the implemented design to start an ECO."),
            None,
            t,
        ));
    }
    col()
        .key("eco-pane")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("ECO", "eco", t))
        .child(col().key("eco-scroll").grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(body))
        .child(el().w_full().px(8.0).py(2.0).child(text("ECO - eco_1").font(VivadoTheme::FONT_UI).color(t.text_dim)))
}

/// Tcl verbs (`args` without the verb).
pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    let a = |i: usize, what: &str| args.get(i).cloned().ok_or_else(|| format!("{name}: missing {what}"));
    s.navigate(CenterPane::Eco);
    match name {
        "eco_open" => open(s),
        "eco_set_lut_init" => apply(s, EcoOp::SetLutInit { cell: a(0, "cell")?, init: parse_init(&a(1, "INIT")?)? }),
        "eco_connect_pin" => apply(s, EcoOp::ReconnectPin { cell: a(0, "cell")?, pin: a(1, "pin")?, net: a(2, "net")? }),
        "eco_insert_buffer" => apply(s, EcoOp::InsertBuffer { net: a(0, "net")?, name: a(1, "buffer name")? }),
        "eco_add_lut" => apply(s, EcoOp::AddLut { name: a(0, "name")?, init: parse_init(&a(1, "INIT")?)? }),
        "eco_add_ff" => apply(s, EcoOp::AddFf { name: a(0, "name")?, clk: a(1, "clock net")? }),
        "eco_remove_cell" => apply(s, EcoOp::RemoveCell { cell: a(0, "cell")? }),
        "eco_place_route" => implement(s, EcoImpl::PlaceAndRoute),
        "eco_route" => implement(s, EcoImpl::RouteOnly),
        "write_eco_bitstream" => write_bitstream(s),
        _ => Err(format!("{name}: not an ECO verb")),
    }
}

pub const VERBS: &[&str] = &[
    "eco_open",
    "eco_set_lut_init",
    "eco_connect_pin",
    "eco_insert_buffer",
    "eco_add_lut",
    "eco_add_ff",
    "eco_remove_cell",
    "eco_place_route",
    "eco_route",
    "write_eco_bitstream",
];

pub fn help(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "eco_open" => ("eco_open", "open the last implementation run for ECO editing"),
        "eco_set_lut_init" => ("eco_set_lut_init cell init", "change a LUT INIT (bitstream patched in place)"),
        "eco_connect_pin" => ("eco_connect_pin cell pin net", "move a cell pin onto another net"),
        "eco_insert_buffer" => ("eco_insert_buffer net name", "insert a LUT1 buffer between a net's driver and loads"),
        "eco_add_lut" => ("eco_add_lut name init", "add a LUT to the implemented netlist"),
        "eco_add_ff" => ("eco_add_ff name clk", "add a flip-flop on a clock net"),
        "eco_remove_cell" => ("eco_remove_cell cell", "delete a cell and its connections"),
        "eco_place_route" => ("eco_place_route", "incremental place + route of the ECO netlist"),
        "eco_route" => ("eco_route", "re-route the ECO netlist keeping placement"),
        "write_eco_bitstream" => ("write_eco_bitstream", "write the ECO bitstream and its incremental .pbits"),
        _ => return None,
    })
}

/// Scene fixture: an ECO session on a real in-process counter
/// implementation with an INIT change and a buffer insertion applied.
pub fn scene_fill(s: &mut AppState) {
    let Ok(dev) = helion_device::Device::load_part("HL10T-C32-1") else { return };
    let mut session = helion_proj::Session::new(helion_proj::Mode::Project);
    if session.impl_design(helion_ir::Design::structural_counter(), &dev).is_err() {
        return;
    }
    s.eco.base = session.bitstream.clone();
    s.eco.part = dev.part.clone();
    for op in [
        EcoOp::SetLutInit { cell: "u_lut0".into(), init: 0x1 },
        EcoOp::InsertBuffer { net: "q1".into(), name: "eco_buf".into() },
    ] {
        let r = session.apply_eco(&dev, &op);
        s.eco.ops.push(EcoRow { op, result: r });
    }
    s.eco.pending = true;
    s.eco.kind = EcoKind::Reconnect;
    s.eco.cell = TextEdit::new("u_lut2");
    s.eco.pin = TextEdit::new("I1");
    s.eco.net = TextEdit::new("q0");
    s.eco.status = s.eco.ops.last().map(|r| r.result.clone());
    s.eco.session = Some(Box::new(session));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eco_flow_opens_edits_reimplements_and_writes_incremental_bits() {
        let mut s = crate::views::reports::io::tests::implemented_state();
        s.runs.source = None;
        tcl_verb(&mut s, "eco_open", &[]).unwrap();
        crate::bg::wait_all(&mut s);
        assert!(s.eco.session.is_some(), "{:?}", s.eco.status);
        assert_eq!(s.center, CenterPane::Eco);
        let lut = s.eco.session.as_ref().unwrap().design.as_ref().unwrap().cells.iter()
            .find(|c| matches!(c.kind, helion_ir::CellKind::Lut6 { .. }))
            .map(|c| helion_ir::resolve(c.name).to_string())
            .unwrap();
        tcl_verb(&mut s, "eco_set_lut_init", &[lut.clone(), "64'h1".into()]).unwrap();
        crate::bg::wait_all(&mut s);
        assert!(s.eco.ops[0].result.is_ok(), "{:?}", s.eco.ops[0].result);
        assert!(!s.eco.pending, "INIT changes need no re-implementation");
        tcl_verb(&mut s, "eco_add_lut", &["eco_lut".into(), "2".into()]).unwrap();
        crate::bg::wait_all(&mut s);
        assert!(s.eco.pending);
        assert!(write_bitstream(&mut s).is_err(), "pending netlist changes block the bitstream");
        tcl_verb(&mut s, "eco_place_route", &[]).unwrap();
        crate::bg::wait_all(&mut s);
        assert!(!s.eco.pending, "{:?}", s.eco.status);
        tcl_verb(&mut s, "write_eco_bitstream", &[]).unwrap();
        crate::bg::wait_all(&mut s);
        let out = s.eco.output.as_ref().unwrap().as_ref().unwrap();
        assert!(out.changed_frames > 0 && out.changed_frames <= out.full_frames);
        assert!(out.pbits.starts_with(std::env::temp_dir()));
        assert!(std::fs::metadata(&out.pbits).unwrap().len() > 0);
    }

    #[test]
    fn form_validates_fields() {
        let mut s = AppState::demo();
        s.eco.kind = EcoKind::LutInit;
        assert!(form_op(&s).unwrap_err().contains("LUT cell"));
        s.eco.cell = TextEdit::new("u_lut0");
        s.eco.value = TextEdit::new("zz");
        assert!(form_op(&s).unwrap_err().contains("hex"));
        s.eco.value = TextEdit::new("64'hFFFF_0000");
        assert_eq!(form_op(&s).unwrap(), EcoOp::SetLutInit { cell: "u_lut0".into(), init: 0xFFFF_0000 });
        assert!(open_unavailable(&s).is_some());
    }
}
