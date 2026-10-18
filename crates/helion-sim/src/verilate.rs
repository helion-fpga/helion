//! Verilator-class cycle-accurate C++20 & Rust model code generator
//! ([`emit_cpp_cycle_model`], [`emit_rust_cycle_model`]) and IEEE 1364/1800 VPI
//! signal introspection API ([`VpiRegistry`]).
//!
//! - Topologically levelizes `Design` combinational primitives (`Lut6`, `Carry4`,
//!   `Muxf7`, `Muxf8`, `IobIn`, `IobOut`, `ClockBuf`) and sequential elements
//!   (`Hff`, `Bram18` / `Bram18e1`), emitting standalone, zero-dependency C++20
//!   (`struct VHelionModel { void eval_comb(); void tick(); };`) and Rust cycle
//!   models using branchless 64-bit LUT truth-table shifts (`(0x...ULL >> idx) & 1ULL`).
//! - Provides IEEE 1800 VPI introspection (`vpi_handle_by_name`, `vpi_get_value`,
//!   `vpi_put_value`, `vpi_iterate_ports`, `vpi_iterate_nets`) over [`Sim`](crate::Sim).

use crate::{Sim, SimForceKind};
use helion_ir::{resolve, Cell, CellKind, Design, PortDir};
use std::collections::{BTreeMap, HashMap, VecDeque};

// ============================================================================
// Part 1: Topological Levelization & C++20 / Rust Cycle-Accurate Codegen
// ============================================================================

#[derive(Clone, Debug)]
enum VerilateCombOp {
    Lut6 {
        cell_name: String,
        init: u64,
        inputs: [Option<String>; 6],
        out_net: String,
    },
    Carry4 {
        cell_name: String,
        ci_net: Option<String>,
        cyinit_net: Option<String>,
        di_nets: [Option<String>; 4],
        s_nets: [Option<String>; 4],
        o_nets: [Option<String>; 4],
        co_nets: [Option<String>; 4],
    },
    Mux2 {
        cell_name: String,
        is_f8: bool,
        i0_net: Option<String>,
        i1_net: Option<String>,
        s_net: Option<String>,
        out_net: String,
    },
    Assign {
        dst_net: String,
        src_net: String,
    },
}

impl VerilateCombOp {
    fn input_nets(&self) -> Vec<&str> {
        let mut out = Vec::new();
        match self {
            Self::Lut6 { inputs, .. } => {
                for inp in inputs.iter().flatten() {
                    out.push(inp.as_str());
                }
            }
            Self::Carry4 {
                ci_net,
                cyinit_net,
                di_nets,
                s_nets,
                ..
            } => {
                for inp in ci_net
                    .iter()
                    .chain(cyinit_net.iter())
                    .chain(di_nets.iter().flatten())
                    .chain(s_nets.iter().flatten())
                {
                    out.push(inp.as_str());
                }
            }
            Self::Mux2 {
                i0_net,
                i1_net,
                s_net,
                ..
            } => {
                for inp in i0_net.iter().chain(i1_net.iter()).chain(s_net.iter()) {
                    out.push(inp.as_str());
                }
            }
            Self::Assign { src_net, .. } => out.push(src_net.as_str()),
        }
        out
    }

    fn output_nets(&self) -> Vec<&str> {
        match self {
            Self::Lut6 { out_net, .. } | Self::Mux2 { out_net, .. } => vec![out_net.as_str()],
            Self::Carry4 {
                o_nets, co_nets, ..
            } => o_nets
                .iter()
                .flatten()
                .chain(co_nets.iter().flatten())
                .map(String::as_str)
                .collect(),
            Self::Assign { dst_net, .. } => vec![dst_net.as_str()],
        }
    }
}

#[derive(Clone, Debug)]
struct VerilateFf {
    cell_name: String,
    d_net: Option<String>,
    q_net: Option<String>,
    ce_net: Option<String>,
    rst_net: Option<String>,
}

#[derive(Clone, Debug)]
struct VerilateRam18 {
    cell_name: String,
    raddr: [Option<String>; 10],
    waddr: [Option<String>; 10],
    di: [Option<String>; 16],
    do_nets: [Option<String>; 16],
    we_net: Option<String>,
}

#[derive(Clone, Debug)]
struct VerilateSchedule {
    module_name: String,
    nets: Vec<String>,
    ops: Vec<VerilateCombOp>,
    ffs: Vec<VerilateFf>,
    rams: Vec<VerilateRam18>,
    led_source: Option<String>,
}

/// Sanitize an arbitrary HDL net or cell name into a valid C++20 / Rust identifier.
pub fn sanitize_ident(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 4);
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.chars().next().unwrap().is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

fn classify_muxf(cell: &Cell) -> Option<bool> {
    if let CellKind::BlackBox { module } = cell.kind {
        let m = resolve(module);
        if m.eq_ignore_ascii_case("MUXF7") {
            return Some(false);
        }
        if m.eq_ignore_ascii_case("MUXF8") {
            return Some(true);
        }
    }
    for k in ["primitive", "type", "bel_type"] {
        if let Some(v) = cell.attrs.get(k) {
            if v.eq_ignore_ascii_case("MUXF7") {
                return Some(false);
            }
            if v.eq_ignore_ascii_case("MUXF8") {
                return Some(true);
            }
        }
    }
    None
}

