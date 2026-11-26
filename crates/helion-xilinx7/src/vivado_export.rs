//! Hand Helion's own implementation to Vivado as an independent timing
//! oracle: a structural netlist of UNISIM primitives, an XDC that fixes every
//! cell on the site and BEL Helion placed it on, and Vivado batch scripts
//! that route (or only estimate) and report timing on that placement.
//!
//! The netlist is the packed design, cell for cell: LUTs (`LUT1`..`LUT6`,
//! pins locked to the physical `A1`..`A6` Helion uses; a LUT used with both
//! outputs becomes one cell on the `x6LUT` BEL and one on the `x5LUT` BEL),
//! flops (`FDRE`/`FDSE`/`FDCE`/`FDPE`), `CARRY4`, `MUXF7`/`MUXF8`, `SRL16E`/
//! `SRLC32E`, clock buffers, `IBUF`/`OBUF`. Block RAM, DSP and clock-manager
//! cells take their parameters from the packed cell and their pins from the
//! source netlist, since Helion's packed form does not hold their data pins.
//!
//! Routes are not exported: Vivado routes Helion's placement itself.

use crate::pack::{
    BramKind, CascadeOrder, ClockBufKind, CmtKind, IobDir, MuxfKind, PackedLut, RamMode,
    RamWriteMode, SrlKind,
};
use crate::place::Placed7;
use helion_ir::{resolve, Design, PortDir};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// The exported files' contents.
pub struct VivadoFiles {
    /// Structural Verilog (`<out>.v`).
    pub verilog: String,
    /// Constraints: the user's clocks and pins plus every cell's site/BEL.
    pub xdc: String,
    /// Vivado batch script: place (no-op for fixed cells), route, report.
    pub tcl: String,
    /// The same without routing: estimated timing on the placement alone.
    pub tcl_noroute: String,
}

fn is_const(net: &str) -> Option<bool> {
    match net {
        "0" | "gnd" | "GND" | "$false" => Some(false),
        "1" | "vcc" | "VCC" | "$true" => Some(true),
        _ => None,
    }
}

/// Verilog-safe, unique identifiers for nets and cells (one namespace, as in
/// a Verilog module). Names stay recognizable in Vivado's timing reports.
#[derive(Default)]
struct Names {
    used: HashSet<String>,
    map: HashMap<String, String>,
}

impl Names {
    /// `raw` is prefixed by its kind (`c:` cell, `n:` net): a cell and a net
    /// may share a source name.
    fn id(&mut self, key: &str) -> String {
        if let Some(v) = self.map.get(key) {
            return v.clone();
        }
        let raw = &key[2..];
        let mut base: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
        if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) || KEYWORDS.contains(&base.as_str()) {
            base = format!("h_{base}");
        }
        let mut name = base.clone();
        let mut k = 1;
        while !self.used.insert(name.clone()) {
            name = format!("{base}_{k}");
            k += 1;
        }
        self.map.insert(key.to_string(), name.clone());
        name
    }
}

const KEYWORDS: &[&str] = &[
    "module", "endmodule", "input", "output", "inout", "wire", "reg", "assign", "begin", "end", "always", "initial", "if",
    "else", "case", "for", "parameter", "localparam", "integer", "logic", "function", "task", "default", "or", "and", "not",
    "buf", "signed", "time", "event", "supply0", "supply1", "tri", "generate", "genvar",
];

/// Truth table of one LUT output over its connected pins only (constant pins
/// folded in, an unconnected pin read as 1 as silicon reads it; `a6` forces
/// pin 6). Returns (pins used, INIT over them).
fn lut_function(lut: &PackedLut, a6: Option<bool>) -> (Vec<usize>, u64) {
    let pins: Vec<usize> =
        (0..6).filter(|&i| a6.is_none() || i < 5).filter(|&i| lut.input_nets[i].as_deref().is_some_and(|n| is_const(n).is_none())).collect();
    let mut init = 0u64;
    for a in 0..(1u64 << pins.len()) {
        let mut addr = 0u64;
        for i in 0..6 {
            let bit = match (pins.iter().position(|&p| p == i), lut.input_nets[i].as_deref()) {
                (Some(k), _) => (a >> k) & 1 == 1,
                (None, _) if i == 5 && a6.is_some() => a6.unwrap(),
                (None, Some(n)) => is_const(n).unwrap_or(true),
                (None, None) => true,
            };
            addr |= (bit as u64) << i;
        }
        init |= ((lut.init >> addr) & 1) << a;
    }
    (pins, init)
}

struct Out {
    names: Names,
    /// Raw nets some exported cell drives.
    driven: HashSet<String>,
    /// Declared wires.
    wires: Vec<String>,
    body: String,
    place: String,
    unplaced: Vec<String>,
}

impl Out {
    fn cell(&mut self, raw: &str) -> String {
        self.names.id(&format!("c:{raw}"))
    }
    fn wire(&mut self, raw: &str) -> String {
        let key = format!("n:{raw}");
        let new = !self.names.map.contains_key(&key);
        let w = self.names.id(&key);
        if new {
            self.wires.push(w.clone());
        }
        w
    }
    /// Expression for a net read by a cell input.
    fn net(&mut self, raw: Option<&str>) -> String {
        match raw {
            None | Some("") => "1'b0".into(),
            Some(n) => match is_const(n) {
                Some(v) => format!("1'b{}", v as u8),
                None => {
                    let raw = if !self.driven.contains(n) && self.driven.contains(&format!("{n}_bufg")) { format!("{n}_bufg") } else { n.to_string() };
                    self.wire(&raw)
                }
            },
        }
    }
    /// Expression for a net driven by a cell output (a fresh dangling wire
    /// when the output is unused).
    fn onet(&mut self, raw: Option<&str>, cell: &str, pin: &str) -> String {
        match raw.filter(|n| !n.is_empty() && is_const(n).is_none()) {
            Some(n) => self.wire(n),
            None => self.wire(&format!("{cell}__nc_{pin}")),
        }
    }
    fn inst(&mut self, prim: &str, params: &[(String, String)], cell: &str, pins: &[(String, String)]) {
        let ps = if params.is_empty() {
            String::new()
        } else {
            format!(" #({})", params.iter().map(|(k, v)| format!(".{k}({v})")).collect::<Vec<_>>().join(", "))
        };
        let pins = pins.iter().map(|(k, v)| format!(".{k}({v})")).collect::<Vec<_>>().join(", ");
        let _ = writeln!(self.body, "  {prim}{ps} {cell} ({pins});");
    }
    fn fix(&mut self, cell: &str, site: Option<&str>, bel: Option<&str>) {
        let Some(site) = site.filter(|s| !s.is_empty()) else {
            self.unplaced.push(cell.to_string());
            return;
        };
        if let Some(bel) = bel {
            let _ = writeln!(self.place, "set_property BEL {bel} [get_cells {cell}]");
        }
        let _ = writeln!(self.place, "set_property LOC {site} [get_cells {cell}]");
        let fixed = if bel.is_some() { "IS_BEL_FIXED TRUE IS_LOC_FIXED TRUE" } else { "IS_LOC_FIXED TRUE" };
        let _ = writeln!(self.place, "set_property -dict {{{fixed}}} [get_cells {cell}]");
    }
}

