//! The DRC deck: rules computed from the view contract alone, so every
//! family runs the same checks.
//!
//! A rule carries its Vivado `report_drc` id where the check is Vivado's
//! (`NSTD-1`, `UCIO-1`, `BIVC-1`, `MDRV-1`, `LUTLP-1`, `RTSTAT-1`,
//! `ZPS7-1`) and a `HLN-` id where it is Helion's own. A rule whose input
//! the view does not carry is listed as not run, with the reason; it never
//! passes by default.
//!
//! No em dashes anywhere.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::view::{DrcRule, DrcViolation, ImplView, IoPlan, NetlistStage, Provenance, SchematicNetlist};

type Check = fn(&ImplView) -> Result<Vec<String>, String>;

/// `(id, is a Vivado id, severity, title, method, check)`.
const RULES: &[(&str, bool, &str, &str, &str, Check)] = &[
    (
        "NSTD-1",
        true,
        "Critical Warning",
        "Unspecified I/O standard",
        "IO pins whose standard the flow chose because the constraints set no IOSTANDARD",
        nstd,
    ),
    (
        "UCIO-1",
        true,
        "Critical Warning",
        "Unconstrained logical port",
        "ports of the synthesized netlist with fewer constrained package pins than bits (no pin, or a pin the flow chose)",
        ucio,
    ),
    (
        "BIVC-1",
        true,
        "Error",
        "Bank IO standard VCCO",
        "IO standards placed in one bank must want the same VCCO; standards with no fixed VCCO are not judged",
        bivc,
    ),
    (
        "MDRV-1",
        true,
        "Error",
        "Multiple driver nets",
        "nets of the synthesized netlist with more than one driving pin or input port",
        mdrv,
    ),
    (
        "HLN-NDRV",
        false,
        "Warning",
        "Undriven net with loads",
        "nets of the synthesized netlist with loads and no driving pin or input port (Vivado reports these in synthesis, not in report_drc)",
        ndrv,
    ),
    (
        "LUTLP-1",
        true,
        "Critical Warning",
        "Combinatorial loop",
        "strongly connected components of the synthesized netlist restricted to LUT, carry and mux cells (a loop through a flop, RAM or DSP is not combinational)",
        lutlp,
    ),
    (
        "RTSTAT-1",
        true,
        "Error",
        "Unrouted net",
        "nets the router left unrouted",
        rtstat,
    ),
    (
        "HLN-CLKPIN",
        false,
        "Error",
        "Clock enters on a pin that is not clock capable",
        "each clock net traced back through buffer cells (one input pin) to its input port; the port's package pin must be clock capable in the device model (Vivado stops in the placer with Place 30-574; it has no report_drc rule)",
        clkpin,
    ),
    (
        "ZPS7-1",
        true,
        "Warning",
        "PS7 block required",
        "a Zynq-7000 part with no placed PS7 cell",
        zps7,
    ),
];

/// Run the deck on a filled view: violations into [`ImplView::drc`], every
/// rule (run or not, with its method) into [`ImplView::drc_rules`].
pub fn fill(view: &mut ImplView) {
    let mut found = Vec::new();
    let mut deck = Vec::new();
    for &(id, vivado, severity, title, method, check) in RULES {
        let not_run = match check(view) {
            Ok(msgs) => {
                found.extend(msgs.into_iter().map(|message| DrcViolation { rule: id.to_string(), severity, message }));
                None
            }
            Err(why) => Some(why),
        };
        deck.push(DrcRule { id, vivado, title, method, not_run });
    }
    let ran: Vec<&str> = deck.iter().filter(|r| r.not_run.is_none()).map(|r| r.id).collect();
    let skipped: Vec<String> = deck.iter().filter_map(|r| Some(format!("{} ({})", r.id, r.not_run.as_ref()?))).collect();
    let mut method = format!("rules run: {}", ran.join(", "));
    if !skipped.is_empty() {
        method.push_str(&format!("; not run: {}", skipped.join("; ")));
    }
    view.drc = found;
    view.drc_rules = deck;
    view.set_provenance("drc", Provenance::new("helion-targets view_drc", method, "this run's view contract"));
}