fn build_verilate_schedule(design: &Design) -> VerilateSchedule {
    let pin_ix = design.pin_index();
    let mut net_set: BTreeMap<String, ()> = BTreeMap::new();
    for p in &design.ports {
        let pname = resolve(p.name).to_string();
        net_set.insert(pname, ());
    }
    for n in &design.nets {
        let nname = resolve(n.name);
        if nname != "0" && nname != "1" && nname != "1'b0" && nname != "1'b1" {
            net_set.insert(nname.to_string(), ());
        }
    }

    let mut raw_ops: Vec<VerilateCombOp> = Vec::new();
    let mut ffs: Vec<VerilateFf> = Vec::new();
    let mut rams: Vec<VerilateRam18> = Vec::new();
    let mut led_source: Option<String> = None;

    for cell in &design.cells {
        let cname = resolve(cell.name);
        match cell.kind {
            CellKind::Lut6 { init } => {
                let Some(out_s) = pin_ix.net_on(cname, "O") else {
                    continue;
                };
                let mut inputs: [Option<String>; 6] = Default::default();
                for (i, slot) in inputs.iter_mut().enumerate() {
                    *slot = pin_ix
                        .net_on(cname, &format!("I{i}"))
                        .map(|s| resolve(s).to_string());
                }
                raw_ops.push(VerilateCombOp::Lut6 {
                    cell_name: cname.to_string(),
                    init,
                    inputs,
                    out_net: resolve(out_s).to_string(),
                });
            }
            CellKind::Carry4 => {
                let ci_net = pin_ix.net_on(cname, "CI").map(|s| resolve(s).to_string());
                let cyinit_net = pin_ix
                    .net_on(cname, "CYINIT")
                    .map(|s| resolve(s).to_string());
                let mut di_nets: [Option<String>; 4] = Default::default();
                let mut s_nets: [Option<String>; 4] = Default::default();
                let mut o_nets: [Option<String>; 4] = Default::default();
                let mut co_nets: [Option<String>; 4] = Default::default();
                for i in 0..4 {
                    di_nets[i] = pin_ix
                        .net_on(cname, &format!("DI{i}"))
                        .map(|s| resolve(s).to_string());
                    s_nets[i] = pin_ix
                        .net_on(cname, &format!("S{i}"))
                        .map(|s| resolve(s).to_string());
                    o_nets[i] = pin_ix
                        .net_on(cname, &format!("O{i}"))
                        .map(|s| resolve(s).to_string());
                    co_nets[i] = pin_ix
                        .net_on(cname, &format!("CO{i}"))
                        .map(|s| resolve(s).to_string());
                }
                raw_ops.push(VerilateCombOp::Carry4 {
                    cell_name: cname.to_string(),
                    ci_net,
                    cyinit_net,
                    di_nets,
                    s_nets,
                    o_nets,
                    co_nets,
                });
            }
            CellKind::Hff => {
                let d_net = pin_ix.net_on(cname, "D").map(|s| resolve(s).to_string());
                let q_net = pin_ix.net_on(cname, "Q").map(|s| resolve(s).to_string());
                let ce_net = pin_ix.net_on(cname, "CE").map(|s| resolve(s).to_string());
                let rst_net = pin_ix
                    .net_on(cname, "R")
                    .or_else(|| pin_ix.net_on(cname, "RST"))
                    .or_else(|| pin_ix.net_on(cname, "SR"))
                    .map(|s| resolve(s).to_string());
                ffs.push(VerilateFf {
                    cell_name: cname.to_string(),
                    d_net,
                    q_net,
                    ce_net,
                    rst_net,
                });
            }
            CellKind::Bram18 | CellKind::Bram18e1 { .. } => {
                let mut raddr: [Option<String>; 10] = Default::default();
                let mut waddr: [Option<String>; 10] = Default::default();
                for i in 0..10 {
                    raddr[i] = pin_ix
                        .net_on(cname, &format!("RADDR{i}"))
                        .or_else(|| pin_ix.net_on(cname, &format!("ADDRARDADDR{i}")))
                        .map(|s| resolve(s).to_string());
                    waddr[i] = pin_ix
                        .net_on(cname, &format!("WADDR{i}"))
                        .or_else(|| pin_ix.net_on(cname, &format!("ADDRBWRADDR{i}")))
                        .map(|s| resolve(s).to_string());
                }
                let mut di: [Option<String>; 16] = Default::default();
                let mut do_nets: [Option<String>; 16] = Default::default();
                for i in 0..16 {
                    di[i] = pin_ix
                        .net_on(cname, &format!("DI{i}"))
                        .or_else(|| pin_ix.net_on(cname, &format!("WDATA{i}")))
                        .or_else(|| pin_ix.net_on(cname, &format!("DIADI{i}")))
                        .map(|s| resolve(s).to_string());
                    do_nets[i] = pin_ix
                        .net_on(cname, &format!("DO{i}"))
                        .or_else(|| pin_ix.net_on(cname, &format!("RDATA{i}")))
                        .or_else(|| pin_ix.net_on(cname, &format!("DOADO{i}")))
                        .map(|s| resolve(s).to_string());
                }
                let we_net = pin_ix
                    .net_on(cname, "WE")
                    .or_else(|| pin_ix.net_on(cname, "WEA0"))
                    .map(|s| resolve(s).to_string());
                rams.push(VerilateRam18 {
                    cell_name: cname.to_string(),
                    raddr,
                    waddr,
                    di,
                    do_nets,
                    we_net,
                });
            }
            CellKind::IobIn | CellKind::ClockBuf => {
                let i_pin = pin_ix
                    .net_on(cname, "I")
                    .or_else(|| pin_ix.net_on(cname, "PAD"));
                let o_pin = pin_ix.net_on(cname, "O");
                if let (Some(src), Some(dst)) = (i_pin, o_pin) {
                    let s_name = resolve(src).to_string();
                    let d_name = resolve(dst).to_string();
                    if s_name != d_name {
                        raw_ops.push(VerilateCombOp::Assign {
                            dst_net: d_name,
                            src_net: s_name,
                        });
                    }
                }
            }
            CellKind::IobOut => {
                if let Some(i_s) = pin_ix.net_on(cname, "I") {
                    let i_name = resolve(i_s).to_string();
                    if led_source.is_none() {
                        led_source = Some(i_name.clone());
                    }
                    if let Some(pad_s) = pin_ix.net_on(cname, "PAD") {
                        let pad_name = resolve(pad_s).to_string();
                        if pad_name != i_name {
                            raw_ops.push(VerilateCombOp::Assign {
                                dst_net: pad_name,
                                src_net: i_name,
                            });
                        }
                    }
                }
            }
            _ => {
                if let Some(is_f8) = classify_muxf(cell) {
                    if let Some(out_s) = pin_ix.net_on(cname, "O") {
                        raw_ops.push(VerilateCombOp::Mux2 {
                            cell_name: cname.to_string(),
                            is_f8,
                            i0_net: pin_ix.net_on(cname, "I0").map(|s| resolve(s).to_string()),
                            i1_net: pin_ix.net_on(cname, "I1").map(|s| resolve(s).to_string()),
                            s_net: pin_ix.net_on(cname, "S").map(|s| resolve(s).to_string()),
                            out_net: resolve(out_s).to_string(),
                        });
                    }
                }
            }
        }
    }

    // Topological levelization of `raw_ops` via Kahn's algorithm.
    let n_ops = raw_ops.len();
    let mut driver_of_net: HashMap<String, usize> = HashMap::new();
    for (idx, op) in raw_ops.iter().enumerate() {
        for out_net in op.output_nets() {
            driver_of_net.insert(out_net.to_string(), idx);
        }
    }
    // Sequential FF Q outputs break combinational dependencies.
    for ff in &ffs {
        if let Some(q) = &ff.q_net {
            driver_of_net.remove(q);
        }
    }

    let mut in_deg = vec![0usize; n_ops];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n_ops];
    for (idx, op) in raw_ops.iter().enumerate() {
        let mut preds = Vec::new();
        for inp in op.input_nets() {
            if let Some(&drv) = driver_of_net.get(inp) {
                if drv != idx && !preds.contains(&drv) {
                    preds.push(drv);
                }
            }
        }
        in_deg[idx] = preds.len();
        for p in preds {
            adj[p].push(idx);
        }
    }

    let mut q: VecDeque<usize> = in_deg
        .iter()
        .enumerate()
        .filter_map(|(i, &d)| (d == 0).then_some(i))
        .collect();
    let mut visited = vec![false; n_ops];
    let mut ordered_ops = Vec::with_capacity(n_ops);
    while let Some(u) = q.pop_front() {
        visited[u] = true;
        ordered_ops.push(raw_ops[u].clone());
        for &v in &adj[u] {
            in_deg[v] = in_deg[v].saturating_sub(1);
            if in_deg[v] == 0 {
                q.push_back(v);
            }
        }
    }
    for i in 0..n_ops {
        if !visited[i] {
            ordered_ops.push(raw_ops[i].clone());
        }
    }

    VerilateSchedule {
        module_name: resolve(design.name).to_string(),
        nets: net_set.into_keys().collect(),
        ops: ordered_ops,
        ffs,
        rams,
        led_source,
    }
}

