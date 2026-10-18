//! Vivado UG907 `opt_design -PowerOpt` dynamic power optimization on [`Design`].
//!
//! Provides three netlist-level dynamic power transformations orchestrated by
//! [`optimize_dynamic_power`] (and [`Design::optimize_dynamic_power`]):
//! 1. **Feedback Mux $\to$ `FDRE` Clock-Enable Extraction** ([`extract_fdre_clock_enables`]):
//!    Detects `Lut6` cells driving `Hff.D` where one mux branch feeds back `Hff.Q`
//!    ($D = \text{en} ? D_{\text{new}} : Q$), wires `en` (or `en_old & en`) directly to `Hff.CE`,
//!    rewires `Hff.D = D_new`, and prunes the dead feedback LUT.
//! 2. **High-Fanout `BUFGCE` Clock Gating** ([`hoist_bufgce_clock_gates`]):
//!    When $\ge \text{min\_fanout}$ (e.g. 16) flip-flops on the same clock domain share an identical
//!    `CE` net, inserts a `BUFGCE` clock gate (`CellKind::ClockBuf` with `PRIMITIVE="BUFGCE"`) gating
//!    the clock net and ties the downstream flip-flops' `CE` pins to `1'b1`.
//! 3. **BRAM Idle Enable Gating** ([`gate_bram_idle_enables`]):
//!    Detects `Bram18` / `Bram18e1` / `Ram18` / `Ram36` instances with constant-high or unconnected
//!    `ENARDEN`/`ENBWREN` whose read data has no active downstream consumer condition or whose
//!    write-enable is inactive in `NO_CHANGE` mode, gating the enable net and reporting estimated
//!    dynamic power savings in mW ([`PowerOptReport`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{CellKind, Design, Endpoint, PortDir, Spur, intern, resolve};

/// Summary report returned by [`optimize_dynamic_power`] (`opt_design -PowerOpt`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerOptReport {
    /// Number of flip-flops whose `CE` pin was extracted from a self-feedback LUT mux.
    pub fdre_ce_extracted: usize,
    /// Number of dead feedback LUTs pruned after `FDRE` CE extraction.
    pub dead_luts_pruned: usize,
    /// Number of high-fanout `BUFGCE` clock-gate buffers inserted.
    pub bufgce_inserted: usize,
    /// Total number of flip-flops moved onto `BUFGCE`-gated clock nets.
    pub bufgce_ffs_gated: usize,
    /// Number of BRAM port enables (`ENARDEN` / `ENBWREN`) gated to suppress idle array precharge.
    pub bram_ports_gated: usize,
    /// Estimated total dynamic power savings in milliwatts (mW).
    pub estimated_saved_mw: f64,
    /// Human-readable audit log of optimization actions applied.
    pub actions: Vec<String>,
}

impl PowerOptReport {
    pub fn total_optimizations(&self) -> usize {
        self.fdre_ce_extracted + self.bufgce_inserted + self.bram_ports_gated
    }

    pub fn summary_text(&self) -> String {
        format!(
            "opt_design -PowerOpt: fdre_ce={} luts_pruned={} bufgce={} bufgce_ffs={} bram_ports_gated={} saved={:.3}mW",
            self.fdre_ce_extracted,
            self.dead_luts_pruned,
            self.bufgce_inserted,
            self.bufgce_ffs_gated,
            self.bram_ports_gated,
            self.estimated_saved_mw
        )
    }
}

fn is_const_one_net(name: &str) -> bool {
    matches!(
        name.trim(),
        "1'b1" | "1" | "VCC" | "vcc" | "const1" | "$true" | "true"
    )
}

fn is_const_zero_net(name: &str) -> bool {
    matches!(
        name.trim(),
        "1'b0" | "0" | "GND" | "gnd" | "const0" | "$false" | "false"
    )
}

/// Disconnect a specific `(cell, pin)` endpoint from whichever net currently carries it.
fn disconnect_pin(design: &mut Design, cell: Spur, pin: Spur) {
    for net in &mut design.nets {
        net.endpoints.retain(|e| !(e.cell == cell && e.pin == pin));
    }
    design.nets.retain(|n| !n.endpoints.is_empty());
    design.rebuild_indexes();
}