fn p(k: &str, v: impl std::fmt::Display) -> (String, String) {
    (k.to_string(), v.to_string())
}
fn q(k: &str, v: &str) -> (String, String) {
    (k.to_string(), format!("\"{v}\""))
}
fn bus(bits: &[String]) -> String {
    format!("{{{}}}", bits.iter().rev().cloned().collect::<Vec<_>>().join(", "))
}

/// Export the placed design.
///
/// * `iob_ports`: IOB cell name -> the top-level port bit it serves, in XDC
///   form (`led[3]`, `clk`).
/// * `user_xdc`: the user's constraints; port, clock and pin lines are kept,
///   lines naming internal cells, nets or pins are dropped (names differ).
/// * `base`: the file name stem the scripts read (`<base>.v`, `<base>.xdc`).
pub fn export(
    design: &Design,
    placed: &Placed7,
    iob_ports: &HashMap<String, String>,
    user_xdc: &str,
    top: &str,
    part: &str,
    base: &str,
) -> Result<VivadoFiles, String> {
    let packed = &placed.packed;
    let mut o = Out {
        names: Names::default(),
        driven: HashSet::new(),
        wires: Vec::new(),
        body: String::new(),
        place: String::new(),
        unplaced: Vec::new(),
    };

    // Ports: names from the source netlist, widths from the bits Helion
    // pinned and the bits the user's XDC names.
    let mut width: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let bit_of = |key: &str| -> (String, Option<usize>) {
        match key.split_once('[') {
            Some((p, rest)) => (p.to_string(), rest.trim_end_matches(']').parse().ok()),
            None => (key.to_string(), None),
        }
    };
    for key in iob_ports.values() {
        let (p, b) = bit_of(key);
        let w = width.entry(p).or_insert(None);
        if let Some(b) = b {
            *w = Some(w.unwrap_or(0).max(b + 1));
        }
    }
    let mut ports = Vec::new();
    for port in &design.ports {
        let name = resolve(port.name).to_string();
        let mut w = width.get(&name).copied().flatten();
        for (i, _) in user_xdc.match_indices(&format!("{name}[")) {
            let ok_before = user_xdc[..i].chars().last().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
            let rest = &user_xdc[i + name.len() + 1..];
            if let (true, Some(b)) = (ok_before, rest.split(']').next().and_then(|d| d.parse::<usize>().ok())) {
                w = Some(w.unwrap_or(0).max(b + 1));
            }
        }
        o.names.used.insert(name.clone());
        let dir = match port.dir {
            PortDir::In => "input",
            PortDir::Out => "output",
            PortDir::Inout => "inout",
        };
        ports.push(match w {
            Some(w) => format!("  {dir} wire [{}:0] {name}", w - 1),
            None => format!("  {dir} wire {name}"),
        });
    }

    // Every net an exported cell drives, before any name is handed out.
    let lut5: HashMap<&str, String> = packed.dual_luts.iter().map(|n| (n.as_str(), format!("{n}__o5"))).collect();
    for l in &packed.luts {
        o.driven.insert(l.output_net.clone());
    }
    o.driven.extend(lut5.values().cloned());
    for f in &packed.ffs {
        o.driven.insert(f.q_net.clone());
    }
    for i in packed.iobs.iter().filter(|i| i.dir == IobDir::In) {
        o.driven.insert(i.net.clone());
    }
    for b in &packed.bufgs {
        o.driven.insert(b.out_net.clone());
    }
    for c in &packed.carries {
        o.driven.extend(c.o_nets.iter().chain(c.co_nets.iter()).chain(std::iter::once(&c.co_net)).flatten().cloned());
    }
    for m in &packed.muxfs {
        o.driven.insert(m.o_net.clone());
    }
    for s in &packed.srls {
        o.driven.insert(s.q_net.clone());
        o.driven.extend(s.q31_net.clone());
    }

    let lut_at: HashMap<&str, (&str, &str)> = placed.luts.iter().map(|l| (l.name.as_str(), (l.site.as_str(), l.bel.as_str()))).collect();

    // LUTs.
    for l in &packed.luts {
        let (site, bel) = lut_at.get(l.name.as_str()).map(|&(s, b)| (Some(s), Some(b))).unwrap_or((None, None));
        let halves: Vec<(String, Option<bool>, String, Option<String>)> = if let Some(o5) = lut5.get(l.name.as_str()) {
            vec![
                (l.name.clone(), Some(true), l.output_net.clone(), bel.map(str::to_string)),
                (format!("{}__lut5", l.name), Some(false), o5.clone(), bel.map(|b| b.replace("6LUT", "5LUT"))),
            ]
        } else {
            vec![(l.name.clone(), None, l.output_net.clone(), bel.map(str::to_string))]
        };
        for (cell, a6, out_net, bel) in halves {
            let (pins, init) = lut_function(l, a6);
            let y = o.wire(&out_net);
            if pins.is_empty() {
                // A constant: Vivado ties it off; there is no cell to place.
                let _ = writeln!(o.body, "  assign {y} = 1'b{};", init & 1);
                continue;
            }
            let c = o.cell(&cell);
            let k = pins.len();
            let mut conn = vec![p("O", &y)];
            for (j, &pi) in pins.iter().enumerate() {
                let n = o.net(l.input_nets[pi].as_deref());
                conn.push((format!("I{j}"), n));
            }
            o.inst(&format!("LUT{k}"), &[p("INIT", format!("{}'h{:x}", 1u32 << k, init))], &c, &conn);
            o.fix(&c, site, bel.as_deref());
            let lock: Vec<String> = pins.iter().enumerate().map(|(j, &pi)| format!("I{j}:A{}", pi + 1)).collect();
            let _ = writeln!(o.place, "set_property LOCK_PINS {{{}}} [get_cells {c}]", lock.join(" "));
        }
    }

    // Flops.
    let ff_at: HashMap<&str, (&str, &str)> = placed.ffs.iter().map(|f| (f.name.as_str(), (f.site.as_str(), f.bel.as_str()))).collect();
    for f in &packed.ffs {
        let (prim, rpin) = match (f.sr_net.is_some(), f.sr_async, f.srval) {
            (true, true, false) => ("FDCE", "CLR"),
            (true, true, true) => ("FDPE", "PRE"),
            (true, false, true) => ("FDSE", "S"),
            _ => ("FDRE", "R"),
        };
        let c = o.cell(&f.name);
        let mut params = vec![p("INIT", format!("1'b{}", f.init as u8))];
        if packed.negedge_ffs.contains(&f.name) {
            params.push(p("IS_C_INVERTED", "1'b1"));
        }
        let conn = vec![
            p("C", o.net(Some(&f.clk_net))),
            p("CE", f.ce_net.as_deref().map_or("1'b1".into(), |n| o.net(Some(n)))),
            p(rpin, o.net(f.sr_net.as_deref())),
            p("D", o.net(Some(&f.d_net))),
            p("Q", o.wire(&f.q_net)),
        ];
        o.inst(prim, &params, &c, &conn);
        let (site, bel) = ff_at.get(f.name.as_str()).map(|&(s, b)| (Some(s), Some(b))).unwrap_or((None, None));
        o.fix(&c, site, bel);
    }

    // Carry chains.
    let co_nets: HashSet<&str> = packed.carries.iter().filter_map(|c| c.co_net.as_deref()).collect();
    let s_lut: HashMap<&str, &str> = packed.luts.iter().map(|l| (l.output_net.as_str(), l.name.as_str())).collect();
    let carry_at: HashMap<&str, &str> = placed.carries.iter().map(|c| (c.name.as_str(), c.site.as_str())).collect();
    for c in &packed.carries {
        let adder = packed.adder_carries.contains(&c.name);
        let (ci, cyinit) = if co_nets.contains(c.ci_net.as_str()) {
            (o.net(Some(&c.ci_net)), "1'b0".to_string())
        } else if let Some(v) = c.cyinit {
            ("1'b0".to_string(), format!("1'b{}", v as u8))
        } else if let Some(n) = &c.cyinit_net {
            ("1'b0".to_string(), o.net(Some(n)))
        } else {
            ("1'b0".to_string(), o.net(Some(&c.ci_net)))
        };
        let mut di = Vec::new();
        for j in 0..4 {
            let o5 = c.s_nets[j]
                .as_deref()
                .filter(|_| adder && c.di_nets[j].is_some())
                .and_then(|s| s_lut.get(s))
                .and_then(|l| lut5.get(l))
                .cloned();
            di.push(o.net(o5.as_deref().or(c.di_nets[j].as_deref())));
        }
        let s: Vec<String> = (0..4).map(|j| o.net(c.s_nets[j].as_deref())).collect();
        let outs: Vec<String> = (0..4).map(|j| o.onet(c.o_nets[j].as_deref(), &c.name, &format!("O{j}"))).collect();
        let cos: Vec<String> = (0..4)
            .map(|j| {
                let n = if j == 3 { c.co_nets[3].as_deref().or(c.co_net.as_deref()) } else { c.co_nets[j].as_deref() };
                o.onet(n, &c.name, &format!("CO{j}"))
            })
            .collect();
        if let (Some(a), Some(b)) = (c.co_nets[3].as_deref(), c.co_net.as_deref())
            && a != b
        {
            return Err(format!("export: CARRY4 {} has CO[3] `{a}` and CO `{b}`", c.name));
        }
        let cell = o.cell(&c.name);
        let conn = vec![p("CI", ci), p("CYINIT", cyinit), p("DI", bus(&di)), p("S", bus(&s)), p("O", bus(&outs)), p("CO", bus(&cos))];
        o.inst("CARRY4", &[], &cell, &conn);
        o.fix(&cell, carry_at.get(c.name.as_str()).copied(), Some("CARRY4"));
    }

    // Wide multiplexers.
    for m in &packed.muxfs {
        let pm = placed.muxfs.iter().find(|x| x.name == m.name);
        let cell = o.cell(&m.name);
        let conn = vec![p("O", o.wire(&m.o_net)), p("I0", o.net(Some(&m.i0_net))), p("I1", o.net(Some(&m.i1_net))), p("S", o.net(Some(&m.s_net)))];
        o.inst(if m.kind == MuxfKind::Muxf7 { "MUXF7" } else { "MUXF8" }, &[], &cell, &conn);
        o.fix(&cell, pm.map(|x| x.site.as_str()), pm.map(|x| x.bel.as_str()));
    }

    // Shift registers.
    for s in &packed.srls {
        let ps = placed.srls.iter().find(|x| x.name == s.name);
        let cell = o.cell(&s.name);
        let mut conn = vec![p("Q", o.wire(&s.q_net)), p("CE", s.ce_net.as_deref().map_or("1'b1".into(), |n| o.net(Some(n)))), p("CLK", o.net(Some(&s.clk_net))), p("D", o.net(Some(&s.d_net)))];
        let a: Vec<String> = s.addr_nets.iter().map(|n| o.net(n.as_deref())).collect();
        let inv = format!("1'b{}", s.is_clk_inverted as u8);
        if s.kind == SrlKind::Srl16e {
            for (j, n) in a.iter().take(4).enumerate() {
                conn.push((format!("A{j}"), n.clone()));
            }
            o.inst("SRL16E", &[p("INIT", format!("16'h{:04x}", s.init & 0xffff)), p("IS_CLK_INVERTED", inv)], &cell, &conn);
        } else {
            conn.push(p("A", bus(&a)));
            conn.push(p("Q31", o.onet(s.q31_net.as_deref(), &s.name, "Q31")));
            o.inst("SRLC32E", &[p("INIT", format!("32'h{:08x}", s.init)), p("IS_CLK_INVERTED", inv)], &cell, &conn);
        }
        o.fix(&cell, ps.map(|x| x.site.as_str()), ps.map(|x| x.bel.as_str()));
    }

    // Clock buffers.
    for b in &packed.bufgs {
        let pb = placed.bufgs.iter().find(|x| x.name == b.name);
        let cell = o.cell(&b.name);
        let out = o.wire(&b.out_net);
        let i0 = o.net(Some(&b.in_net));
        let opt = |o: &mut Out, n: &Option<String>, d: &str| n.as_deref().map_or(d.to_string(), |n| o.net(Some(n)));
        match b.kind {
            ClockBufKind::Bufg => o.inst("BUFG", &[], &cell, &[p("I", i0), p("O", out)]),
            ClockBufKind::Bufgce => {
                let ce = opt(&mut o, &b.ce0_net, "1'b1");
                o.inst("BUFGCE", &[], &cell, &[p("I", i0), p("CE", ce), p("O", out)])
            }
            ClockBufKind::Bufgctrl => {
                let conn = vec![
                    p("I0", i0),
                    p("I1", opt(&mut o, &b.in1_net, "1'b0")),
                    p("CE0", opt(&mut o, &b.ce0_net, "1'b1")),
                    p("CE1", opt(&mut o, &b.ce1_net, "1'b0")),
                    p("S0", opt(&mut o, &b.s0_net, "1'b1")),
                    p("S1", opt(&mut o, &b.s1_net, "1'b0")),
                    p("IGNORE0", opt(&mut o, &b.ignore0_net, "1'b0")),
                    p("IGNORE1", opt(&mut o, &b.ignore1_net, "1'b0")),
                    p("O", out),
                ];
                let params = vec![
                    p("INIT_OUT", b.init_out),
                    q("PRESELECT_I0", if b.preselect_i0 { "TRUE" } else { "FALSE" }),
                    q("PRESELECT_I1", if b.preselect_i1 { "TRUE" } else { "FALSE" }),
                ];
                o.inst("BUFGCTRL", &params, &cell, &conn)
            }
            ClockBufKind::Bufio => o.inst("BUFIO", &[], &cell, &[p("I", i0), p("O", out)]),
            ClockBufKind::Bufr => {
                let conn = vec![p("I", i0), p("CE", opt(&mut o, &b.ce0_net, "1'b1")), p("CLR", opt(&mut o, &b.clr_net, "1'b0")), p("O", out)];
                let div = if b.bufr_divide.is_empty() { "BYPASS" } else { b.bufr_divide.as_str() };
                o.inst("BUFR", &[q("BUFR_DIVIDE", div), q("SIM_DEVICE", "7SERIES")], &cell, &conn)
            }
        }
        o.fix(&cell, pb.map(|x| x.site.as_str()), None);
    }

    // Pads: Vivado places the buffers from the ports' PACKAGE_PIN.
    let mut pins_xdc = String::new();
    for i in &packed.iobs {
        let key = iob_ports.get(&i.name).ok_or_else(|| format!("export: IOB {} serves no known port bit", i.name))?;
        let cell = o.cell(&i.name);
        if i.dir == IobDir::In {
            let n = o.wire(&i.net);
            o.inst("IBUF", &[], &cell, &[p("I", key.clone()), p("O", n)]);
        } else {
            let n = o.net(Some(&i.net));
            o.inst("OBUF", &[], &cell, &[p("I", n), p("O", key.clone())]);
        }
        let mut dict = format!("IOSTANDARD {}", i.iostandard);
        if let Some(loc) = &i.loc {
            dict = format!("PACKAGE_PIN {loc} {dict}");
        }
        if let Some(d) = &i.drive {
            dict.push_str(&format!(" DRIVE {d}"));
        }
        if let Some(s) = &i.slew {
            dict.push_str(&format!(" SLEW {s}"));
        }
        let _ = writeln!(pins_xdc, "set_property -dict {{{dict}}} [get_ports {{{key}}}]");
    }

    // Block RAM, DSP and clock managers: parameters from the packed cell,
    // pins from the source netlist.
    let cell_pins = ir_cell_pins(design);
    for b in &packed.brams {
        let pb = placed.brams.iter().find(|x| x.name == b.name);
        let (prim, w) = if b.kind == BramKind::Ramb36e1 { ("RAMB36E1", 36) } else { ("RAMB18E1", 18) };
        let mode = |m: RamWriteMode| match m {
            RamWriteMode::WriteFirst => "WRITE_FIRST",
            RamWriteMode::ReadFirst => "READ_FIRST",
            RamWriteMode::NoChange => "NO_CHANGE",
        };
        let mut params = vec![
            q("RAM_MODE", if b.ram_mode == RamMode::Sdp { "SDP" } else { "TDP" }),
            p("READ_WIDTH_A", b.read_width_a),
            p("READ_WIDTH_B", b.read_width_b),
            p("WRITE_WIDTH_A", b.write_width_a),
            p("WRITE_WIDTH_B", b.write_width_b),
            q("WRITE_MODE_A", mode(b.write_mode_a)),
            q("WRITE_MODE_B", mode(b.write_mode_b)),
            p("DOA_REG", b.doa_reg),
            p("DOB_REG", b.dob_reg),
            p("INIT_A", format!("{w}'h{:x}", b.init_a)),
            p("INIT_B", format!("{w}'h{:x}", b.init_b)),
            p("SRVAL_A", format!("{w}'h{:x}", b.srval_a)),
            p("SRVAL_B", format!("{w}'h{:x}", b.srval_b)),
        ];
        if b.kind == BramKind::Ramb36e1 {
            for (k, c) in [("RAM_EXTENSION_A", b.cascade_order_a), ("RAM_EXTENSION_B", b.cascade_order_b)] {
                let v = match c {
                    CascadeOrder::None => "NONE",
                    CascadeOrder::First => "LOWER",
                    CascadeOrder::Last => "UPPER",
                    CascadeOrder::Middle => return Err(format!("export: RAMB36E1 {} is a middle cascade stage", b.name)),
                };
                params.push(q(k, v));
            }
        }
        let mut rows = b.init_rows.clone();
        if rows.is_empty() {
            for (r, ch) in b.init.chunks(4).enumerate() {
                let mut row = [0u64; 4];
                row[..ch.len()].copy_from_slice(ch);
                rows.insert(r as u8, row);
            }
        }
        let hex = |r: &[u64; 4]| format!("256'h{:016x}{:016x}{:016x}{:016x}", r[3], r[2], r[1], r[0]);
        for (r, v) in &rows {
            params.push(p(&format!("INIT_{r:02X}"), hex(v)));
        }
        for (r, v) in &b.initp_rows {
            params.push(p(&format!("INITP_{r:02X}"), hex(v)));
        }
        ir_instance(&mut o, &cell_pins, prim, &b.name, params, &[])?;
        let id = o.cell(&b.name);
        o.fix(&id, pb.map(|x| x.site.as_str()), None);
    }
    for d in &packed.dsps {
        let pd = placed.dsps.iter().find(|x| x.name == d.name);
        let tf = |v: bool| if v { "TRUE" } else { "FALSE" };
        let params = vec![
            p("AREG", d.areg),
            p("ACASCREG", d.areg),
            p("BREG", d.breg),
            p("BCASCREG", d.breg),
            p("CREG", d.creg),
            p("DREG", d.dreg),
            p("ADREG", d.adreg),
            p("MREG", d.mreg),
            p("PREG", d.preg),
            p("OPMODEREG", d.opmodereg),
            p("ALUMODEREG", d.alumodereg),
            p("INMODEREG", d.inmodereg),
            p("CARRYINREG", d.carryinreg),
            p("CARRYINSELREG", d.carryinselreg),
            q("USE_MULT", if d.use_mult.is_empty() { "MULTIPLY" } else { &d.use_mult }),
            q("USE_DPORT", tf(d.use_dport)),
            q("USE_SIMD", if d.use_simd.is_empty() { "ONE48" } else { &d.use_simd }),
            q("USE_PATTERN_DETECT", if d.use_pattern_detect { "PATDET" } else { "NO_PATDET" }),
            p("PATTERN", format!("48'h{:x}", d.pattern)),
            p("MASK", format!("48'h{:x}", d.mask)),
            q("SEL_PATTERN", if d.sel_pattern.is_empty() { "PATTERN" } else { &d.sel_pattern }),
            q("SEL_MASK", if d.sel_mask.is_empty() { "MASK" } else { &d.sel_mask }),
            q("A_INPUT", if d.a_input_cascade { "CASCADE" } else { "DIRECT" }),
            q("B_INPUT", if d.b_input_cascade { "CASCADE" } else { "DIRECT" }),
        ];
        // Modes Helion holds as cell configuration are constant pins.
        let consts = [("OPMODE", 7, d.opmode as u64), ("ALUMODE", 4, d.alumode as u64), ("INMODE", 5, d.inmode as u64)];
        ir_instance(&mut o, &cell_pins, "DSP48E1", &d.name, params, &consts)?;
        let id = o.cell(&d.name);
        o.fix(&id, pd.map(|x| x.site.as_str()), None);
    }
    for c in &packed.cmts {
        let pc = placed.cmts.iter().find(|x| x.name == c.name);
        let mmcm = c.kind == CmtKind::MmcmE2Adv;
        let mut params = vec![
            q("BANDWIDTH", if c.bandwidth.is_empty() { "OPTIMIZED" } else { &c.bandwidth }),
            q("COMPENSATION", if c.compensation.is_empty() { "ZHOLD" } else { &c.compensation }),
            p("CLKIN1_PERIOD", format!("{:.3}", c.clkin1_period_ns)),
            p("DIVCLK_DIVIDE", c.divclk_divide),
            p("CLKFBOUT_PHASE", format!("{:.3}", c.clkfbout_phase)),
        ];
        if c.clkin2_period_ns > 0.0 {
            params.push(p("CLKIN2_PERIOD", format!("{:.3}", c.clkin2_period_ns)));
        }
        let outs = if mmcm { 7 } else { 6 };
        if mmcm {
            params.push(p("CLKFBOUT_MULT_F", format!("{:.3}", c.clkfbout_mult_f)));
            params.push(p("CLKOUT0_DIVIDE_F", format!("{:.3}", if c.clkout0_divide_f > 0.0 { c.clkout0_divide_f } else { c.clkout_divide[0] as f64 })));
        } else {
            params.push(p("CLKFBOUT_MULT", c.clkfbout_mult_f.round() as u32));
            params.push(p("CLKOUT0_DIVIDE", c.clkout_divide[0]));
        }
        for i in 0..outs {
            if i > 0 {
                params.push(p(&format!("CLKOUT{i}_DIVIDE"), c.clkout_divide[i].max(1)));
            }
            params.push(p(&format!("CLKOUT{i}_PHASE"), format!("{:.3}", c.clkout_phase[i])));
            params.push(p(&format!("CLKOUT{i}_DUTY_CYCLE"), format!("{:.3}", if c.clkout_duty[i] > 0.0 { c.clkout_duty[i] } else { 0.5 })));
        }
        ir_instance(&mut o, &cell_pins, if mmcm { "MMCME2_ADV" } else { "PLLE2_ADV" }, &c.name, params, &[])?;
        let id = o.cell(&c.name);
        o.fix(&id, pc.map(|x| x.site.as_str()), None);
    }
    let unsupported = [
        ("IDDR/ODDR", packed.ioddrs.len()),
        ("ISERDESE2/OSERDESE2", packed.serdes.len()),
        ("IDELAYE2/ODELAYE2/IDELAYCTRL", packed.iodelays.len()),
        ("GTPE2", packed.gtp_channels.len() + packed.gtp_commons.len() + packed.ibufds_gte2s.len()),
        ("PCIE_2_1", packed.pcies.len()),
        ("XADC", packed.xadcs.len()),
        ("configuration primitives", packed.cfg_prims.len()),
    ];
    if let Some((k, n)) = unsupported.iter().find(|(_, n)| *n > 0) {
        return Err(format!("export: {n} {k} cell(s): not supported by the Vivado export yet"));
    }
    // LUT RAM: the source primitive as written, its INIT and inversion
    // parameters copied, LOC'd to Helion's SLICEM (Vivado picks the BELs inside it).
    let attrs_of: HashMap<String, &helion_ir::Attrs> = design.cells.iter().map(|c| (resolve(c.name).to_string(), &c.attrs)).collect();
    for r in &packed.lutrams {
        let params: Vec<(String, String)> = attrs_of
            .get(&r.name)
            .map(|a| a.map.iter().filter(|(k, _)| k.starts_with("INIT") || (k.starts_with("IS_") && k.ends_with("_INVERTED"))))
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), if v.contains('\'') { v.clone() } else { format!("{}'h{}", if r.module.starts_with("RAM32X") { 32 } else { 64 }, v.trim_start_matches("0x")) }))
            .collect();
        ir_instance(&mut o, &cell_pins, &r.module, &r.name, params, &[])?;
        let id = o.cell(&r.name);
        o.fix(&id, placed.lutrams.iter().find(|x| x.name == r.name).map(|x| x.site.as_str()), None);
    }


    let mut v = String::new();
    let _ = writeln!(v, "// Helion's packed and placed netlist of `{top}` for {part}, as UNISIM primitives.");
    let _ = writeln!(v, "// Placement (LOC/BEL) is in the matching .xdc. Generated; do not edit.");
    let _ = writeln!(v, "module {top} (\n{}\n);", ports.join(",\n"));
    for w in &o.wires {
        let _ = writeln!(v, "  wire {w};");
    }
    v.push_str(&o.body);
    v.push_str("endmodule\n");

    // Constraints.
    let mut x = String::new();
    let _ = writeln!(x, "# Constraints for Helion's placement of `{top}` ({part}). Generated; do not edit.");
    let _ = writeln!(x, "\n# From the design's XDC (lines naming internal cells, nets or pins are dropped).");
    for line in user_xdc.lines() {
        let internal = ["get_cells", "get_nets", "get_pins"].iter().any(|k| line.contains(k)) && !line.trim_start().starts_with('#');
        if internal {
            let _ = writeln!(x, "# dropped: {line}");
        } else {
            let _ = writeln!(x, "{line}");
        }
    }
    let _ = writeln!(x, "\n# Clocks Helion made in the fabric: a toggle flop halves its clock (zedbit refuses any other).");
    for b in &packed.bufgs {
        if let Some(f) = packed.ffs.iter().find(|f| f.q_net == b.in_net) {
            let c = o.cell(&f.name);
            let _ = writeln!(x, "create_generated_clock -name {} -source [get_pins {c}/C] -divide_by 2 [get_pins {c}/Q]", o.wire(&b.out_net));
        }
    }
    let _ = writeln!(x, "\n# Pads (Helion's pins and I/O standards).\n{pins_xdc}");
    let _ = writeln!(x, "# Every cell where Helion placed it.\n{}", o.place);
    if !o.unplaced.is_empty() {
        let _ = writeln!(x, "# {} cells Helion did not place: {}", o.unplaced.len(), o.unplaced.join(" "));
    }

    let script = |route: bool| -> String {
        let tag = if route { "routed" } else { "placed" };
        let mut t = String::new();
        let _ = writeln!(t, "# Vivado as a timing oracle for Helion's placement of `{top}`.");
        let _ = writeln!(t, "#   vivado -mode batch -source {base}{}.tcl", if route { "" } else { "_noroute" });
        let _ = writeln!(t, "# Reports are written next to this script, named {base}_{tag}_*.rpt.");
        let _ = writeln!(t, "set d [file normalize [file dirname [info script]]]");
        let _ = writeln!(t, "read_verilog $d/{base}.v\nread_xdc $d/{base}.xdc");
        let _ = writeln!(t, "link_design -part {part} -top {top}");
        t.push_str(
            "set leaf [get_cells -hierarchical -filter {IS_PRIMITIVE && PRIMITIVE_LEVEL != INTERNAL && REF_NAME !~ *BUF && REF_NAME != VCC && REF_NAME != GND}]\n\
             set before [dict create]\n\
             set noloc 0\n\
             foreach c $leaf { dict set before $c [list [get_property LOC $c] [get_property BEL $c]]; if {[get_property LOC $c] eq \"\"} { incr noloc } }\n\
             puts \"helion: [llength $leaf] cells, $noloc without a LOC after link_design\"\n\
             # Places only what Helion did not fix (nothing, normally).\n\
             place_design\n\
             set moved 0\n\
             foreach c $leaf {\n\
             \x20 set was [dict get $before $c]\n\
             \x20 if {[lindex $was 0] ne \"\" && $was ne [list [get_property LOC $c] [get_property BEL $c]]} { incr moved; if {$moved <= 20} { puts \"helion: moved $c: $was -> [get_property LOC $c] [get_property BEL $c]\" } }\n\
             }\n\
             puts \"helion: $moved cells moved by place_design (must be 0)\"\n",
        );
        if route {
            t.push_str("route_design\n");
            let _ = writeln!(t, "report_route_status -file $d/{base}_{tag}_route_status.rpt");
        }
        let _ = writeln!(t, "report_timing_summary -max_paths 10 -file $d/{base}_{tag}_timing_summary.rpt");
        let _ = writeln!(t, "report_timing -max_paths 50 -nworst 1 -sort_by slack -file $d/{base}_{tag}_timing.rpt");
        let _ = writeln!(t, "report_utilization -file $d/{base}_{tag}_utilization.rpt");
        let _ = writeln!(t, "report_clocks -file $d/{base}_{tag}_clocks.rpt");
        let _ = writeln!(t, "puts \"helion: WNS [get_property SLACK [get_timing_paths -max_paths 1 -setup]] ns, WHS [get_property SLACK [get_timing_paths -max_paths 1 -hold]] ns ({})\"", if route { "routed by Vivado" } else { "estimated, unrouted" });
        t
    };
    Ok(VivadoFiles { verilog: v, xdc: x, tcl: script(true), tcl_noroute: script(false) })
}