fn net_expr_cpp(net: Option<&str>) -> String {
    match net {
        None | Some("0") | Some("1'b0") | Some("gnd") | Some("GND") => "0ULL".to_string(),
        Some("1") | Some("1'b1") | Some("vcc") | Some("VCC") => "1ULL".to_string(),
        Some(n) => format!("(this->n_{} & 1ULL)", sanitize_ident(n)),
    }
}

fn net_expr_rust(net: Option<&str>) -> String {
    match net {
        None | Some("0") | Some("1'b0") | Some("gnd") | Some("GND") => "0u64".to_string(),
        Some("1") | Some("1'b1") | Some("vcc") | Some("VCC") => "1u64".to_string(),
        Some(n) => format!("(self.n_{} & 1u64)", sanitize_ident(n)),
    }
}

/// Emit a standalone, zero-dependency C++20 cycle-accurate model (`struct VHelionModel`)
/// for `design` using topologically levelized combinational evaluation and branchless
/// 64-bit LUT truth-table shifts (`(0x...ULL >> idx) & 1ULL`).
pub fn emit_cpp_cycle_model(design: &Design) -> String {
    let sched = build_verilate_schedule(design);
    let mut out = String::with_capacity(4096);

    out.push_str("// Generated by helion-sim verilate::emit_cpp_cycle_model\n");
    out.push_str("// Target standard: C++20 (standalone, zero external dependencies)\n");
    out.push_str("#ifndef VHELION_MODEL_H\n#define VHELION_MODEL_H\n#include <cstdint>\n#include <array>\n\n");
    out.push_str(&format!(
        "struct VHelionModel {{\n    // Module: {}\n",
        sched.module_name
    ));
    out.push_str("    uint64_t cycle{0};\n");
    out.push_str("    uint64_t led{0};\n");

    // Net state variables
    for net in &sched.nets {
        out.push_str(&format!("    uint64_t n_{}{{0}};\n", sanitize_ident(net)));
    }
    // Sequential FF state variables
    for ff in &sched.ffs {
        out.push_str(&format!(
            "    uint64_t ff_{}{{0}};\n",
            sanitize_ident(&ff.cell_name)
        ));
    }
    // Block RAM 18Kb arrays
    for ram in &sched.rams {
        out.push_str(&format!(
            "    std::array<uint16_t, 1024> ram_{}{{}};\n",
            sanitize_ident(&ram.cell_name)
        ));
    }

    // eval_comb()
    out.push_str("\n    constexpr void eval_comb() noexcept {\n");
    for ff in &sched.ffs {
        if let Some(q) = &ff.q_net {
            out.push_str(&format!(
                "        this->n_{} = this->ff_{} & 1ULL;\n",
                sanitize_ident(q),
                sanitize_ident(&ff.cell_name)
            ));
        }
    }
    for op in &sched.ops {
        match op {
            VerilateCombOp::Lut6 {
                cell_name,
                init,
                inputs,
                out_net,
            } => {
                let cid = sanitize_ident(cell_name);
                let mut terms = Vec::new();
                for (pin, inp) in inputs.iter().enumerate() {
                    if let Some(n) = inp.as_deref() {
                        let e = net_expr_cpp(Some(n));
                        if pin == 0 {
                            terms.push(e);
                        } else {
                            terms.push(format!("({e} << {pin}U)"));
                        }
                    }
                }
                let idx_expr = if terms.is_empty() {
                    "0ULL".to_string()
                } else {
                    terms.join(" | ")
                };
                out.push_str(&format!(
                    "        const uint64_t idx_{cid} = {idx_expr};\n"
                ));
                out.push_str(&format!(
                    "        this->n_{} = (0x{init:016X}ULL >> idx_{cid}) & 1ULL;\n",
                    sanitize_ident(out_net)
                ));
            }
            VerilateCombOp::Carry4 {
                cell_name,
                ci_net,
                cyinit_net,
                di_nets,
                s_nets,
                o_nets,
                co_nets,
            } => {
                let cid = sanitize_ident(cell_name);
                let c0 = if ci_net.is_some() {
                    net_expr_cpp(ci_net.as_deref())
                } else {
                    net_expr_cpp(cyinit_net.as_deref())
                };
                out.push_str(&format!("        uint64_t c_{cid} = {c0};\n"));
                for i in 0..4 {
                    let s_e = net_expr_cpp(s_nets[i].as_deref());
                    let di_e = net_expr_cpp(di_nets[i].as_deref());
                    out.push_str(&format!(
                        "        const uint64_t s_{cid}_{i} = {s_e};\n        const uint64_t di_{cid}_{i} = {di_e};\n"
                    ));
                    if let Some(on) = &o_nets[i] {
                        out.push_str(&format!(
                            "        this->n_{} = (s_{cid}_{i} ^ c_{cid}) & 1ULL;\n",
                            sanitize_ident(on)
                        ));
                    }
                    out.push_str(&format!(
                        "        c_{cid} = ((s_{cid}_{i} & c_{cid}) | ((1ULL ^ s_{cid}_{i}) & di_{cid}_{i})) & 1ULL;\n"
                    ));
                    if let Some(con) = &co_nets[i] {
                        out.push_str(&format!(
                            "        this->n_{} = c_{cid};\n",
                            sanitize_ident(con)
                        ));
                    }
                }
            }
            VerilateCombOp::Mux2 {
                cell_name,
                is_f8,
                i0_net,
                i1_net,
                s_net,
                out_net,
            } => {
                let tag = if *is_f8 { "muxf8" } else { "muxf7" };
                let cid = sanitize_ident(cell_name);
                let s_e = net_expr_cpp(s_net.as_deref());
                let i0_e = net_expr_cpp(i0_net.as_deref());
                let i1_e = net_expr_cpp(i1_net.as_deref());
                out.push_str(&format!(
                    "        // {tag} {cid}\n        this->n_{} = (({s_e} & {i1_e}) | ((1ULL ^ {s_e}) & {i0_e})) & 1ULL;\n",
                    sanitize_ident(out_net)
                ));
            }
            VerilateCombOp::Assign { dst_net, src_net } => {
                out.push_str(&format!(
                    "        this->n_{} = {};\n",
                    sanitize_ident(dst_net),
                    net_expr_cpp(Some(src_net))
                ));
            }
        }
    }
    if let Some(led_src) = &sched.led_source {
        out.push_str(&format!(
            "        this->led = {};\n",
            net_expr_cpp(Some(led_src))
        ));
    } else if sched.nets.iter().any(|n| n == "led") {
        out.push_str("        this->led = this->n_led & 1ULL;\n");
    }
    out.push_str("    }\n");

    // tick()
    out.push_str("\n    constexpr void tick() noexcept {\n");
    out.push_str("        this->eval_comb();\n");
    for ff in &sched.ffs {
        let fid = sanitize_ident(&ff.cell_name);
        let d_e = net_expr_cpp(ff.d_net.as_deref());
        let ce_e = match &ff.ce_net {
            Some(ce) => net_expr_cpp(Some(ce)),
            None => "1ULL".to_string(),
        };
        let rst_e = net_expr_cpp(ff.rst_net.as_deref());
        out.push_str(&format!(
            "        const uint64_t next_ff_{fid} = ({rst_e} != 0ULL) ? 0ULL : (({ce_e} != 0ULL) ? {d_e} : this->ff_{fid});\n"
        ));
    }
    for ram in &sched.rams {
        let rid = sanitize_ident(&ram.cell_name);
        let mut r_terms = Vec::new();
        let mut w_terms = Vec::new();
        for i in 0..10 {
            if let Some(rn) = ram.raddr[i].as_deref() {
                r_terms.push(format!("({} << {i}U)", net_expr_cpp(Some(rn))));
            }
            if let Some(wn) = ram.waddr[i].as_deref() {
                w_terms.push(format!("({} << {i}U)", net_expr_cpp(Some(wn))));
            }
        }
        let raddr_e = if r_terms.is_empty() {
            "0ULL".to_string()
        } else {
            r_terms.join(" | ")
        };
        let waddr_e = if w_terms.is_empty() {
            "0ULL".to_string()
        } else {
            w_terms.join(" | ")
        };
        let mut di_terms = Vec::new();
        for i in 0..16 {
            if let Some(dn) = ram.di[i].as_deref() {
                di_terms.push(format!("({} << {i}U)", net_expr_cpp(Some(dn))));
            }
        }
        let di_e = if di_terms.is_empty() {
            "0ULL".to_string()
        } else {
            di_terms.join(" | ")
        };
        let we_e = match &ram.we_net {
            Some(we) => net_expr_cpp(Some(we)),
            None => "0ULL".to_string(),
        };
        out.push_str(&format!(
            "        if ({we_e} != 0ULL) {{ this->ram_{rid}[({waddr_e}) & 0x3FFULL] = static_cast<uint16_t>({di_e}); }}\n"
        ));
        out.push_str(&format!(
            "        const uint16_t rword_{rid} = this->ram_{rid}[({raddr_e}) & 0x3FFULL];\n"
        ));
        for i in 0..16 {
            if let Some(don) = &ram.do_nets[i] {
                out.push_str(&format!(
                    "        this->n_{} = (rword_{rid} >> {i}U) & 1U;\n",
                    sanitize_ident(don)
                ));
            }
        }
    }
    for ff in &sched.ffs {
        let fid = sanitize_ident(&ff.cell_name);
        out.push_str(&format!("        this->ff_{fid} = next_ff_{fid};\n"));
        if let Some(q) = &ff.q_net {
            out.push_str(&format!(
                "        this->n_{} = this->ff_{fid};\n",
                sanitize_ident(q)
            ));
        }
    }
    out.push_str("        this->cycle += 1ULL;\n");
    out.push_str("        this->eval_comb();\n");
    out.push_str("    }\n};\n#endif // VHELION_MODEL_H\n");
    out
}