/// Check if a `Lut6` with 64-bit `init` and connected input map `inputs: &[(usize, Spur)]`
/// (where each entry is `(lut_input_idx_0_to_5, net_spur)`) implements the 2:1 feedback mux:
/// $$O = \text{en} ? d_{\text{new}} : Q$$
/// given `q_net`.
///
/// Returns `Some((en_net, d_new_net))` if an exact match is found.
fn match_lut6_feedback_mux(
    init: u64,
    inputs: &[(usize, Spur)],
    q_net: Spur,
) -> Option<(Spur, Spur)> {
    let &(q_pin, _) = inputs.iter().find(|&&(_, n)| n == q_net)?;
    for &(en_pin, en_net) in inputs {
        if en_pin == q_pin || en_net == q_net {
            continue;
        }
        for &(d_pin, d_net) in inputs {
            if d_pin == q_pin || d_pin == en_pin || d_net == q_net {
                continue;
            }
            let mut exact = true;
            for addr in 0..64u64 {
                let q_bit = (addr >> q_pin) & 1;
                let en_bit = (addr >> en_pin) & 1;
                let d_bit = (addr >> d_pin) & 1;
                let expected = if en_bit == 1 { d_bit } else { q_bit };
                let actual = (init >> addr) & 1;
                if actual != expected {
                    exact = false;
                    break;
                }
            }
            if exact {
                return Some((en_net, d_net));
            }
        }
    }
    None
}

/// Check if a `Lut6` with 64-bit `init` and connected inputs masks `data_net` whenever `en_net == 0`
/// (i.e., when `en_bit == 0`, flipping `data_bit` never changes the LUT output, and when `en_bit == 1`,
/// flipping `data_bit` changes the LUT output for at least one minterm).
fn match_lut6_downstream_mask(
    init: u64,
    inputs: &[(usize, Spur)],
    data_net: Spur,
) -> Option<Spur> {
    let &(data_pin, _) = inputs.iter().find(|&&(_, n)| n == data_net)?;
    let data_mask = 1u64 << data_pin;
    for &(en_pin, en_net) in inputs {
        if en_pin == data_pin || en_net == data_net {
            continue;
        }
        let en_mask = 1u64 << en_pin;
        let mut masked_when_zero = true;
        let mut active_when_one = false;
        for addr in 0..64u64 {
            if (addr & data_mask) != 0 {
                continue;
            }
            let f0 = (init >> addr) & 1;
            let f1 = (init >> (addr | data_mask)) & 1;
            let diff = f0 ^ f1;
            if (addr & en_mask) == 0 {
                if diff != 0 {
                    masked_when_zero = false;
                    break;
                }
            } else if diff != 0 {
                active_when_one = true;
            }
        }
        if masked_when_zero && active_when_one {
            return Some(en_net);
        }
    }
    None
}

/// Collect all `(lut_input_idx, net_spur)` pairs connected to `I0`..`I5` of `lut_cell`.
fn lut_connected_inputs(design: &Design, lut_cell: Spur) -> Vec<(usize, Spur)> {
    let pin_names = ["I0", "I1", "I2", "I3", "I4", "I5"];
    let mut out = Vec::new();
    for (idx, p_str) in pin_names.iter().enumerate() {
        if let Some(net) = design.net_on(lut_cell, intern(p_str)) {
            out.push((idx, net));
        }
    }
    out
}

/// Extract `FDRE` (`Hff`) clock-enable (`CE`) pins from self-feedback `Lut6` multiplexers
/// ($D = \text{en} ? D_{\text{new}} : Q$), rewire `Hff.D = D_new`, and prune dead feedback LUTs.
///
/// Returns the number of flip-flops optimized.
pub fn extract_fdre_clock_enables(design: &mut Design) -> usize {
    extract_fdre_clock_enables_with_log(design).0
}