/// Source-netlist pins of each cell: cell -> [(pin, net)].
fn ir_cell_pins(design: &Design) -> HashMap<String, Vec<(String, String)>> {
    let mut m: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for n in &design.nets {
        for e in &n.endpoints {
            m.entry(resolve(e.cell).to_string()).or_default().push((resolve(e.pin).to_string(), resolve(n.name).to_string()));
        }
    }
    m
}

/// Bus pins (name, width) of the primitives written from source-netlist
/// pins; the source may name their bits `NAME[i]` or `NAMEi`.
fn buses_of(prim: &str) -> &'static [(&'static str, usize)] {
    match prim {
        "RAMB18E1" => &[("ADDRARDADDR", 14), ("ADDRBWRADDR", 14), ("DIADI", 16), ("DIBDI", 16), ("DIPADIP", 2), ("DIPBDIP", 2), ("DOADO", 16), ("DOBDO", 16), ("DOPADOP", 2), ("DOPBDOP", 2), ("WEA", 2), ("WEBWE", 4)],
        "RAMB36E1" => &[
            ("ADDRARDADDR", 16), ("ADDRBWRADDR", 16), ("DIADI", 32), ("DIBDI", 32), ("DIPADIP", 4), ("DIPBDIP", 4), ("DOADO", 32), ("DOBDO", 32),
            ("DOPADOP", 4), ("DOPBDOP", 4), ("WEA", 4), ("WEBWE", 8), ("ECCPARITY", 8), ("RDADDRECC", 9),
        ],
        "DSP48E1" => &[
            ("A", 30), ("B", 18), ("C", 48), ("D", 25), ("P", 48), ("PCIN", 48), ("PCOUT", 48), ("ACIN", 30), ("ACOUT", 30), ("BCIN", 18),
            ("BCOUT", 18), ("OPMODE", 7), ("ALUMODE", 4), ("INMODE", 5), ("CARRYINSEL", 3), ("CARRYOUT", 4),
        ],
        "RAM32M" => &[("ADDRA", 5), ("ADDRB", 5), ("ADDRC", 5), ("ADDRD", 5), ("DIA", 2), ("DIB", 2), ("DIC", 2), ("DID", 2), ("DOA", 2), ("DOB", 2), ("DOC", 2), ("DOD", 2)],
        "RAM64M" => &[("ADDRA", 6), ("ADDRB", 6), ("ADDRC", 6), ("ADDRD", 6)],
        "RAM128X1D" => &[("A", 7), ("DPRA", 7)],
        "RAM256X1S" => &[("A", 8)],
        // The other LUT RAMs have scalar A0..An / DPRA0..n pins.
        m if crate::lutram::is_lutram(m) => &[],
        _ => &[("DADDR", 7), ("DI", 16), ("DO", 16)],
    }
}