/// Emit a standalone, zero-dependency Rust cycle-accurate model (`pub struct VHelionModel`)
/// for `design` using topologically levelized combinational evaluation and branchless
/// 64-bit LUT truth-table shifts (`(0x...u64 >> idx) & 1u64`).
pub fn emit_rust_cycle_model(design: &Design) -> String {
    let sched = build_verilate_schedule(design);
    let mut out = String::with_capacity(4096);

    out.push_str("// Generated by helion-sim verilate::emit_rust_cycle_model\n");
    out.push_str("#[derive(Clone, Debug)]\n");
    out.push_str("pub struct VHelionModel {\n");
    out.push_str("    pub cycle: u64,\n");
    out.push_str("    pub led: u64,\n");
    for net in &sched.nets {
        out.push_str(&format!("    pub n_{}: u64,\n", sanitize_ident(net)));
    }
    for ff in &sched.ffs {
        out.push_str(&format!(
            "    pub ff_{}: u64,\n",
            sanitize_ident(&ff.cell_name)
        ));
    }
    for ram in &sched.rams {
        out.push_str(&format!(
            "    pub ram_{}: [u16; 1024],\n",
            sanitize_ident(&ram.cell_name)
        ));
    }
    out.push_str("}\n\n");

    out.push_str("impl Default for VHelionModel {\n    fn default() -> Self {\n        Self::new()\n    }\n}\n\n");
    out.push_str("impl VHelionModel {\n    pub const fn new() -> Self {\n        Self {\n            cycle: 0,\n            led: 0,\n");
    for net in &sched.nets {
        out.push_str(&format!("            n_{}: 0,\n", sanitize_ident(net)));
    }
    for ff in &sched.ffs {
        out.push_str(&format!(
            "            ff_{}: 0,\n",
            sanitize_ident(&ff.cell_name)
        ));
    }
    for ram in &sched.rams {
        out.push_str(&format!(
            "            ram_{}: [0u16; 1024],\n",
            sanitize_ident(&ram.cell_name)
        ));
    }
    out.push_str("        }\n    }\n\n");

    // eval_comb(&mut self)
    out.push_str("    #[inline]\n    pub fn eval_comb(&mut self) {\n");
    for ff in &sched.ffs {
        if let Some(q) = &ff.q_net {
            out.push_str(&format!(
                "        self.n_{} = self.ff_{} & 1u64;\n",
                sanitize_ident(q),
                sanitize_ident(&ff.cell_name)
            ));
        }
    }
    for op in &sched.ops {
        match op {
            VerilateCombOp::Lut6 {
                cell_name,
                init,
                inputs,
                out_net,
            } => {
                let cid = sanitize_ident(cell_name);
                let mut terms = Vec::new();
                for (pin, inp) in inputs.iter().enumerate() {
                    if let Some(n) = inp.as_deref() {
                        let e = net_expr_rust(Some(n));
                        if pin == 0 {
                            terms.push(e);
                        } else {
                            terms.push(format!("({e} << {pin})"));
                        }
                    }
                }
                let idx_expr = if terms.is_empty() {
                    "0u64".to_string()
                } else {
                    terms.join(" | ")
                };
                out.push_str(&format!("        let idx_{cid}: u64 = {idx_expr};\n"));
                out.push_str(&format!(
                    "        self.n_{} = (0x{init:016X}u64 >> idx_{cid}) & 1u64;\n",
                    sanitize_ident(out_net)
                ));
            }
            VerilateCombOp::Carry4 {
                cell_name,
                ci_net,
                cyinit_net,
                di_nets,
                s_nets,
                o_nets,
                co_nets,
            } => {
                let cid = sanitize_ident(cell_name);
                let c0 = if ci_net.is_some() {
                    net_expr_rust(ci_net.as_deref())
                } else {
                    net_expr_rust(cyinit_net.as_deref())
                };
                out.push_str(&format!("        let mut c_{cid}: u64 = {c0};\n"));
                for i in 0..4 {
                    let s_e = net_expr_rust(s_nets[i].as_deref());
                    let di_e = net_expr_rust(di_nets[i].as_deref());
                    out.push_str(&format!(
                        "        let s_{cid}_{i}: u64 = {s_e};\n        let di_{cid}_{i}: u64 = {di_e};\n"
                    ));
                    if let Some(on) = &o_nets[i] {
                        out.push_str(&format!(
                            "        self.n_{} = (s_{cid}_{i} ^ c_{cid}) & 1u64;\n",
                            sanitize_ident(on)
                        ));
                    }
                    out.push_str(&format!(
                        "        c_{cid} = ((s_{cid}_{i} & c_{cid}) | ((1u64 ^ s_{cid}_{i}) & di_{cid}_{i})) & 1u64;\n"
                    ));
                    if let Some(con) = &co_nets[i] {
                        out.push_str(&format!(
                            "        self.n_{} = c_{cid};\n",
                            sanitize_ident(con)
                        ));
                    }
                }
            }
            VerilateCombOp::Mux2 {
                cell_name,
                is_f8: _,
                i0_net,
                i1_net,
                s_net,
                out_net,
            } => {
                let _ = cell_name;
                let s_e = net_expr_rust(s_net.as_deref());
                let i0_e = net_expr_rust(i0_net.as_deref());
                let i1_e = net_expr_rust(i1_net.as_deref());
                out.push_str(&format!(
                    "        self.n_{} = (({s_e} & {i1_e}) | ((1u64 ^ {s_e}) & {i0_e})) & 1u64;\n",
                    sanitize_ident(out_net)
                ));
            }
            VerilateCombOp::Assign { dst_net, src_net } => {
                out.push_str(&format!(
                    "        self.n_{} = {};\n",
                    sanitize_ident(dst_net),
                    net_expr_rust(Some(src_net))
                ));
            }
        }
    }
    if let Some(led_src) = &sched.led_source {
        out.push_str(&format!(
            "        self.led = {};\n",
            net_expr_rust(Some(led_src))
        ));
    } else if sched.nets.iter().any(|n| n == "led") {
        out.push_str("        self.led = self.n_led & 1u64;\n");
    }
    out.push_str("    }\n\n");

    // tick(&mut self)
    out.push_str("    #[inline]\n    pub fn tick(&mut self) {\n");
    out.push_str("        self.eval_comb();\n");
    for ff in &sched.ffs {
        let fid = sanitize_ident(&ff.cell_name);
        let d_e = net_expr_rust(ff.d_net.as_deref());
        let ce_e = match &ff.ce_net {
            Some(ce) => net_expr_rust(Some(ce)),
            None => "1u64".to_string(),
        };
        let rst_e = net_expr_rust(ff.rst_net.as_deref());
        out.push_str(&format!(
            "        let next_ff_{fid}: u64 = if {rst_e} != 0 {{ 0 }} else if {ce_e} != 0 {{ {d_e} }} else {{ self.ff_{fid} }};\n"
        ));
    }
    for ff in &sched.ffs {
        let fid = sanitize_ident(&ff.cell_name);
        out.push_str(&format!("        self.ff_{fid} = next_ff_{fid};\n"));
        if let Some(q) = &ff.q_net {
            out.push_str(&format!(
                "        self.n_{} = self.ff_{fid};\n",
                sanitize_ident(q)
            ));
        }
    }
    out.push_str("        self.cycle += 1;\n");
    out.push_str("        self.eval_comb();\n");
    out.push_str("    }\n}\n");
    out
}