fn extract_fdre_clock_enables_with_log(design: &mut Design) -> (usize, usize, Vec<String>) {
    design.rebuild_indexes();

    let ila_nets: HashSet<Spur> = design
        .cells
        .iter()
        .filter_map(|c| match &c.kind {
            CellKind::Ila { net } => Some(*net),
            _ => None,
        })
        .collect();
    let out_port_nets: HashSet<Spur> = design
        .ports
        .iter()
        .filter(|p| matches!(p.dir, PortDir::Out | PortDir::Inout))
        .map(|p| p.name)
        .collect();

    // Map net -> driving LUT cell name and init
    let mut net_lut_driver: HashMap<Spur, (Spur, u64)> = HashMap::new();
    for cell in &design.cells {
        if cell.attrs.flag("DONT_TOUCH") {
            continue;
        }
        if let CellKind::Lut6 { init } = cell.kind {
            if let Some(out_net) = design.net_on(cell.name, intern("O")) {
                net_lut_driver.insert(out_net, (cell.name, init));
            }
        }
    }

    #[derive(Clone, Debug)]
    struct PendingCeExtract {
        ff_cell: Spur,
        lut_cell: Spur,
        d_mux_net: Spur,
        en_net: Spur,
        d_new_net: Spur,
        old_ce_net: Option<Spur>,
    }

    let mut pending = Vec::new();
    for cell in &design.cells {
        if cell.attrs.flag("DONT_TOUCH") || !matches!(cell.kind, CellKind::Hff) {
            continue;
        }
        let Some(d_mux_net) = design.net_on(cell.name, intern("D")) else {
            continue;
        };
        let Some(q_net) = design.net_on(cell.name, intern("Q")) else {
            continue;
        };
        let Some(&(lut_cell, init)) = net_lut_driver.get(&d_mux_net) else {
            continue;
        };
        let inputs = lut_connected_inputs(design, lut_cell);
        if let Some((en_net, d_new_net)) = match_lut6_feedback_mux(init, &inputs, q_net) {
            let old_ce_net = design.net_on(cell.name, intern("CE"));
            pending.push(PendingCeExtract {
                ff_cell: cell.name,
                lut_cell,
                d_mux_net,
                en_net,
                d_new_net,
                old_ce_net,
            });
        }
    }

    let mut extracted_count = 0usize;
    let mut actions = Vec::new();
    let mut candidate_dead_luts = Vec::new();

    for (k, item) in pending.into_iter().enumerate() {
        let ff_name = resolve(item.ff_cell).to_string();
        let lut_name = resolve(item.lut_cell).to_string();
        let en_name = resolve(item.en_net).to_string();
        let d_new_name = resolve(item.d_new_net).to_string();

        // 1. Rewire Hff.D from d_mux_net to d_new_net
        disconnect_pin(design, item.ff_cell, intern("D"));
        design.connect(&d_new_name, &ff_name, "D");

        // 2. Wire Hff.CE to en_net (or old_ce_net & en_net if an active CE already existed)
        let target_ce_net = match item.old_ce_net {
            None => en_name.clone(),
            Some(old_ce) => {
                let old_ce_str = resolve(old_ce);
                if is_const_one_net(old_ce_str) || old_ce == item.en_net {
                    disconnect_pin(design, item.ff_cell, intern("CE"));
                    en_name.clone()
                } else {
                    disconnect_pin(design, item.ff_cell, intern("CE"));
                    let and_cell_name = format!("{ff_name}_ce_and_{k}");
                    let and_net_name = format!("{ff_name}_ce_comb_{k}");
                    // 2-input AND on I0, I1 replicated across I2..I5: 0x8888_8888_8888_8888
                    design.add_cell(
                        &and_cell_name,
                        CellKind::Lut6 {
                            init: 0x8888_8888_8888_8888,
                        },
                    );
                    design.connect(old_ce_str, &and_cell_name, "I0");
                    design.connect(&en_name, &and_cell_name, "I1");
                    design.connect(&and_net_name, &and_cell_name, "O");
                    and_net_name
                }
            }
        };
        design.connect(&target_ce_net, &ff_name, "CE");
        candidate_dead_luts.push((item.lut_cell, item.d_mux_net));
        extracted_count += 1;
        actions.push(format!(
            "extract_fdre_ce: {} D<-{} CE<-{} (from feedback LUT {})",
            ff_name, d_new_name, target_ce_net, lut_name
        ));
    }

    // 3. Prune dead feedback LUTs whose output net now has only the LUT's own "O" endpoint
    let mut dead_luts: HashSet<Spur> = HashSet::new();
    for (lut_cell, out_net) in candidate_dead_luts {
        if ila_nets.contains(&out_net) || out_port_nets.contains(&out_net) {
            continue;
        }
        let is_dead = design
            .net(resolve(out_net))
            .map(|n| {
                !n.attrs.flag("mark_debug")
                    && n.endpoints
                        .iter()
                        .all(|e| e.cell == lut_cell && e.pin == intern("O"))
            })
            .unwrap_or(true);
        if is_dead {
            dead_luts.insert(lut_cell);
        }
    }

    let pruned_count = dead_luts.len();
    if !dead_luts.is_empty() {
        for net in &mut design.nets {
            net.endpoints.retain(|e| !dead_luts.contains(&e.cell));
        }
        design.nets.retain(|n| !n.endpoints.is_empty());
        design.cells.retain(|c| !dead_luts.contains(&c.name));
        design.rebuild_indexes();
    }

    (extracted_count, pruned_count, actions)
}