fn io(view: &ImplView) -> Result<&IoPlan, String> {
    view.io.as_ref().ok_or_else(|| view.missing.get("io").cloned().unwrap_or_else(|| "the run has no IO plan".into()))
}

fn synthesized(view: &ImplView) -> Result<&SchematicNetlist, String> {
    view.netlist(NetlistStage::Synthesized).ok_or_else(|| {
        view.missing.get(NetlistStage::Synthesized.section()).cloned().unwrap_or_else(|| "the run kept no synthesized netlist".into())
    })
}

fn nstd(view: &ImplView) -> Result<Vec<String>, String> {
    if let Some(why) = view.missing.get("io-standard") {
        return Err(why.clone());
    }
    let ports: Vec<&str> = io(view)?.pins.iter().filter(|p| p.iostandard_default).map(|p| p.port.as_str()).collect();
    Ok(if ports.is_empty() {
        Vec::new()
    } else {
        let std = io(view)?.pins.iter().find(|p| p.iostandard_default).map(|p| p.iostandard.as_str()).unwrap_or_default();
        vec![format!("{} port(s) have no IOSTANDARD and use the default {std}: {}", ports.len(), ports.join(", "))]
    })
}

fn ucio(view: &ImplView) -> Result<Vec<String>, String> {
    let (io, syn) = (io(view)?, synthesized(view)?);
    let mut out = Vec::new();
    for p in &syn.ports {
        let bit = format!("{}[", p.name);
        let pins = io.pins.iter().filter(|i| i.port == p.name || i.port.starts_with(&bit));
        let constrained = pins.clone().filter(|i| !i.pin_default).count();
        let bits = p.nets.len().max(1);
        if constrained < bits {
            let chosen: Vec<&str> = pins.filter(|i| i.pin_default).map(|i| i.package_pin.as_str()).collect();
            out.push(if chosen.is_empty() {
                format!("{}: {constrained} of {bits} bit(s) have a package pin", p.name)
            } else {
                format!("{}: {constrained} of {bits} bit(s) constrained; the flow chose {}", p.name, chosen.join(", "))
            });
        }
    }
    Ok(out)
}

/// Supply voltage an IO standard needs from its bank's VCCO, when the
/// standard fixes one.
fn vcco_of(iostandard: &str) -> Option<&'static str> {
    Some(match iostandard.to_ascii_uppercase().as_str() {
        "LVCMOS33" | "LVTTL" | "TMDS_33" => "3.3",
        "LVCMOS25" | "LVDS_25" => "2.5",
        "LVCMOS18" => "1.8",
        "LVCMOS15" => "1.5",
        "LVCMOS12" => "1.2",
        _ => return None,
    })
}

/// Bank VCCO consistency over an IO plan: one message per bank that mixes
/// supply levels.
pub fn bank_vcco(io: &IoPlan) -> Vec<String> {
    let mut by_bank: BTreeMap<u32, BTreeMap<&'static str, BTreeSet<&str>>> = BTreeMap::new();
    for p in &io.pins {
        if let (Some(b), Some(v)) = (p.bank, vcco_of(&p.iostandard)) {
            by_bank.entry(b).or_default().entry(v).or_default().insert(p.port.as_str());
        }
    }
    by_bank
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(bank, v)| {
            let detail: Vec<String> = v.iter().map(|(volt, ports)| format!("{volt} V: {}", ports.iter().copied().collect::<Vec<_>>().join(", "))).collect();
            format!("bank {bank} mixes VCCO levels ({})", detail.join("; "))
        })
        .collect()
}

fn bivc(view: &ImplView) -> Result<Vec<String>, String> {
    if let Some(why) = view.missing.get("io-standard") {
        return Err(why.clone());
    }
    Ok(bank_vcco(io(view)?))
}

fn mdrv(view: &ImplView) -> Result<Vec<String>, String> {
    Ok(synthesized(view)?.nets.iter().filter(|n| n.drivers.len() > 1).map(|n| format!("net {} has {} drivers: {}", n.name, n.drivers.len(), n.drivers.join(", "))).collect())
}

fn ndrv(view: &ImplView) -> Result<Vec<String>, String> {
    Ok(synthesized(view)?
        .nets
        .iter()
        .filter(|n| n.drivers.is_empty() && !n.loads.is_empty())
        .map(|n| format!("net {} has no driver and {} load(s): {}", n.name, n.loads.len(), n.loads.iter().take(4).cloned().collect::<Vec<_>>().join(", ")))
        .collect())
}