// ============================================================================
// Part 2: IEEE 1364 / 1800 VPI Signal Introspection API (`VpiRegistry`)
// ============================================================================

/// Opaque handle to an IEEE 1800 VPI object (`vpiHandle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VpiHandle(pub u32);

/// VPI object classification (`vpiPort`, `vpiNet`, `vpiReg`, `vpiMemory`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VpiObjectKind {
    Port,
    Net,
    Reg,
}

/// Metadata for a registered VPI signal object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VpiObjectInfo {
    pub handle: VpiHandle,
    pub name: String,
    pub full_name: String,
    pub kind: VpiObjectKind,
    pub width: u32,
    pub dir: Option<PortDir>,
    /// Underlying scalar signal names (`[lsb..msb]` for multi-bit buses, or `[name]` for 1-bit).
    pub bit_signals: Vec<String>,
}

/// IEEE 1800 `s_vpi_value.format` selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VpiFormat {
    /// `vpiBinStrVal`: binary string (`"1010"`).
    BinStr,
    /// `vpiHexStrVal`: lowercase hexadecimal string (`"a"`).
    HexStr,
    /// `vpiDecStrVal`: unsigned decimal string (`"10"`).
    DecStr,
    /// `vpiIntVal`: integer value.
    IntVal,
    /// `vpiVectorVal`: array of 32-bit 4-state `(aval, bval)` chunks (`s_vpi_vecval`).
    VectorVal,
}