/// Hoist shared high-fanout flip-flop clock enables into global `BUFGCE` clock gates.
///
/// Whenever $\ge \text{min\_fanout}$ `Hff` cells on the same clock net share an identical active
/// `CE` net, inserts a `BUFGCE` cell (`CellKind::ClockBuf` with `PRIMITIVE="BUFGCE"`) gating the
/// clock net and ties the downstream flip-flops' `CE` pins to `1'b1`.
///
/// Returns the number of `BUFGCE` cells inserted.
pub fn hoist_bufgce_clock_gates(design: &mut Design, min_fanout: usize) -> usize {
    hoist_bufgce_clock_gates_with_log(design, min_fanout).0
}

fn hoist_bufgce_clock_gates_with_log(
    design: &mut Design,
    min_fanout: usize,
) -> (usize, usize, Vec<String>) {
    design.rebuild_indexes();
    let threshold = min_fanout.max(1);

    // Group Hff cells by (clk_net, ce_net)
    let mut groups: BTreeMap<(String, String), Vec<(Spur, &'static str)>> = BTreeMap::new();
    for cell in &design.cells {
        if cell.attrs.flag("DONT_TOUCH") || !matches!(cell.kind, CellKind::Hff) {
            continue;
        }
        let (clk_net, clk_pin) = if let Some(n) = design.net_on(cell.name, intern("CLK")) {
            (n, "CLK")
        } else if let Some(n) = design.net_on(cell.name, intern("C")) {
            (n, "C")
        } else {
            continue;
        };
        let Some(ce_net) = design.net_on(cell.name, intern("CE")) else {
            continue;
        };
        let ce_str = resolve(ce_net);
        if is_const_one_net(ce_str) || is_const_zero_net(ce_str) {
            continue;
        }
        let clk_str = resolve(clk_net).to_string();
        groups
            .entry((clk_str, ce_str.to_string()))
            .or_default()
            .push((cell.name, clk_pin));
    }

    let mut bufgce_inserted = 0usize;
    let mut total_ffs_gated = 0usize;
    let mut actions = Vec::new();

    for ((clk_name, ce_name), ffs) in groups {
        if ffs.len() < threshold {
            continue;
        }
        let gate_idx = bufgce_inserted;
        let bufgce_cell_name = format!("u_bufgce_{clk_name}_{ce_name}_{gate_idx}");
        let gated_clk_net = format!("{clk_name}_gated_{ce_name}_{gate_idx}");

        design.add_cell(&bufgce_cell_name, CellKind::ClockBuf);
        let _ = design.set_cell_attr(&bufgce_cell_name, "PRIMITIVE", "BUFGCE");
        let _ = design.set_cell_attr(&bufgce_cell_name, "CELL_TYPE", "BUFGCE");
        design.connect(&clk_name, &bufgce_cell_name, "I");
        design.connect(&ce_name, &bufgce_cell_name, "CE");
        design.connect(&gated_clk_net, &bufgce_cell_name, "O");

        for &(ff_spur, clk_pin) in &ffs {
            let ff_name = resolve(ff_spur).to_string();
            disconnect_pin(design, ff_spur, intern(clk_pin));
            disconnect_pin(design, ff_spur, intern("CE"));
            design.connect(&gated_clk_net, &ff_name, clk_pin);
            design.connect("1'b1", &ff_name, "CE");
        }

        bufgce_inserted += 1;
        total_ffs_gated += ffs.len();
        actions.push(format!(
            "hoist_bufgce: inserted {} (I={}, CE={}, O={}) gating {} FFs",
            bufgce_cell_name,
            clk_name,
            ce_name,
            gated_clk_net,
            ffs.len()
        ));
    }

    (bufgce_inserted, total_ffs_gated, actions)
}

fn is_bram_cell(kind: &CellKind) -> bool {
    match kind {
        CellKind::Bram18 | CellKind::Bram18e1 { .. } => true,
        CellKind::BlackBox { module } => {
            let m = resolve(*module);
            matches!(
                m,
                "RAMB18E1" | "RAMB36E1" | "RAMB18E2" | "RAMB36E2" | "Ram18" | "Ram36"
            )
        }
        _ => false,
    }
}

/// Gate idle `Bram18` / `Bram18e1` / `Ram18` / `Ram36` port enables (`ENARDEN`, `ENBWREN`).
///
/// Detects BRAM ports whose enable is unconnected or tied high (`1'b1`) and:
/// 1. When read data outputs have no active downstream consumers (or the port is in `NO_CHANGE` write mode)
///    and a write-enable (`WEA` / `WEBWE`) is present, gates the port enable with the write-enable net.
/// 2. When read data outputs only feed downstream `Lut6` logic that masks them with a common enable
///    signal `read_en`, gates the port enable with `read_en` (or ties to `1'b0` if both read and write
///    ports are completely unused).
///
/// Returns the number of BRAM port enables gated.
pub fn gate_bram_idle_enables(design: &mut Design) -> usize {
    gate_bram_idle_enables_with_log(design).0
}

fn gate_bram_idle_enables_with_log(design: &mut Design) -> (usize, Vec<String>) {
    design.rebuild_indexes();

    // Precompute LUT inputs for downstream read-mask detection
    let mut lut_info: HashMap<Spur, (u64, Vec<(usize, Spur)>)> = HashMap::new();
    for cell in &design.cells {
        if let CellKind::Lut6 { init } = cell.kind {
            lut_info.insert(cell.name, (init, lut_connected_inputs(design, cell.name)));
        }
    }

    let bram_cells: Vec<Spur> = design
        .cells
        .iter()
        .filter(|c| !c.attrs.flag("DONT_TOUCH") && is_bram_cell(&c.kind))
        .map(|c| c.name)
        .collect();

    let mut gated_ports = 0usize;
    let mut actions = Vec::new();

    for bram_spur in bram_cells {
        let bram_name = resolve(bram_spur).to_string();
        let no_change_mode = design
            .cell(&bram_name)
            .and_then(|c| {
                c.attrs
                    .get("WRITE_MODE")
                    .or_else(|| c.attrs.get("WRITE_MODE_A"))
                    .or_else(|| c.attrs.get("WRITE_MODE_B"))
            })
            .map(|m| m.eq_ignore_ascii_case("NO_CHANGE"))
            .unwrap_or(false);

        for (en_pin, we_pins, do_prefix) in [
            (
                "ENARDEN",
                &["WEA", "WEA[0]", "WEA[1]", "WE"][..],
                &["DOADO", "DOA", "DO"][..],
            ),
            (
                "ENBWREN",
                &["WEBWE", "WEBWE[0]", "WEBWE[1]", "WEB"][..],
                &["DOBDO", "DOB"][..],
            ),
        ] {
            let cur_en = design.net_on(bram_spur, intern(en_pin));
            let is_always_on = match cur_en {
                None => true,
                Some(n) => is_const_one_net(resolve(n)),
            };
            if !is_always_on {
                continue;
            }

            // Find write enable net on this port, if any
            let we_net: Option<Spur> = we_pins
                .iter()
                .find_map(|p| design.net_on(bram_spur, intern(p)))
                .filter(|&n| !is_const_one_net(resolve(n)) && !is_const_zero_net(resolve(n)));

            // Find all read output nets driven by this BRAM port
            let mut do_nets: Vec<Spur> = Vec::new();
            for net in &design.nets {
                for ep in &net.endpoints {
                    if ep.cell == bram_spur {
                        let pin_str = resolve(ep.pin);
                        if do_prefix.iter().any(|pref| pin_str.starts_with(pref)) {
                            do_nets.push(net.name);
                        }
                    }
                }
            }
            do_nets.sort_by_key(|s| resolve(*s));
            do_nets.dedup();

            // Inspect downstream consumers of `do_nets`
            let mut total_consumers = 0usize;
            let mut common_mask_en: Option<Spur> = None;
            let mut all_consumers_masked = true;

            for &d_net in &do_nets {
                if let Some(net_obj) = design.net(resolve(d_net)) {
                    for ep in &net_obj.endpoints {
                        if ep.cell == bram_spur {
                            continue;
                        }
                        total_consumers += 1;
                        if let Some((init, inputs)) = lut_info.get(&ep.cell) {
                            if let Some(mask_en) = match_lut6_downstream_mask(*init, inputs, d_net)
                            {
                                match common_mask_en {
                                    None => common_mask_en = Some(mask_en),
                                    Some(prev) if prev == mask_en => {}
                                    _ => all_consumers_masked = false,
                                }
                            } else {
                                all_consumers_masked = false;
                            }
                        } else {
                            all_consumers_masked = false;
                        }
                    }
                }
            }

            // Determine gating net for this BRAM port
            let chosen_gate: Option<String> = if total_consumers == 0 {
                if let Some(we) = we_net {
                    Some(resolve(we).to_string())
                } else if cur_en.is_some() {
                    Some("1'b0".to_string())
                } else {
                    None
                }
            } else if all_consumers_masked && common_mask_en.is_some() {
                let rd_en = common_mask_en.unwrap();
                if let Some(we) = we_net {
                    if we == rd_en {
                        Some(resolve(rd_en).to_string())
                    } else {
                        // Combine read_en | we_net via 2-input OR Lut6 (init = 0xEEEE_EEEE_EEEE_EEEE)
                        let or_cell = format!("{bram_name}_{en_pin}_or");
                        let or_net = format!("{bram_name}_{en_pin}_en");
                        design.add_cell(
                            &or_cell,
                            CellKind::Lut6 {
                                init: 0xEEEE_EEEE_EEEE_EEEE,
                            },
                        );
                        design.connect(resolve(rd_en), &or_cell, "I0");
                        design.connect(resolve(we), &or_cell, "I1");
                        design.connect(&or_net, &or_cell, "O");
                        Some(or_net)
                    }
                } else {
                    Some(resolve(rd_en).to_string())
                }
            } else if no_change_mode && we_net.is_some() {
                Some(resolve(we_net.unwrap()).to_string())
            } else {
                None
            };

            if let Some(gate_net_name) = chosen_gate {
                disconnect_pin(design, bram_spur, intern(en_pin));
                design.connect(&gate_net_name, &bram_name, en_pin);
                let _ = design.set_cell_attr(&bram_name, "WRITE_MODE", "NO_CHANGE");
                gated_ports += 1;
                actions.push(format!(
                    "gate_bram_en: {} {}<-{}",
                    bram_name, en_pin, gate_net_name
                ));
            }
        }
    }

    (gated_ports, actions)
}

/// Run the full Vivado UG907 `opt_design -PowerOpt` dynamic power optimization pipeline on `design`:
/// 1. [`extract_fdre_clock_enables`]
/// 2. [`hoist_bufgce_clock_gates`]
/// 3. [`gate_bram_idle_enables`]
pub fn optimize_dynamic_power(design: &mut Design, min_bufgce_fanout: usize) -> PowerOptReport {
    let (fdre_ce_extracted, dead_luts_pruned, mut actions) =
        extract_fdre_clock_enables_with_log(design);
    let (bufgce_inserted, bufgce_ffs_gated, bufg_actions) =
        hoist_bufgce_clock_gates_with_log(design, min_bufgce_fanout);
    let (bram_ports_gated, bram_actions) = gate_bram_idle_enables_with_log(design);

    actions.extend(bufg_actions);
    actions.extend(bram_actions);

    // Modeled 28nm dynamic power savings (UG907):
    // - Each FDRE feedback-mux CE extraction + LUT removal saves ~0.085 mW
    // - Each BUFGCE leaf-clock tree branch gated saves ~0.045 mW per FF + 0.35 mW tree buffer
    // - Each BRAM18/36 idle port enable gated saves ~2.40 mW of bitline precharge power
    let estimated_saved_mw = (fdre_ce_extracted as f64) * 0.085
        + (dead_luts_pruned as f64) * 0.025
        + (bufgce_inserted as f64) * 0.35
        + (bufgce_ffs_gated as f64) * 0.045
        + (bram_ports_gated as f64) * 2.40;

    PowerOptReport {
        fdre_ce_extracted,
        dead_luts_pruned,
        bufgce_inserted,
        bufgce_ffs_gated,
        bram_ports_gated,
        estimated_saved_mw,
        actions,
    }
}

impl Design {
    /// Extract `FDRE` (`Hff`) clock-enable (`CE`) pins from self-feedback `Lut6` multiplexers.
    pub fn extract_fdre_clock_enables(&mut self) -> usize {
        extract_fdre_clock_enables(self)
    }

    /// Hoist shared high-fanout flip-flop `CE` nets into `BUFGCE` global clock gates.
    pub fn hoist_bufgce_clock_gates(&mut self, min_fanout: usize) -> usize {
        hoist_bufgce_clock_gates(self, min_fanout)
    }

    /// Gate constant-high `Bram18` / `Bram18e1` `ENARDEN` and `ENBWREN` enables on idle cycles.
    pub fn gate_bram_idle_enables(&mut self) -> usize {
        gate_bram_idle_enables(self)
    }

    /// Run all three UG907 `opt_design -PowerOpt` dynamic power optimizations on this design.
    pub fn optimize_dynamic_power(&mut self, min_bufgce_fanout: usize) -> PowerOptReport {
        optimize_dynamic_power(self, min_bufgce_fanout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compute 64-bit LUT6 INIT for `I2 ? I1 : I0` (where I0=Q, I1=d_new, I2=en).
    fn feedback_mux_init_i0_q_i1_d_i2_en() -> u64 {
        let mut init = 0u64;
        for addr in 0..64u64 {
            let q = addr & 1;
            let d = (addr >> 1) & 1;
            let en = (addr >> 2) & 1;
            let out = if en == 1 { d } else { q };
            init |= out << addr;
        }
        init
    }

    #[test]
    fn extract_fdre_ce_rewires_d_and_ce_and_prunes_feedback_lut() {
        let mut d = Design::new("fb_ce_test");
        d.add_port("clk", PortDir::In);
        d.add_port("en", PortDir::In);
        d.add_port("din", PortDir::In);
        d.add_port("qout", PortDir::Out);

        d.add_cell(
            "u_fb_lut",
            CellKind::Lut6 {
                init: feedback_mux_init_i0_q_i1_d_i2_en(),
            },
        );
        d.add_cell("u_ff", CellKind::Hff);

        d.connect("qout", "u_fb_lut", "I0");
        d.connect("din", "u_fb_lut", "I1");
        d.connect("en", "u_fb_lut", "I2");
        d.connect("d_mux", "u_fb_lut", "O");

        d.connect("clk", "u_ff", "CLK");
        d.connect("d_mux", "u_ff", "D");
        d.connect("qout", "u_ff", "Q");

        let count = extract_fdre_clock_enables(&mut d);
        assert_eq!(count, 1);
        assert!(
            d.cell("u_fb_lut").is_none(),
            "dead feedback LUT must be pruned after CE extraction"
        );
        assert_eq!(
            resolve(d.net_on(intern("u_ff"), intern("D")).unwrap()),
            "din"
        );
        assert_eq!(
            resolve(d.net_on(intern("u_ff"), intern("CE")).unwrap()),
            "en"
        );
    }

    #[test]
    fn hoist_bufgce_gates_shared_high_fanout_enable_domain() {
        let mut d = Design::new("bufgce_test");
        let init = feedback_mux_init_i0_q_i1_d_i2_en();
        for i in 0..16 {
            let lut = format!("u_lut{i}");
            let ff = format!("u_ff{i}");
            let din = format!("din{i}");
            let dmux = format!("dmux{i}");
            let q = format!("q{i}");
            d.add_cell(&lut, CellKind::Lut6 { init });
            d.add_cell(&ff, CellKind::Hff);
            d.connect(&q, &lut, "I0");
            d.connect(&din, &lut, "I1");
            d.connect("shared_en", &lut, "I2");
            d.connect(&dmux, &lut, "O");

            d.connect("sys_clk", &ff, "CLK");
            d.connect(&dmux, &ff, "D");
            d.connect(&q, &ff, "Q");
        }

        let rep = optimize_dynamic_power(&mut d, 16);
        assert_eq!(rep.fdre_ce_extracted, 16);
        assert_eq!(rep.dead_luts_pruned, 16);
        assert_eq!(rep.bufgce_inserted, 1);
        assert_eq!(rep.bufgce_ffs_gated, 16);
        assert!(rep.estimated_saved_mw > 1.0);

        // Verify the BUFGCE cell exists and all 16 FFs now have CE tied to 1'b1
        let bufg = d
            .cells
            .iter()
            .find(|c| matches!(c.kind, CellKind::ClockBuf))
            .expect("inserted BUFGCE ClockBuf");
        assert_eq!(bufg.attrs.get("PRIMITIVE"), Some("BUFGCE"));
        for i in 0..16 {
            let ff = format!("u_ff{i}");
            assert_eq!(
                resolve(d.net_on(intern(&ff), intern("CE")).unwrap()),
                "1'b1"
            );
        }
    }

    #[test]
    fn gate_bram_idle_enables_gates_write_only_and_masked_read_ports() {
        let mut d = Design::new("bram_gate_test");
        d.add_cell(
            "u_ram",
            CellKind::Bram18e1 {
                read_width_a: 18,
                read_width_b: 18,
                write_width_a: 18,
                write_width_b: 18,
            },
        );
        // Port A: constant-high ENARDEN, read output DOADO[0] masked downstream by AND LUT with `rd_valid`
        d.connect("1'b1", "u_ram", "ENARDEN");
        d.connect("bram_do0", "u_ram", "DOADO[0]");
        d.add_cell(
            "u_rd_mask",
            CellKind::Lut6 {
                init: 0x8888_8888_8888_8888, // I0 & I1
            },
        );
        d.connect("bram_do0", "u_rd_mask", "I0");
        d.connect("rd_valid", "u_rd_mask", "I1");
        d.connect("masked_out", "u_rd_mask", "O");

        // Port B: constant-high ENBWREN, write-only port with WEBWE[0] = `wr_en` and no read loads
        d.connect("1'b1", "u_ram", "ENBWREN");
        d.connect("wr_en", "u_ram", "WEBWE[0]");

        let gated = gate_bram_idle_enables(&mut d);
        assert_eq!(gated, 2);
        assert_eq!(
            resolve(d.net_on(intern("u_ram"), intern("ENARDEN")).unwrap()),
            "rd_valid"
        );
        assert_eq!(
            resolve(d.net_on(intern("u_ram"), intern("ENBWREN")).unwrap()),
            "wr_en"
        );
        assert_eq!(
            d.cell("u_ram").unwrap().attrs.get("WRITE_MODE"),
            Some("NO_CHANGE")
        );
    }
}