/// Cell types a combinational loop can run through.
fn is_combinational(kind: &str) -> bool {
    ["LUT", "CARRY", "MUXF", "CCU2", "SB_CARRY"].iter().any(|p| kind.starts_with(p))
}

fn lutlp(view: &ImplView) -> Result<Vec<String>, String> {
    let syn = synthesized(view)?;
    let comb: Vec<usize> = (0..syn.cells.len()).filter(|&i| is_combinational(&syn.cells[i].kind)).collect();
    let id: HashMap<usize, usize> = comb.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let mut loads_of: HashMap<&str, Vec<usize>> = HashMap::new();
    for (&i, &k) in &id {
        for p in syn.cells[i].pins.iter().filter(|p| !p.output) {
            loads_of.entry(p.net.as_str()).or_default().push(k);
        }
    }
    let succ: Vec<Vec<usize>> = comb
        .iter()
        .map(|&i| {
            let mut v: Vec<usize> = syn.cells[i].pins.iter().filter(|p| p.output).flat_map(|p| loads_of.get(p.net.as_str()).into_iter().flatten().copied()).collect();
            v.sort_unstable();
            v.dedup();
            v
        })
        .collect();
    Ok(cycles(&succ)
        .into_iter()
        .map(|c| {
            let names: Vec<&str> = c.iter().take(6).map(|&k| syn.cells[comb[k]].name.as_str()).collect();
            format!("combinational loop through {} cell(s): {}{}", c.len(), names.join(" -> "), if c.len() > 6 { " ..." } else { "" })
        })
        .collect())
}

/// Strongly connected components that hold a cycle (more than one node, or
/// a node that feeds itself), by Tarjan's algorithm without recursion.
fn cycles(succ: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = succ.len();
    let (mut index, mut low, mut on) = (vec![usize::MAX; n], vec![0usize; n], vec![false; n]);
    let (mut stack, mut next, mut out) = (Vec::new(), 0usize, Vec::new());
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut work = vec![(root, 0usize)];
        while let Some(&mut (v, ref mut ei)) = work.last_mut() {
            if *ei == 0 && index[v] == usize::MAX {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on[v] = true;
            }
            if let Some(&w) = succ[v].get(*ei) {
                *ei += 1;
                if index[w] == usize::MAX {
                    work.push((w, 0));
                } else if on[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut comp = Vec::new();
                while let Some(w) = stack.pop() {
                    on[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                if comp.len() > 1 || succ[v].contains(&v) {
                    comp.reverse();
                    out.push(comp);
                }
            }
        }
    }
    out
}

fn rtstat(view: &ImplView) -> Result<Vec<String>, String> {
    if view.nets.is_empty() {
        return Err(view.missing.get("routing").cloned().unwrap_or_else(|| "the run has no routed nets".into()));
    }
    Ok(view.nets.iter().filter(|n| n.kind == "unrouted").map(|n| format!("net {} is unrouted ({} load(s))", n.net, n.loads.len())).collect())
}

/// The input port a net comes from, following cells with one input pin
/// (IO and clock buffers) back from the net's driver.
fn source_port<'a>(syn: &'a SchematicNetlist, net: &str) -> Option<&'a str> {
    let nets: HashMap<&str, &crate::view::NetlistNet> = syn.nets.iter().map(|n| (n.name.as_str(), n)).collect();
    let cells: HashMap<&str, &crate::view::NetlistCell> = syn.cells.iter().map(|c| (c.name.as_str(), c)).collect();
    let mut cur = net.to_string();
    for _ in 0..16 {
        if let Some(p) = syn.ports.iter().find(|p| p.direction == "input" && p.nets.iter().any(|n| *n == cur)) {
            return Some(p.name.as_str());
        }
        let driver = nets.get(cur.as_str())?.drivers.first()?;
        let cell = cells.get(driver.split('/').next()?)?;
        let mut inputs = cell.pins.iter().filter(|p| !p.output);
        let (Some(only), None) = (inputs.next(), inputs.next()) else { return None };
        cur = only.net.clone();
    }
    None
}