/// One 32-bit 4-state `s_vpi_vecval` chunk (`bval = 0` for known `0`/`1`, `bval = 1` for `X`/`Z`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VpiVecVal {
    pub aval: u32,
    pub bval: u32,
}

/// Value returned by [`VpiRegistry::vpi_get_value`] or passed to [`VpiRegistry::vpi_put_value`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VpiValue {
    BinStr(String),
    HexStr(String),
    DecStr(String),
    IntVal(i64),
    VectorVal(Vec<VpiVecVal>),
}

impl VpiValue {
    /// Parse or extract a `(u64_val, xz_mask)` pair from this [`VpiValue`].
    pub fn to_u64_and_xz(&self) -> Result<(u64, u64), String> {
        match self {
            Self::IntVal(v) => Ok((*v as u64, 0)),
            Self::DecStr(s) => {
                let v: u64 = s
                    .trim()
                    .parse()
                    .map_err(|_| format!("vpi: invalid decimal string `{s}`"))?;
                Ok((v, 0))
            }
            Self::BinStr(s) => {
                let mut val = 0u64;
                let mut xz = 0u64;
                for ch in s.trim().chars().filter(|&c| c != '_') {
                    val <<= 1;
                    xz <<= 1;
                    match ch {
                        '0' => {}
                        '1' => val |= 1,
                        'x' | 'X' => xz |= 1,
                        'z' | 'Z' => {
                            val |= 1;
                            xz |= 1;
                        }
                        _ => return Err(format!("vpi: invalid binary char `{ch}` in `{s}`")),
                    }
                }
                Ok((val, xz))
            }
            Self::HexStr(s) => {
                let cleaned: String = s
                    .trim()
                    .trim_start_matches("0x")
                    .trim_start_matches("0X")
                    .chars()
                    .filter(|&c| c != '_')
                    .collect();
                let mut val = 0u64;
                let mut xz = 0u64;
                for ch in cleaned.chars() {
                    val <<= 4;
                    xz <<= 4;
                    match ch {
                        '0'..='9' => val |= u64::from(ch as u8 - b'0'),
                        'a'..='f' => val |= u64::from(ch as u8 - b'a' + 10),
                        'A'..='F' => val |= u64::from(ch as u8 - b'A' + 10),
                        'x' | 'X' => xz |= 0xF,
                        'z' | 'Z' => {
                            val |= 0xF;
                            xz |= 0xF;
                        }
                        _ => return Err(format!("vpi: invalid hex char `{ch}` in `{s}`")),
                    }
                }
                Ok((val, xz))
            }
            Self::VectorVal(chunks) => {
                let lo = chunks.first().copied().unwrap_or_default();
                let hi = chunks.get(1).copied().unwrap_or_default();
                let val = u64::from(lo.aval) | (u64::from(hi.aval) << 32);
                let xz = u64::from(lo.bval) | (u64::from(hi.bval) << 32);
                Ok((val, xz))
            }
        }
    }
}

/// IEEE 1800 `vpi_put_value` scheduling flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VpiPutFlags {
    /// `vpiNoDelay`: immediate deposit/drive on port or register.
    NoDelay,
    /// `vpiInertialDelay`: scheduled inertial drive applied before the next clock edge.
    InertialDelay,
    /// `vpiForceFlag`: continuous force overriding HDL drivers across clock edges.
    ForceFlag,
    /// `vpiReleaseFlag`: release an active force so HDL drivers resume.
    ReleaseFlag,
}