/// Output pins of the primitives written from source-netlist pins.
fn is_output_pin(prim: &str, pin: &str) -> bool {
    let outs: &[&str] = match prim {
        "RAMB18E1" | "RAMB36E1" => &["DOADO", "DOBDO", "DOPADOP", "DOPBDOP", "CASCADEOUTA", "CASCADEOUTB", "DBITERR", "SBITERR", "ECCPARITY", "RDADDRECC"],
        m if crate::lutram::is_lutram(m) => &["DOA", "DOB", "DOC", "DOD", "O", "SPO", "DPO"],
        "DSP48E1" => &["P", "PCOUT", "ACOUT", "BCOUT", "CARRYOUT", "CARRYCASCOUT", "MULTSIGNOUT", "PATTERNDETECT", "PATTERNBDETECT", "OVERFLOW", "UNDERFLOW"],
        _ => &["CLKOUT0", "CLKOUT0B", "CLKOUT1", "CLKOUT1B", "CLKOUT2", "CLKOUT2B", "CLKOUT3", "CLKOUT3B", "CLKOUT4", "CLKOUT5", "CLKOUT6", "CLKFBOUT", "CLKFBOUTB", "LOCKED", "DO", "DRDY", "PSDONE", "CLKINSTOPPED", "CLKFBSTOPPED"],
    };
    outs.contains(&pin)
}