fn clkpin(view: &ImplView) -> Result<Vec<String>, String> {
    let (io, syn) = (io(view)?, synthesized(view)?);
    let clocks = view.clocks.as_ref().ok_or_else(|| view.missing.get("clocks").cloned().unwrap_or_else(|| "the run has no clock nets".into()))?;
    if io.pins.iter().all(|p| p.clock_capable.is_none()) {
        return Err(view.missing.get("clock-capable").cloned().unwrap_or_else(|| "the device model does not say which pins are clock capable".into()));
    }
    let mut out = Vec::new();
    for c in &clocks.clocks {
        let Some(port) = source_port(syn, &c.name) else { continue };
        if let Some(pin) = io.pins.iter().find(|p| p.port == port && p.clock_capable == Some(false)) {
            out.push(format!("clock {} enters on port {port} at {}, which is not a clock-capable pin", c.name, pin.package_pin));
        }
    }
    Ok(out)
}

fn zps7(view: &ImplView) -> Result<Vec<String>, String> {
    if !view.part.to_ascii_lowercase().starts_with("xc7z") {
        return Err(format!("{} is not a Zynq-7000 part", view.part));
    }
    if view.cells.is_empty() {
        return Err("the run has no placement".into());
    }
    Ok(if view.cells.iter().any(|c| c.kind == "PS7") { Vec::new() } else { vec!["the design has no PS7 cell; the processing system keeps its power-on configuration".into()] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Family;
    use crate::view::{CellPlacement, ClockNet, ClockUtilization, IoPin, NetlistCell, NetlistNet, NetlistPin, NetlistPort, RoutedNet, Stage};

    fn pin(port: &str, package_pin: &str, bank: u32, std: &str) -> IoPin {
        IoPin { port: port.into(), package_pin: package_pin.into(), bank: Some(bank), iostandard: std.into(), direction: "input", iostandard_default: false, pin_default: false, clock_capable: Some(true) }
    }

    fn cell(name: &str, kind: &str, ins: &[&str], out: &str) -> NetlistCell {
        let mut pins: Vec<NetlistPin> = ins.iter().enumerate().map(|(i, n)| NetlistPin { name: format!("I{i}"), output: false, net: n.to_string() }).collect();
        pins.push(NetlistPin { name: "O".into(), output: true, net: out.into() });
        NetlistCell { name: name.into(), kind: kind.into(), pins, site: None, bel: None }
    }

    /// A clean view: clk port through IBUF and BUFG to a flop, one LUT.
    fn clean() -> ImplView {
        let mut v = ImplView::empty(Family::Xilinx7, "xc7z020clg484-1", Stage::Routed);
        v.io = Some(IoPlan { pins: vec![pin("clk", "Y9", 13, "LVCMOS33"), pin("d", "T22", 33, "LVCMOS33")], package_pins: 484 });
        let net = |name: &str, drivers: &[&str], loads: &[&str]| NetlistNet { name: name.into(), drivers: drivers.iter().map(|s| s.to_string()).collect(), loads: loads.iter().map(|s| s.to_string()).collect() };
        v.schematic.push(SchematicNetlist {
            stage: NetlistStage::Synthesized,
            top: "t".into(),
            ports: vec![
                NetlistPort { name: "clk".into(), direction: "input", nets: vec!["clk".into()] },
                NetlistPort { name: "d".into(), direction: "input", nets: vec!["d".into()] },
            ],
            cells: vec![cell("ib", "IBUF", &["clk"], "clk_i"), cell("bg", "BUFG", &["clk_i"], "clk_g"), cell("l0", "LUT2", &["d", "q"], "n0"), cell("ff", "FDRE", &["n0", "clk_g"], "q")],
            nets: vec![net("clk", &["clk"], &["ib/I0"]), net("clk_i", &["ib/O"], &["bg/I0"]), net("clk_g", &["bg/O"], &["ff/I1"]), net("d", &["d"], &["l0/I0"]), net("n0", &["l0/O"], &["ff/I0"]), net("q", &["ff/O"], &["l0/I1"])],
            instances: vec![],
        });
        v.cells.push(CellPlacement { cell: "ps".into(), site: "PS7_X0Y0".into(), bel: None, tile: None, kind: "PS7".into() });
        v.nets.push(RoutedNet { net: "n0".into(), driver: None, loads: vec!["ff.D".into()], hops: vec![], kind: "pips" });
        v.clocks = Some(ClockUtilization { clocks: vec![ClockNet { name: "clk_g".into(), period_ps: 10_000, loads: 1, regions: vec![], buffer: None }], loads_by_region: BTreeMap::new(), bufgs_used: 1, bufgs_available: 32 });
        v
    }

    fn run(v: &mut ImplView) -> Vec<(String, String)> {
        fill(v);
        v.drc.iter().map(|d| (d.rule.clone(), d.message.clone())).collect()
    }

    fn syn(v: &mut ImplView) -> &mut SchematicNetlist {
        &mut v.schematic[0]
    }

    #[test]
    fn a_clean_view_runs_every_rule_and_finds_nothing() {
        let mut v = clean();
        assert_eq!(run(&mut v), vec![]);
        assert_eq!(v.drc_rules.len(), RULES.len());
        assert!(v.drc_rules.iter().all(|r| r.not_run.is_none()), "{:?}", v.drc_rules);
        assert!(v.provenance["drc"].method.contains("LUTLP-1") && !v.provenance["drc"].method.contains("not run"));
    }

    #[test]
    fn nstd_flags_a_defaulted_standard_and_is_not_run_without_standards() {
        let mut v = clean();
        v.io.as_mut().unwrap().pins[1].iostandard_default = true;
        let d = run(&mut v);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].0 == "NSTD-1" && d[0].1.contains("default LVCMOS33: d"), "{d:?}");
        v.mark_missing("io-standard", "this flow programs no IO standard");
        assert_eq!(run(&mut v), vec![]);
        let rule = |v: &ImplView, id: &str| v.drc_rules.iter().find(|r| r.id == id).unwrap().not_run.clone();
        assert_eq!(rule(&v, "NSTD-1").as_deref(), Some("this flow programs no IO standard"));
        assert!(rule(&v, "BIVC-1").is_some(), "no standards, no VCCO check");
    }

    #[test]
    fn ucio_flags_a_port_without_a_pin_a_short_bus_and_a_pin_the_flow_chose() {
        let mut v = clean();
        syn(&mut v).ports.push(NetlistPort { name: "led".into(), direction: "output", nets: vec!["l0".into(), "l1".into()] });
        v.io.as_mut().unwrap().pins.push(pin("led[0]", "U14", 33, "LVCMOS33"));
        v.io.as_mut().unwrap().pins[1].pin_default = true;
        let d = run(&mut v);
        assert_eq!(d.iter().filter(|x| x.0 == "UCIO-1").count(), 2, "{d:?}");
        assert!(d.iter().any(|x| x.1 == "led: 1 of 2 bit(s) have a package pin"), "{d:?}");
        assert!(d.iter().any(|x| x.1.contains("d: 0 of 1") && x.1.contains("the flow chose T22")), "{d:?}");
    }

    #[test]
    fn bivc_flags_only_a_bank_that_mixes_levels() {
        let io = IoPlan {
            pins: vec![pin("a", "", 33, "LVCMOS33"), pin("b", "", 33, "LVCMOS18"), pin("c", "", 13, "LVCMOS33"), pin("d", "", 13, "LVTTL"), pin("e", "", 34, "LVCMOS33"), pin("f", "", 34, "DIFF_SSTL15")],
            package_pins: 0,
        };
        let m = bank_vcco(&io);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("bank 33") && m[0].contains("3.3 V: a") && m[0].contains("1.8 V: b"), "{}", m[0]);
        let mut v = clean();
        v.io.as_mut().unwrap().pins.push(pin("x", "U1", 13, "LVCMOS18"));
        syn(&mut v).ports.push(NetlistPort { name: "x".into(), direction: "input", nets: vec!["x".into()] });
        assert!(run(&mut v).iter().any(|d| d.0 == "BIVC-1" && d.1.contains("bank 13")));
        assert_eq!(v.drc.iter().find(|d| d.rule == "BIVC-1").unwrap().severity, "Error");
    }

    #[test]
    fn mdrv_and_ndrv_flag_two_drivers_and_none() {
        let mut v = clean();
        syn(&mut v).nets[4].drivers.push("ff/O".into());
        syn(&mut v).nets[3].drivers.clear();
        let d = run(&mut v);
        assert!(d.iter().any(|x| x.0 == "MDRV-1" && x.1.starts_with("net n0 has 2 drivers")), "{d:?}");
        assert!(d.iter().any(|x| x.0 == "HLN-NDRV" && x.1.starts_with("net d has no driver and 1 load")), "{d:?}");
        assert!(!v.drc_rules.iter().find(|r| r.id == "HLN-NDRV").unwrap().vivado, "a Helion rule does not borrow a Vivado id");
        // A net nothing reads is not an undriven-net violation.
        let mut v = clean();
        syn(&mut v).nets.push(NetlistNet { name: "spare".into(), drivers: vec![], loads: vec![] });
        assert_eq!(run(&mut v), vec![]);
    }

    #[test]
    fn lutlp_finds_a_loop_through_luts_and_not_one_through_a_flop() {
        // clean(): l0 -> ff -> l0 runs through the flop, so it is no loop.
        let mut v = clean();
        syn(&mut v).cells.push(cell("l1", "LUT1", &["n2"], "n1"));
        syn(&mut v).cells.push(cell("l2", "LUT1", &["n1"], "n2"));
        let d = run(&mut v);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].0 == "LUTLP-1" && d[0].1.contains("2 cell(s)") && d[0].1.contains("l1") && d[0].1.contains("l2"), "{d:?}");
        // A LUT that feeds itself is a loop of one.
        let mut v = clean();
        syn(&mut v).cells.push(cell("osc", "LUT1", &["x"], "x"));
        assert!(run(&mut v).iter().any(|x| x.0 == "LUTLP-1" && x.1.contains("1 cell(s): osc")));
        assert_eq!(cycles(&[vec![1], vec![2], vec![0], vec![0]]), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn rtstat_flags_an_unrouted_net_and_is_not_run_before_routing() {
        let mut v = clean();
        v.nets.push(RoutedNet { net: "lost".into(), driver: None, loads: vec!["a.I0".into(), "b.I0".into()], hops: vec![], kind: "unrouted" });
        assert_eq!(run(&mut v), vec![("RTSTAT-1".to_string(), "net lost is unrouted (2 load(s))".to_string())]);
        v.nets.clear();
        v.mark_missing("routing", "placed only");
        assert_eq!(run(&mut v), vec![]);
        assert_eq!(v.drc_rules.iter().find(|r| r.id == "RTSTAT-1").unwrap().not_run.as_deref(), Some("placed only"));
    }

    #[test]
    fn clkpin_traces_the_clock_through_its_buffers_to_the_pin() {
        let mut v = clean();
        assert_eq!(source_port(&v.schematic[0], "clk_g"), Some("clk"));
        assert_eq!(source_port(&v.schematic[0], "q"), None, "a flop output is not a pin");
        v.io.as_mut().unwrap().pins[0].clock_capable = Some(false);
        let d = run(&mut v);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].0 == "HLN-CLKPIN" && d[0].1.contains("port clk at Y9"), "{d:?}");
        for p in v.io.as_mut().unwrap().pins.iter_mut() {
            p.clock_capable = None;
        }
        v.mark_missing("clock-capable", "any pad can drive a clock line");
        assert_eq!(run(&mut v), vec![]);
        assert_eq!(v.drc_rules.iter().find(|r| r.id == "HLN-CLKPIN").unwrap().not_run.as_deref(), Some("any pad can drive a clock line"));
    }

    #[test]
    fn zps7_flags_a_zynq_design_without_the_ps7_and_skips_other_parts() {
        let mut v = clean();
        v.cells[0].kind = "LUT".into();
        let d = run(&mut v);
        assert!(d.len() == 1 && d[0].0 == "ZPS7-1", "{d:?}");
        v.part = "ice40-hx8k".into();
        assert_eq!(run(&mut v), vec![]);
        assert!(v.drc_rules.iter().find(|r| r.id == "ZPS7-1").unwrap().not_run.as_ref().unwrap().contains("not a Zynq-7000 part"));
    }
}