fn split_bus_suffix(name: &str) -> Option<(String, usize)> {
    if let Some((base, bit_str)) = name.rsplit_once('_') {
        if !base.is_empty() && bit_str.chars().all(|c| c.is_ascii_digit()) {
            let idx: usize = bit_str.parse().ok()?;
            return Some((base.to_string(), idx));
        }
    }
    if let Some(open) = name.rfind('[') {
        if name.ends_with(']') && open > 0 {
            let base = &name[..open];
            let idx: usize = name[open + 1..name.len() - 1].parse().ok()?;
            return Some((base.to_string(), idx));
        }
    }
    let digit_pos = name.rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1)?;
    if digit_pos > 0 && digit_pos < name.len() && name.len() - digit_pos == 1 {
        let base = &name[..digit_pos];
        if base == "q" || base == "d" || base.contains("cnt") {
            let idx: usize = name[digit_pos..].parse().ok()?;
            return Some((base.to_string(), idx));
        }
    }
    None
}

/// IEEE 1364 / 1800 VPI signal introspection and control registry.
#[derive(Clone, Debug, Default)]
pub struct VpiRegistry {
    module_name: String,
    objects: Vec<VpiObjectInfo>,
    by_name: HashMap<String, VpiHandle>,
    port_handles: Vec<VpiHandle>,
    net_handles: Vec<VpiHandle>,
    pending_inertial: Vec<(VpiHandle, u64)>,
}

impl VpiRegistry {
    /// Construct a [`VpiRegistry`] populated with all ports, nets, flip-flops, and
    /// reconstructed multi-bit buses from `design`.
    pub fn new(design: &Design) -> Self {
        let module_name = resolve(design.name).to_string();
        let mut reg = Self {
            module_name: module_name.clone(),
            objects: Vec::new(),
            by_name: HashMap::new(),
            port_handles: Vec::new(),
            net_handles: Vec::new(),
            pending_inertial: Vec::new(),
        };

        // 1. Primary ports (plus synthetic `led` if not already declared as a port).
        for p in &design.ports {
            let pname = resolve(p.name).to_string();
            let h = reg.add_obj(
                pname.clone(),
                VpiObjectKind::Port,
                1,
                Some(p.dir),
                vec![pname],
            );
            reg.port_handles.push(h);
        }
        if !reg.by_name.contains_key("led") {
            let h = reg.add_obj(
                "led".to_string(),
                VpiObjectKind::Port,
                1,
                Some(PortDir::Out),
                vec!["led".to_string()],
            );
            reg.port_handles.push(h);
        }

        // 2. Sequential HFF registers.
        for c in &design.cells {
            if matches!(c.kind, CellKind::Hff) {
                let cname = resolve(c.name).to_string();
                let h = reg.add_obj(cname.clone(), VpiObjectKind::Reg, 1, None, vec![cname]);
                reg.net_handles.push(h);
            }
        }

        // 3. Nets and auto-assembled multi-bit vector buses.
        let mut bus_map: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        for n in &design.nets {
            let nname = resolve(n.name).to_string();
            if nname == "0" || nname == "1" || nname == "1'b0" || nname == "1'b1" {
                continue;
            }
            if let Some((prefix, bit_idx)) = split_bus_suffix(&nname) {
                bus_map
                    .entry(prefix)
                    .or_default()
                    .insert(bit_idx, nname.clone());
            }
            if !reg.by_name.contains_key(&nname) {
                let h = reg.add_obj(nname.clone(), VpiObjectKind::Net, 1, None, vec![nname]);
                reg.net_handles.push(h);
            }
        }

        for (prefix, bits) in bus_map {
            if bits.len() >= 2
                && (0..bits.len()).all(|i| bits.contains_key(&i))
                && !reg.by_name.contains_key(&prefix)
            {
                let bit_signals: Vec<String> = (0..bits.len()).map(|i| bits[&i].clone()).collect();
                let width = bit_signals.len() as u32;
                let h = reg.add_obj(prefix, VpiObjectKind::Net, width, None, bit_signals);
                reg.net_handles.push(h);
            }
        }

        reg
    }

    fn add_obj(
        &mut self,
        name: String,
        kind: VpiObjectKind,
        width: u32,
        dir: Option<PortDir>,
        bit_signals: Vec<String>,
    ) -> VpiHandle {
        if let Some(&existing) = self.by_name.get(&name) {
            return existing;
        }
        let h = VpiHandle(self.objects.len() as u32);
        let full_name = format!("{}.{}", self.module_name, name);
        self.by_name.insert(name.clone(), h);
        self.by_name.insert(full_name.clone(), h);
        self.objects.push(VpiObjectInfo {
            handle: h,
            name,
            full_name,
            kind,
            width,
            dir,
            bit_signals,
        });
        h
    }

    /// Look up a [`VpiHandle`] by signal name (`"q0"`, `"counter.q0"`, `"top.cnt"`, `"led"`).
    pub fn vpi_handle_by_name(&self, name: &str) -> Option<VpiHandle> {
        let trimmed = name.trim();
        if let Some(&h) = self.by_name.get(trimmed) {
            return Some(h);
        }
        // Strip leading hierarchy prefix (`top.dut.sig` -> `sig`).
        if let Some((_, tail)) = trimmed.rsplit_once('.') {
            if let Some(&h) = self.by_name.get(tail) {
                return Some(h);
            }
        }
        if let Some((_, tail)) = trimmed.rsplit_once('/') {
            if let Some(&h) = self.by_name.get(tail) {
                return Some(h);
            }
        }
        None
    }

    /// Inspect metadata for a [`VpiHandle`].
    pub fn vpi_object_info(&self, handle: VpiHandle) -> Option<&VpiObjectInfo> {
        self.objects.get(handle.0 as usize)
    }

    /// Return handles for all top-level ports (`vpi_iterate(vpiPort, top)`).
    pub fn vpi_iterate_ports(&self) -> Vec<VpiHandle> {
        self.port_handles.clone()
    }