/// One instance whose pins come from the source netlist: `NAME[i]` pins
/// gather into buses (missing input bits 0), other pins connect by name.
/// `consts` ties whole buses the source leaves open to a constant.
fn ir_instance(
    o: &mut Out,
    cell_pins: &HashMap<String, Vec<(String, String)>>,
    prim: &str,
    name: &str,
    params: Vec<(String, String)>,
    consts: &[(&str, usize, u64)],
) -> Result<(), String> {
    let pins = cell_pins.get(name).cloned().unwrap_or_default();
    let mut buses: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
    let mut scalars: BTreeMap<String, String> = BTreeMap::new();
    let known = buses_of(prim);
    for (pin, net) in &pins {
        let bit = pin.split_once('[').map(|(b, i)| (b, i.trim_end_matches(']'))).or_else(|| {
            known.iter().find_map(|(b, _)| pin.strip_prefix(b).filter(|i| !i.is_empty() && i.bytes().all(|c| c.is_ascii_digit())).map(|i| (*b, i)))
        });
        match bit {
            Some((b, i)) => {
                let i: usize = i.parse().map_err(|_| format!("export: {name}: bad pin `{pin}`"))?;
                buses.entry(b.to_string()).or_default().insert(i, net.clone());
            }
            None => {
                scalars.insert(pin.clone(), net.clone());
            }
        }
    }
    let mut conn = Vec::new();
    for (pin, net) in &scalars {
        let e = if is_output_pin(prim, pin) { o.onet(Some(net), name, pin) } else { o.net(Some(net)) };
        conn.push((pin.clone(), e));
    }
    for (b, bits) in &buses {
        let w = known.iter().find(|(k, _)| k == b).map_or(0, |k| k.1).max(bits.keys().max().map_or(0, |m| m + 1));
        let out = is_output_pin(prim, b);
        let v: Vec<String> = (0..w)
            .map(|i| if out { o.onet(bits.get(&i).map(|s| s.as_str()), name, &format!("{b}{i}")) } else { o.net(bits.get(&i).map(|s| s.as_str())) })
            .collect();
        conn.push((b.clone(), bus(&v)));
    }
    for (b, w, v) in consts {
        if !buses.contains_key(*b) && !scalars.contains_key(*b) {
            conn.push((b.to_string(), format!("{w}'d{v}")));
        }
    }
    let cell = o.cell(name);
    o.inst(prim, &params, &cell, &conn);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{Packed7, PackedCarry, PackedFf, PackedIob};
    use crate::place::{PlacedCarry, PlacedFf, PlacedLut};

    fn lut(init: u64, ins: [Option<&str>; 6], out: &str) -> PackedLut {
        PackedLut { name: format!("{out}_lut"), init, input_nets: ins.map(|n| n.map(str::to_string)), output_net: out.into() }
    }

    /// LUT RAM ports go out as the UNISIM buses and outputs (G122).
    #[test]
    fn lut_ram_pins_are_unisim_buses() {
        assert!(buses_of("RAM32M").contains(&("DOA", 2)) && buses_of("RAM64M").contains(&("ADDRD", 6)));
        assert!(buses_of("RAM64X1D").is_empty(), "A0..A5 are scalar ports");
        assert!(is_output_pin("RAM64X1D", "DPO") && is_output_pin("RAM32M", "DOD") && !is_output_pin("RAM32M", "DID"));
    }

    /// The compacted LUT equals the physical one: connected pins as given,
    /// constant pins at their value, unconnected pins high.
    #[test]
    fn lut_function_matches_the_physical_lut() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let nets = [None, Some("1"), Some("0"), Some("x")];
        for _ in 0..2000 {
            let init = rnd();
            let ins: [Option<&str>; 6] = std::array::from_fn(|_| nets[(rnd() % 4) as usize]);
            let l = lut(init, ins, "y");
            let (pins, f) = lut_function(&l, None);
            for a in 0..64u64 {
                let phys = (0..6).fold(0u64, |acc, i| {
                    let bit = match ins[i] {
                        None | Some("1") => 1,
                        Some("0") => 0,
                        _ => (a >> i) & 1,
                    };
                    acc | bit << i
                });
                let k = pins.iter().enumerate().fold(0u64, |acc, (j, &p)| acc | ((a >> p) & 1) << j);
                assert_eq!((init >> phys) & 1, (f >> k) & 1, "init {init:x} ins {ins:?} addr {a}");
            }
        }
    }

    #[test]
    fn export_writes_fixed_cells() {
        let mut d = Design::new("t");
        d.add_port("clk", PortDir::In);
        d.add_port("a", PortDir::In);
        d.add_port("y", PortDir::Out);
        let mut packed = Packed7::default();
        // A dual LUT: O6 = a ^ q (propagate), O5 = a (generate).
        packed.luts.push(lut(0x6666_6666_AAAA_AAAA, [Some("a"), Some("q"), None, None, None, None], "s0"));
        packed.dual_luts.insert("s0_lut".into());
        packed.carries.push(PackedCarry {
            name: "c0".into(),
            cyinit: Some(false),
            s_nets: [Some("s0".into()), None, None, None],
            di_nets: [Some("a".into()), None, None, None],
            o_nets: [Some("sum0".into()), None, None, None],
            ..Default::default()
        });
        packed.adder_carries.insert("c0".into());
        packed.ffs.push(PackedFf {
            name: "q_reg".into(),
            d_net: "sum0".into(),
            q_net: "q".into(),
            clk_net: "clk_bufg".into(),
            ce_net: None,
            sr_net: Some("a".into()),
            init: false,
            srval: false,
            sr_async: true,
        });
        packed.negedge_ffs.insert("q_reg".into());
        let iob = |name: &str, dir, net: &str| PackedIob {
            name: name.into(),
            port: String::new(),
            dir,
            net: net.into(),
            loc: Some("Y9".into()),
            iostandard: "LVCMOS33".into(),
            drive: None,
            slew: None,
        };
        packed.iobs = vec![iob("ib_a", IobDir::In, "a"), iob("ob_y", IobDir::Out, "q"), iob("ibuf_clk", IobDir::In, "clk_ibuf")];
        // A block RAM whose source pins name bus bits without brackets.
        packed.brams.push(crate::pack::PackedBram { name: "ram".into(), read_width_a: 18, ..Default::default() });
        d.add_cell("ram", helion_ir::CellKind::Bram18e1 { read_width_a: 18, read_width_b: 18, write_width_a: 18, write_width_b: 18 });
        d.connect("q", "ram", "ADDRARDADDR13");
        d.connect("rd0", "ram", "DOADO0");
        d.connect("clk_bufg", "ram", "CLKARDCLK");
        packed.bufgs.push(crate::pack::PackedBufg { name: "bufg_clk".into(), in_net: "clk_ibuf".into(), out_net: "clk_bufg".into(), ..Default::default() });
        let placed = Placed7 {
            luts: vec![PlacedLut { name: "s0_lut".into(), init: 0, tile: "T".into(), site: "SLICE_X1Y1".into(), bel: "B6LUT".into() }],
            ffs: vec![PlacedFf { name: "q_reg".into(), tile: "T".into(), site: "SLICE_X1Y1".into(), bel: "BFF".into() }],
            carries: vec![PlacedCarry { name: "c0".into(), tile: "T".into(), site: "SLICE_X1Y1".into() }],
            packed,
            ..Default::default()
        };
        let ports: HashMap<String, String> = [("ib_a", "a"), ("ob_y", "y"), ("ibuf_clk", "clk")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let f = export(&d, &placed, &ports, "create_clock -period 10 [get_ports clk]\nset_false_path -to [get_cells x]\n", "t", "xc7z020clg484-1", "t").unwrap();
        let v = &f.verilog;
        assert!(v.contains("LUT2 #(.INIT(4'h6)) s0_lut (.O(s0), .I0(a_1), .I1(q))"), "{v}");
        assert!(v.contains("LUT2 #(.INIT(4'ha)) s0_lut__lut5 (.O(s0_lut__o5), .I0(a_1), .I1(q))"), "{v}");
        assert!(v.contains(".DI({1'b0, 1'b0, 1'b0, s0_lut__o5})"), "{v}");
        assert!(v.contains("FDCE #(.INIT(1'b0), .IS_C_INVERTED(1'b1)) q_reg (.C(clk_bufg), .CE(1'b1), .CLR(a_1)"), "{v}");
        assert!(v.contains("IBUF ib_a (.I(a), .O(a_1))") && v.contains("OBUF ob_y (.I(q), .O(y))"), "{v}");
        assert!(v.contains(".ADDRARDADDR({q, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0, 1'b0})"), "{v}");
        assert!(v.contains(".DOADO({ram__nc_DOADO15,") && v.contains("ram__nc_DOADO1, rd0})") && v.contains(".CLKARDCLK(clk_bufg)"), "{v}");
        let x = &f.xdc;
        for want in [
            "set_property BEL B5LUT [get_cells s0_lut__lut5]",
            "set_property LOCK_PINS {I0:A1 I1:A2} [get_cells s0_lut]",
            "set_property BEL BFF [get_cells q_reg]\nset_property LOC SLICE_X1Y1 [get_cells q_reg]",
            "set_property BEL CARRY4 [get_cells c0]",
            "# dropped: set_false_path -to [get_cells x]",
            "create_clock -period 10 [get_ports clk]",
        ] {
            assert!(x.contains(want), "missing `{want}` in\n{x}");
        }
        assert!(f.tcl.contains("route_design") && !f.tcl_noroute.contains("route_design"));
    }
}