    /// Return handles for all internal nets, registers, and vector buses (`vpi_iterate(vpiNet, top)`).
    pub fn vpi_iterate_nets(&self) -> Vec<VpiHandle> {
        self.net_handles.clone()
    }

    /// Read the current value of `handle` from `sim` formatted according to `format`.
    pub fn vpi_get_value(
        &self,
        sim: &Sim,
        handle: VpiHandle,
        format: VpiFormat,
    ) -> Result<VpiValue, String> {
        let info = self
            .objects
            .get(handle.0 as usize)
            .ok_or_else(|| format!("vpi_get_value: invalid VpiHandle({})", handle.0))?;

        let mut aval = 0u64;
        let mut bval = 0u64;
        for (bit_idx, sig_name) in info.bit_signals.iter().enumerate() {
            if bit_idx >= 64 {
                break;
            }
            let sampled = if sig_name.eq_ignore_ascii_case("led") {
                Some(u64::from(sim.led))
            } else {
                sim.sample(sig_name)
                    .or_else(|| sim.net_value(sig_name).map(u64::from))
            };
            match sampled {
                Some(v) => {
                    if (v & 1) != 0 {
                        aval |= 1u64 << bit_idx;
                    }
                }
                None => {
                    // Unknown `X` in 4-state VPI representation: `aval = 0, bval = 1`.
                    bval |= 1u64 << bit_idx;
                }
            }
        }

        let width = info.width.max(1) as usize;
        match format {
            VpiFormat::IntVal => Ok(VpiValue::IntVal(aval as i64)),
            VpiFormat::DecStr => {
                if bval != 0 {
                    Ok(VpiValue::DecStr("x".to_string()))
                } else {
                    Ok(VpiValue::DecStr(format!("{aval}")))
                }
            }
            VpiFormat::BinStr => {
                let mut s = String::with_capacity(width);
                for b in (0..width).rev() {
                    if ((bval >> b) & 1) != 0 {
                        s.push('x');
                    } else if ((aval >> b) & 1) != 0 {
                        s.push('1');
                    } else {
                        s.push('0');
                    }
                }
                Ok(VpiValue::BinStr(s))
            }
            VpiFormat::HexStr => {
                let nibbles = (width + 3) / 4;
                let mut s = String::with_capacity(nibbles);
                for n in (0..nibbles).rev() {
                    let shift = n * 4;
                    let mask = if shift + 4 <= width {
                        0xFu64
                    } else {
                        (1u64 << (width - shift)) - 1
                    };
                    let b_nib = (bval >> shift) & mask;
                    let a_nib = (aval >> shift) & mask;
                    if b_nib != 0 {
                        s.push('x');
                    } else {
                        s.push(b"0123456789abcdef"[a_nib as usize] as char);
                    }
                }
                Ok(VpiValue::HexStr(s))
            }
            VpiFormat::VectorVal => {
                let mut vecs = vec![VpiVecVal {
                    aval: aval as u32,
                    bval: bval as u32,
                }];
                if width > 32 {
                    vecs.push(VpiVecVal {
                        aval: (aval >> 32) as u32,
                        bval: (bval >> 32) as u32,
                    });
                }
                Ok(VpiValue::VectorVal(vecs))
            }
        }
    }

    /// Write or force `value` onto `handle` in `sim` according to `flags`.
    pub fn vpi_put_value(
        &mut self,
        sim: &mut Sim,
        handle: VpiHandle,
        value: &VpiValue,
        flags: VpiPutFlags,
    ) -> Result<(), String> {
        let info = self
            .objects
            .get(handle.0 as usize)
            .ok_or_else(|| format!("vpi_put_value: invalid VpiHandle({})", handle.0))?
            .clone();
        let (word_val, _) = value.to_u64_and_xz()?;

        match flags {
            VpiPutFlags::InertialDelay => {
                self.pending_inertial.push((handle, word_val));
                Ok(())
            }
            VpiPutFlags::NoDelay => {
                self.apply_immediate(sim, &info, word_val, false)
            }
            VpiPutFlags::ForceFlag => {
                self.apply_immediate(sim, &info, word_val, true)
            }
            VpiPutFlags::ReleaseFlag => {
                for sig in &info.bit_signals {
                    if let Some(ff_cell) = sim.net_ff_cell(sig) {
                        let _ = sim.release(&ff_cell);
                    } else {
                        let _ = sim.release(sig);
                    }
                }
                Ok(())
            }
        }
    }

    /// Flush any pending `VpiPutFlags::InertialDelay` puts into `sim` prior to a clock edge.
    pub fn flush_inertial_puts(&mut self, sim: &mut Sim) -> Result<(), String> {
        let pending = std::mem::take(&mut self.pending_inertial);
        for (handle, word_val) in pending {
            if let Some(info) = self.objects.get(handle.0 as usize).cloned() {
                self.apply_immediate(sim, &info, word_val, false)?;
            }
        }
        Ok(())
    }

    fn apply_immediate(
        &self,
        sim: &mut Sim,
        info: &VpiObjectInfo,
        word_val: u64,
        is_force: bool,
    ) -> Result<(), String> {
        for (bit_idx, sig) in info.bit_signals.iter().enumerate() {
            let bit = (word_val >> bit_idx) & 1;
            if is_force {
                if sim.force(sig, bit).is_err() {
                    if let Some(ff_cell) = sim.net_ff_cell(sig) {
                        sim.force(&ff_cell, bit)?;
                    } else if sim.drive(sig, bit != 0).is_err() {
                        return Err(format!("vpi_put_value(ForceFlag): signal `{sig}` is not forceable"));
                    }
                }
            } else if sim.drive(sig, bit != 0).is_err() {
                if sim.deposit(sig, bit).is_err() {
                    if let Some(ff_cell) = sim.net_ff_cell(sig) {
                        sim.deposit(&ff_cell, bit)?;
                    } else {
                        return Err(format!(
                            "vpi_put_value: signal `{sig}` is neither an input port nor a sequential register"
                        ));
                    }
                }
            }
        }
        let _ = SimForceKind::Force;
        Ok(())
    }
}
