//! Pack logical cells into Helion site primitives (LUTFF + IOB + MAC27 + BRAM18 + ILA).

use helion_device::{Device, Site};
use helion_ir::{intern, resolve, CellKind, Design, Spur};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Packed {
    pub lutffs: Vec<PackedLutFf>,
    pub iobs: Vec<PackedIob>,
    pub macs: Vec<PackedMac>,
    pub brams: Vec<PackedBram>,
}

#[derive(Clone, Debug)]
pub struct PackedBram {
    pub cell: String,
    /// INIT words from IR attr `INIT` (comma-separated hex).
    pub init: Vec<u64>,
}

#[derive(Clone, Debug)]
pub struct PackedLutFf {
    pub lut_cell: String,
    pub ff_cell: String,
    pub init: u64,
    /// LUT pin → FF cell whose Q drives it (local cluster).
    pub lut_pins: Vec<(u8, String)>,
    /// Q net of the packed FF (IOB matching).
    pub q_net: String,
}

#[derive(Clone, Debug)]
pub struct PackedIob {
    pub cell: String,
    pub from_net: String,
    /// Optional `IOB_XxYy` from IR port LOC.
    pub loc: Option<String>,
    /// UG893 I/O Ports `DRIVE` (mA), copied from the PAD port.
    pub drive: Option<String>,
    /// UG893 I/O Ports `SLEW`.
    pub slew: Option<String>,
    /// UG893 I/O Ports `PULLTYPE`.
    pub pulltype: Option<String>,
    /// UG893 I/O Ports `DIFF_TERM`.
    pub diff_term: Option<String>,
    /// UG893 I/O Ports `IN_TERM`.
    pub in_term: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PackedMac {
    pub cell: String,
}

/// LUTFF occupancy per HAD CLB site ID (`CLB_X2Y1` → N). Derived from
/// placement sites, not a decorative pie.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackingSummary {
    pub sites: BTreeMap<String, usize>,
}

impl PackingSummary {
    /// Group placed LUTFF clusters by [`Site::id`].
    pub fn from_lutff_sites(sites: &[(Site, u8)]) -> Self {
        let mut m = BTreeMap::new();
        for (site, _) in sites {
            *m.entry(site.id()).or_insert(0) += 1;
        }
        Self { sites: m }
    }

    pub fn lutff_in(&self, site_id: &str) -> usize {
        self.sites.get(site_id).copied().unwrap_or(0)
    }
}

pub fn pack(design: &Design, _dev: &Device) -> Result<Packed, String> {
    // Multi-backend cell kinds never reach the HL10T packer: reject loudly
    // instead of silently dropping them (which would miscompile).
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobIn) {
            return Err(format!(
                "cell {} is IobIn (input IOB): unsupported for HL10T pack (multi-backend flow only)",
                resolve(c.name)
            ));
        }
        if matches!(c.kind, CellKind::ClockBuf) {
            return Err(format!(
                "cell {} is ClockBuf (global clock buffer): unsupported for HL10T pack (multi-backend flow only)",
                resolve(c.name)
            ));
        }
    }
    // Ibex-scale: index (cell, pin) -> net by Spur directly without resolve/intern churn.
    let total_eps: usize = design.nets.iter().map(|n| n.endpoints.len()).sum();
    let mut pin_to_net: HashMap<(Spur, Spur), Spur> = HashMap::with_capacity(total_eps);
    for n in &design.nets {
        for e in &n.endpoints {
            pin_to_net.insert((e.cell, e.pin), n.name);
        }
    }
    let pin_d = intern("D");
    let pin_q = intern("Q");
    let pin_o = intern("O");
    let pin_i = intern("I");
    let pin_pad = intern("PAD");
    let ipins: [Spur; 6] = [
        intern("I0"),
        intern("I1"),
        intern("I2"),
        intern("I3"),
        intern("I4"),
        intern("I5"),
    ];
    let mut d_driver: HashMap<Spur, Spur> = HashMap::new();
    let mut q_driver: HashMap<Spur, Spur> = HashMap::new();
    let mut ff_q_net: HashMap<Spur, Spur> = HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Hff) {
            if let Some(&d) = pin_to_net.get(&(c.name, pin_d)) {
                d_driver.entry(d).or_insert(c.name);
            }
            if let Some(&q) = pin_to_net.get(&(c.name, pin_q)) {
                q_driver.insert(q, c.name);
                ff_q_net.insert(c.name, q);
            }
        }
    }
    let mut used_ff = HashSet::<Spur>::with_capacity(d_driver.len());
    let mut lut_jobs: Vec<(Spur, u64, Spur, Option<Spur>)> = Vec::new();
    for c in &design.cells {
        let CellKind::Lut6 { init } = c.kind else {
            continue;
        };
        let Some(&o_net) = pin_to_net.get(&(c.name, pin_o)) else {
            continue;
        };
        // Comb LUTs (no FF on O) pack as LUT-only: empty ff_cell, q_net = O so
        // IOB/route match the LUT output. Registered LUTs keep the FF cluster.
        let ff_name = d_driver
            .get(&o_net)
            .copied()
            .filter(|n| used_ff.insert(*n));
        lut_jobs.push((c.name, init, o_net, ff_name));
    }
    let build_lutff = |&(lut_name, init, o_net, ff_name): &(Spur, u64, Spur, Option<Spur>)| {
        let mut lut_pins = Vec::new();
        for (pin, &key) in ipins.iter().enumerate() {
            if let Some(&net) = pin_to_net.get(&(lut_name, key)) {
                if let Some(&src) = q_driver.get(&net) {
                    lut_pins.push((pin as u8, resolve(src).to_string()));
                }
            }
        }
        let (ff_cell, q_net) = if let Some(n) = ff_name {
            (
                resolve(n).to_string(),
                ff_q_net.get(&n).copied().map(resolve).unwrap_or("").to_string(),
            )
        } else {
            (String::new(), resolve(o_net).to_string())
        };
        PackedLutFf {
            lut_cell: resolve(lut_name).to_string(),
            ff_cell,
            init,
            lut_pins,
            q_net,
        }
    };
    let lutffs: Vec<PackedLutFf> = if lut_jobs.len() >= 512 {
        lut_jobs.par_iter().map(build_lutff).collect()
    } else {
        lut_jobs.iter().map(build_lutff).collect()
    };
    let port_by_name: HashMap<Spur, &helion_ir::Port> =
        design.ports.iter().map(|p| (p.name, p)).collect();
    let empty_spur = intern("");
    let mut iobs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut) {
            let net = pin_to_net
                .get(&(c.name, pin_i))
                .copied()
                .ok_or_else(|| format!("IOB {} has no I net", resolve(c.name)))?;
            let pad = pin_to_net
                .get(&(c.name, pin_pad))
                .copied()
                .unwrap_or(empty_spur);
            let port = port_by_name.get(&pad).copied();
            let loc = port.and_then(|p| p.attrs.get("LOC").map(|s| s.to_string()));
            iobs.push(PackedIob {
                cell: resolve(c.name).to_string(),
                from_net: resolve(net).to_string(),
                loc,
                drive: port.and_then(|p| p.attrs.get("DRIVE").map(|s| s.to_string())),
                slew: port.and_then(|p| p.attrs.get("SLEW").map(|s| s.to_string())),
                pulltype: port.and_then(|p| p.attrs.get("PULLTYPE").map(|s| s.to_string())),
                diff_term: port.and_then(|p| p.attrs.get("DIFF_TERM").map(|s| s.to_string())),
                in_term: port.and_then(|p| p.attrs.get("IN_TERM").map(|s| s.to_string())),
            });
        }
    }
    let mut macs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }) {
            macs.push(PackedMac {
                cell: resolve(c.name).to_string(),
            });
        }
    }
    let mut brams = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Bram18 | CellKind::Bram18e1 { .. }) {
            let init = c
                .attrs
                .get("INIT")
                .unwrap_or("")
                .split(',')
                .filter(|t| !t.is_empty())
                .map(|t| u64::from_str_radix(t.trim(), 16).unwrap_or(0))
                .collect();
            brams.push(PackedBram {
                cell: resolve(c.name).to_string(),
                init,
            });
        }
    }
    Ok(Packed {
        lutffs,
        iobs,
        macs,
        brams,
    })
}

/// Copy HNF port DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM onto packed IOBs
/// (post-pack set_property).
pub fn apply_iob_electrical(design: &Design, iobs: &mut [PackedIob]) {
    let pins = design.pin_index();
    let ports: std::collections::HashMap<&str, &helion_ir::Port> =
        design.ports.iter().map(|p| (resolve(p.name), p)).collect();
    for iob in iobs.iter_mut() {
        let pad = pins.net_on(&iob.cell, "PAD").unwrap_or(intern(""));
        if let Some(p) = ports.get(resolve(pad)) {
            iob.drive = p.attrs.get("DRIVE").map(|s| s.to_string());
            iob.slew = p.attrs.get("SLEW").map(|s| s.to_string());
            iob.pulltype = p.attrs.get("PULLTYPE").map(|s| s.to_string());
            iob.diff_term = p.attrs.get("DIFF_TERM").map(|s| s.to_string());
            iob.in_term = p.attrs.get("IN_TERM").map(|s| s.to_string());
        }
    }
}

// ============================================================================
// Vivado UG953 SLICE_X7 Architecture Packing:
// 1. Dual-Output `LUT6_2` (`O5` + `O6`) Fracture Packer (`pack_lut6_2_pairs`)
// 2. `CARRY4` + Prop/Gen `LUT6` + `Hff` Co-Packer (`pack_carry4_chains`)
// 3. `FDRE` SLICE Control-Set (`CLK`/`CE`/`SR`) Legalizer (`validate_slice_control_sets`)
// ============================================================================

/// Control-set signature (`CLK`, `CE`, `SR`) of a single `Hff` (`FDRE`/`FDSE`/`FDCE`/`FDPE`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FfControlSet {
    pub clk: String,
    pub ce: Option<String>,
    pub sr: Option<String>,
}

/// Validated UG953 8-FF SLICE control set:
/// - Single shared `CLK` across all FFs in the SLICE
/// - Single shared `SR` (`SR`/`RST`) across all FFs in the SLICE
/// - At most 2 distinct `CE` signals partitioned across the lower 4-FF half (`AFF..B5FF`)
///   and upper 4-FF half (`CFF..D5FF`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceControlSet {
    pub clk: String,
    pub sr: Option<String>,
    pub lower_ce: Option<String>,
    pub upper_ce: Option<String>,
    pub lower_ffs: Vec<String>,
    pub upper_ffs: Vec<String>,
}

/// Reason why a candidate set of flip-flops violates UG953 SLICE control-set rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlSetViolation {
    /// More than 8 flip-flops assigned to a single SLICE.
    TooManyFfs { count: usize },
    /// Flip-flop cell was not found or is not `CellKind::Hff`.
    UnknownFf { cell: String },
    /// Flip-flop is missing a `CLK` connection.
    MissingClk { cell: String },
    /// Multiple distinct `CLK` nets in the same SLICE.
    ConflictingClk { clk_a: String, clk_b: String },
    /// Multiple distinct `SR`/`RST` nets (or mixed reset/resetless FFs) in the same SLICE.
    ConflictingSr {
        sr_a: Option<String>,
        sr_b: Option<String>,
    },
    /// More than 2 distinct `CE` nets in the same SLICE, or >4 FFs on one `CE` when 2 `CE`s are used.
    IncompatibleCe { distinct_ce_count: usize, detail: String },
}

impl std::fmt::Display for ControlSetViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyFfs { count } => {
                write!(f, "SLICE control-set violation: {count} FFs exceeds 8-FF SLICE capacity")
            }
            Self::UnknownFf { cell } => {
                write!(f, "SLICE control-set violation: unknown FF cell '{cell}'")
            }
            Self::MissingClk { cell } => {
                write!(f, "SLICE control-set violation: FF '{cell}' has no CLK net")
            }
            Self::ConflictingClk { clk_a, clk_b } => write!(
                f,
                "SLICE control-set violation: conflicting CLK nets '{clk_a}' vs '{clk_b}'"
            ),
            Self::ConflictingSr { sr_a, sr_b } => write!(
                f,
                "SLICE control-set violation: conflicting SR nets {:?} vs {:?}",
                sr_a, sr_b
            ),
            Self::IncompatibleCe {
                distinct_ce_count,
                detail,
            } => write!(
                f,
                "SLICE control-set violation: {distinct_ce_count} CE groups cannot fit two 4-FF half-slices ({detail})"
            ),
        }
    }
}

fn extract_ff_control_set(
    design: &Design,
    pins: &helion_ir::PinIndex,
    ff_cell: &str,
) -> Result<FfControlSet, ControlSetViolation> {
    let cell_spur = intern(ff_cell);
    let Some(cell) = design.cells.iter().find(|c| c.name == cell_spur) else {
        return Err(ControlSetViolation::UnknownFf {
            cell: ff_cell.to_string(),
        });
    };
    if !matches!(cell.kind, CellKind::Hff) {
        return Err(ControlSetViolation::UnknownFf {
            cell: ff_cell.to_string(),
        });
    }
    let clk = pins
        .net_on(ff_cell, "CLK")
        .or_else(|| pins.net_on(ff_cell, "C"))
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| ControlSetViolation::MissingClk {
            cell: ff_cell.to_string(),
        })?;
    let ce = pins
        .net_on(ff_cell, "CE")
        .map(|s| resolve(s).to_string())
        .or_else(|| cell.attrs.get("CE_NET").map(|s| s.to_string()));
    let sr = pins
        .net_on(ff_cell, "SR")
        .or_else(|| pins.net_on(ff_cell, "RST"))
        .or_else(|| pins.net_on(ff_cell, "R"))
        .or_else(|| pins.net_on(ff_cell, "S"))
        .map(|s| resolve(s).to_string())
        .or_else(|| cell.attrs.get("SR_NET").map(|s| s.to_string()));
    Ok(FfControlSet { clk, ce, sr })
}

/// Validate that `ff_cells` can legally co-reside in a single UG953 7-series SLICE:
/// - At most 8 FFs total.
/// - All FFs share the same `CLK` net and the same `SR` net.
/// - At most 2 distinct `CE` nets across the SLICE, partitionable into two 4-FF half-slices.
pub fn validate_slice_control_sets(
    design: &Design,
    ff_cells: &[&str],
) -> Result<SliceControlSet, ControlSetViolation> {
    if ff_cells.len() > 8 {
        return Err(ControlSetViolation::TooManyFfs {
            count: ff_cells.len(),
        });
    }
    let pins = design.pin_index();
    if ff_cells.is_empty() {
        return Ok(SliceControlSet {
            clk: String::new(),
            sr: None,
            lower_ce: None,
            upper_ce: None,
            lower_ffs: Vec::new(),
            upper_ffs: Vec::new(),
        });
    }

    let first_cs = extract_ff_control_set(design, &pins, ff_cells[0])?;
    let shared_clk = first_cs.clk.clone();
    let shared_sr = first_cs.sr.clone();

    let mut ce_groups: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for &ff in ff_cells {
        let cs = extract_ff_control_set(design, &pins, ff)?;
        if cs.clk != shared_clk {
            return Err(ControlSetViolation::ConflictingClk {
                clk_a: shared_clk,
                clk_b: cs.clk,
            });
        }
        if cs.sr != shared_sr {
            return Err(ControlSetViolation::ConflictingSr {
                sr_a: shared_sr,
                sr_b: cs.sr,
            });
        }
        if let Some((_, group)) = ce_groups.iter_mut().find(|(ce, _)| *ce == cs.ce) {
            group.push(ff.to_string());
        } else {
            ce_groups.push((cs.ce, vec![ff.to_string()]));
        }
    }

    if ce_groups.len() > 2 {
        return Err(ControlSetViolation::IncompatibleCe {
            distinct_ce_count: ce_groups.len(),
            detail: format!("found {} distinct CE nets (max 2 per SLICE)", ce_groups.len()),
        });
    }

    if ce_groups.len() == 1 {
        let (ce, all_ffs) = ce_groups.into_iter().next().unwrap();
        let split = all_ffs.len().min(4);
        let lower_ffs = all_ffs[..split].to_vec();
        let upper_ffs = all_ffs[split..].to_vec();
        return Ok(SliceControlSet {
            clk: shared_clk,
            sr: shared_sr,
            lower_ce: ce.clone(),
            upper_ce: if upper_ffs.is_empty() { None } else { ce },
            lower_ffs,
            upper_ffs,
        });
    }

    // Exactly 2 distinct CE groups: each must have <= 4 FFs to fit in one 4-FF half-slice.
    let (ce0, g0) = &ce_groups[0];
    let (ce1, g1) = &ce_groups[1];
    if g0.len() > 4 || g1.len() > 4 {
        return Err(ControlSetViolation::IncompatibleCe {
            distinct_ce_count: 2,
            detail: format!(
                "CE group sizes ({} and {}) exceed 4-FF half-slice capacity",
                g0.len(),
                g1.len()
            ),
        });
    }
    Ok(SliceControlSet {
        clk: shared_clk,
        sr: shared_sr,
        lower_ce: ce0.clone(),
        upper_ce: ce1.clone(),
        lower_ffs: g0.clone(),
        upper_ffs: g1.clone(),
    })
}

/// Partition `ff_cells` into the minimum number of UG953-legal SLICE FF groups.
pub fn legalize_slice_ffs(
    design: &Design,
    ff_cells: &[&str],
) -> Result<Vec<SliceControlSet>, ControlSetViolation> {
    let pins = design.pin_index();
    // Group first by `(CLK, SR)`, then by `CE` into 4-FF half-slices, then pair half-slices into 8-FF SLICEs.
    let mut by_clk_sr: Vec<((String, Option<String>), Vec<(Option<String>, Vec<String>)>)> =
        Vec::new();
    for &ff in ff_cells {
        let cs = extract_ff_control_set(design, &pins, ff)?;
        let key = (cs.clk, cs.sr);
        let entry = if let Some((_, ce_map)) = by_clk_sr.iter_mut().find(|(k, _)| *k == key) {
            ce_map
        } else {
            by_clk_sr.push((key, Vec::new()));
            &mut by_clk_sr.last_mut().unwrap().1
        };
        if let Some((_, list)) = entry.iter_mut().find(|(ce, _)| *ce == cs.ce) {
            list.push(ff.to_string());
        } else {
            entry.push((cs.ce, vec![ff.to_string()]));
        }
    }

    let mut slices = Vec::new();
    for ((clk, sr), ce_groups) in by_clk_sr {
        let mut halves: Vec<(Option<String>, Vec<String>)> = Vec::new();
        for (ce, ffs) in ce_groups {
            for chunk in ffs.chunks(4) {
                halves.push((ce.clone(), chunk.to_vec()));
            }
        }
        let mut i = 0usize;
        while i < halves.len() {
            let (lower_ce, lower_ffs) = halves[i].clone();
            let (upper_ce, upper_ffs) = if i + 1 < halves.len() {
                halves[i + 1].clone()
            } else {
                (None, Vec::new())
            };
            slices.push(SliceControlSet {
                clk: clk.clone(),
                sr: sr.clone(),
                lower_ce,
                upper_ce,
                lower_ffs,
                upper_ffs,
            });
            i += 2;
        }
    }
    Ok(slices)
}

/// Check whether a 64-bit `Lut6` `init` functionally depends on input variable `pin` (`0..6`).
pub fn lut_init_depends_on_pin(init: u64, pin: usize) -> bool {
    if pin >= 6 {
        return false;
    }
    let step = 1usize << pin;
    for addr in 0..64usize {
        if (addr & step) == 0 {
            let b0 = (init >> addr) & 1;
            let b1 = (init >> (addr | step)) & 1;
            if b0 != b1 {
                return true;
            }
        }
    }
    false
}

/// Active input pins `(pin_idx, net_name)` for a `Lut6` cell.
/// If all connected pins are <=5, preserves all connected pins on `I0..I5`;
/// if 6 pins are connected, drops pins that `init` does not functionally depend on.
pub fn lut_active_inputs(
    pins: &helion_ir::PinIndex,
    cell_name: &str,
    init: u64,
) -> Vec<(usize, String)> {
    const IPINS: [&str; 6] = ["I0", "I1", "I2", "I3", "I4", "I5"];
    let mut connected = Vec::new();
    for (idx, pin_name) in IPINS.iter().enumerate() {
        if let Some(net) = pins.net_on(cell_name, pin_name) {
            connected.push((idx, resolve(net).to_string()));
        }
    }
    if connected.len() <= 5 {
        return connected;
    }
    connected
        .into_iter()
        .filter(|&(idx, _)| lut_init_depends_on_pin(init, idx))
        .collect()
}

/// Evaluate a 64-bit `Lut6` truth table `init` when its active inputs `(pin_idx, net)`
/// are remapped onto `shared_inputs[0..5]` (`I0..I4`), producing a 32-bit `LUT5` truth table.
pub fn permute_lut_to_shared_5(
    init: u64,
    lut_inputs: &[(usize, String)],
    shared_inputs: &[Option<String>; 5],
) -> u32 {
    let mut out32 = 0u32;
    for addr5 in 0..32u32 {
        let mut orig_addr = 0usize;
        for (orig_pin, net) in lut_inputs {
            if let Some(shared_pos) = shared_inputs
                .iter()
                .position(|s| s.as_deref() == Some(net.as_str()))
            {
                if ((addr5 >> shared_pos) & 1) == 1 {
                    orig_addr |= 1usize << orig_pin;
                }
            }
        }
        let bit = ((init >> orig_addr) & 1) as u32;
        out32 |= bit << addr5;
    }
    out32
}

/// Evaluate `O6` (`I5 = 1`, upper 32 bits) of a fractured `LUT6_2` `combined_init`
/// at 5-bit shared input address `addr5` (`0..32`).
#[inline]
pub fn eval_lut6_2_o6(combined_init: u64, addr5: u32) -> bool {
    ((combined_init >> (32 + (addr5 & 31))) & 1) == 1
}

/// Evaluate `O5` (`I5 = 0`, lower 32 bits) of a fractured `LUT6_2` `combined_init`
/// at 5-bit shared input address `addr5` (`0..32`).
#[inline]
pub fn eval_lut6_2_o5(combined_init: u64, addr5: u32) -> bool {
    ((combined_init >> (addr5 & 31)) & 1) == 1
}

/// A physical 7-series `LUT6_2` site packing either a fractured pair of `<=5`-input LUTs
/// (`O6` when `I5=1`, `O5` when `I5=0`) or a single unfractured `LUT6`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedLut6_2 {
    /// Primary LUT cell mapped to `O6` (`I5 = 1`).
    pub o6_lut: String,
    /// Secondary LUT cell mapped to `O5` (`I5 = 0`) when fractured into `LUT6_2`.
    pub o5_lut: Option<String>,
    /// 64-bit `INIT` (`[31:0]` = `O5` truth table, `[63:32]` = `O6` truth table).
    pub combined_init: u64,
    /// Shared physical inputs `I0..I4` (`I5` is tied high `1` when fractured).
    pub shared_inputs: [Option<String>; 5],
    /// Sixth input `I5` (only `Some` for unfractured 6-input LUTs; `None` when fractured).
    pub i5_input: Option<String>,
    /// Net driven by `O6`.
    pub o6_net: String,
    /// Net driven by `O5` (when fractured).
    pub o5_net: Option<String>,
    /// Optional flip-flop (`AFF`..`DFF`) driven by `O6`.
    pub o6_ff: Option<String>,
    /// Optional secondary flip-flop (`A5FF`..`D5FF`) driven by `O5`.
    pub o5_ff: Option<String>,
}

/// Summary report from `pack_lut6_2_pairs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lut6_2PackReport {
    pub sites: Vec<PackedLut6_2>,
    pub original_lut_count: usize,
    pub physical_lut6_count: usize,
    pub fractured_pair_count: usize,
    pub lut6_saved: usize,
}

/// Identify compatible pairs of `<=5`-input LUTs with union support `|supp(L_a) U supp(L_b)| <= 5`,
/// permute their truth tables onto shared inputs `I0..I4`, and pack both into a single physical
/// `LUT6_2` site (`I5=1` -> `O6`, `I5=0` -> `O5`).
pub fn pack_lut6_2_pairs(design: &Design) -> Lut6_2PackReport {
    let pins = design.pin_index();

    // Map net -> FF cell whose `D` is driven by that net.
    let mut d_to_ff: std::collections::HashMap<Spur, String> = std::collections::HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Hff) {
            if let Some(dnet) = pins.net_on(resolve(c.name), "D") {
                d_to_ff
                    .entry(dnet)
                    .or_insert_with(|| resolve(c.name).to_string());
            }
        }
    }

    struct LutCand {
        name: String,
        init: u64,
        o_net: String,
        active_inputs: Vec<(usize, String)>,
        unique_nets: Vec<String>,
        ff_cell: Option<String>,
        ff_cs: Option<FfControlSet>,
        dont_fracture: bool,
    }

    let mut cands: Vec<LutCand> = Vec::new();
    for c in &design.cells {
        let CellKind::Lut6 { init } = c.kind else {
            continue;
        };
        let cname = resolve(c.name).to_string();
        let Some(o_spur) = pins.net_on(&cname, "O") else {
            continue;
        };
        let o_net = resolve(o_spur).to_string();
        let active_inputs = lut_active_inputs(&pins, &cname, init);
        let mut unique_nets: Vec<String> = Vec::new();
        for (_, n) in &active_inputs {
            if !unique_nets.contains(n) {
                unique_nets.push(n.clone());
            }
        }
        let ff_cell = d_to_ff.get(&o_spur).cloned();
        let ff_cs = ff_cell
            .as_deref()
            .and_then(|ff| extract_ff_control_set(design, &pins, ff).ok());
        let dont_fracture = c.attrs.get("SRL_TYPE").is_some() || unique_nets.len() > 5;
        cands.push(LutCand {
            name: cname,
            init,
            o_net,
            active_inputs,
            unique_nets,
            ff_cell,
            ff_cs,
            dont_fracture,
        });
    }

    let original_lut_count = cands.len();
    let mut paired = vec![false; cands.len()];
    let mut sites = Vec::new();
    let mut fractured_pair_count = 0usize;

    for i in 0..cands.len() {
        if paired[i] {
            continue;
        }
        if cands[i].dont_fracture {
            paired[i] = true;
            let mut shared_inputs: [Option<String>; 5] = Default::default();
            let mut i5_input = None;
            for (pin_idx, net) in &cands[i].active_inputs {
                if *pin_idx < 5 {
                    shared_inputs[*pin_idx] = Some(net.clone());
                } else if *pin_idx == 5 {
                    i5_input = Some(net.clone());
                }
            }
            sites.push(PackedLut6_2 {
                o6_lut: cands[i].name.clone(),
                o5_lut: None,
                combined_init: cands[i].init,
                shared_inputs,
                i5_input,
                o6_net: cands[i].o_net.clone(),
                o5_net: None,
                o6_ff: cands[i].ff_cell.clone(),
                o5_ff: None,
            });
            continue;
        }

        // Find best partner `j > i` maximizing shared input count and keeping union <= 5.
        let mut best_j: Option<(usize, usize, usize, Vec<String>)> = None;
        for j in (i + 1)..cands.len() {
            if paired[j] || cands[j].dont_fracture {
                continue;
            }
            // If both LUTs drive FFs, they occupy AFF + A5FF in the same half-slice, so they
            // must share identical (CLK, CE, SR).
            if let (Some(cs_a), Some(cs_b)) = (&cands[i].ff_cs, &cands[j].ff_cs) {
                if cs_a != cs_b {
                    continue;
                }
            }
            let mut union_nets = cands[i].unique_nets.clone();
            let mut shared_cnt = 0usize;
            for n in &cands[j].unique_nets {
                if union_nets.contains(n) {
                    shared_cnt += 1;
                } else {
                    union_nets.push(n.clone());
                }
            }
            if union_nets.len() > 5 {
                continue;
            }
            let better = match &best_j {
                None => true,
                Some((_, best_shared, best_union_len, _)) => {
                    shared_cnt > *best_shared
                        || (shared_cnt == *best_shared && union_nets.len() < *best_union_len)
                }
            };
            if better {
                best_j = Some((j, shared_cnt, union_nets.len(), union_nets));
            }
        }

        if let Some((j, _, _, union_nets)) = best_j {
            paired[i] = true;
            paired[j] = true;
            fractured_pair_count += 1;
            let mut shared_inputs: [Option<String>; 5] = Default::default();
            for (idx, net) in union_nets.into_iter().take(5).enumerate() {
                shared_inputs[idx] = Some(net);
            }
            let o6_init32 =
                permute_lut_to_shared_5(cands[i].init, &cands[i].active_inputs, &shared_inputs);
            let o5_init32 =
                permute_lut_to_shared_5(cands[j].init, &cands[j].active_inputs, &shared_inputs);
            let combined_init = (o5_init32 as u64) | ((o6_init32 as u64) << 32);
            sites.push(PackedLut6_2 {
                o6_lut: cands[i].name.clone(),
                o5_lut: Some(cands[j].name.clone()),
                combined_init,
                shared_inputs,
                i5_input: None,
                o6_net: cands[i].o_net.clone(),
                o5_net: Some(cands[j].o_net.clone()),
                o6_ff: cands[i].ff_cell.clone(),
                o5_ff: cands[j].ff_cell.clone(),
            });
        } else {
            paired[i] = true;
            let mut shared_inputs: [Option<String>; 5] = Default::default();
            for (idx, net) in cands[i].unique_nets.iter().take(5).enumerate() {
                shared_inputs[idx] = Some(net.clone());
            }
            let o6_init32 =
                permute_lut_to_shared_5(cands[i].init, &cands[i].active_inputs, &shared_inputs);
            let combined_init = (o6_init32 as u64) | ((o6_init32 as u64) << 32);
            sites.push(PackedLut6_2 {
                o6_lut: cands[i].name.clone(),
                o5_lut: None,
                combined_init,
                shared_inputs,
                i5_input: None,
                o6_net: cands[i].o_net.clone(),
                o5_net: None,
                o6_ff: cands[i].ff_cell.clone(),
                o5_ff: None,
            });
        }
    }

    let physical_lut6_count = sites.len();
    let lut6_saved = original_lut_count.saturating_sub(physical_lut6_count);
    Lut6_2PackReport {
        sites,
        original_lut_count,
        physical_lut6_count,
        fractured_pair_count,
        lut6_saved,
    }
}

/// Co-packed `CARRY4` SLICE tile containing the `CARRY4` primitive, its driving propagate/generate
/// `LUT6`s (`A6LUT..D6LUT` / `A5LUT..D5LUT`), and its output sum/carry `Hff`s (`AFF..DFF` / `A5FF..D5FF`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedCarrySlice {
    pub carry4_cell: String,
    /// Vertical SLICE offset from the chain's LSB root (`y = y_base + y_offset`).
    pub y_offset: usize,
    /// Propagate LUTs driving `S0..S3` (`S[0]..S[3]`).
    pub prop_luts: [Option<String>; 4],
    /// Generate LUTs driving `DI0..DI3` (`DI[0]..DI[3]`) when driven by a LUT.
    pub gen_luts: [Option<String>; 4],
    /// Sum flip-flops driven by `O0..O3` (`O[0]..O[3]`).
    pub sum_ffs: [Option<String>; 4],
    /// Carry flip-flops driven by `CO0..CO3` (`CO[0]..CO[3]`).
    pub carry_ffs: [Option<String>; 4],
}

/// Contiguous vertical `CARRY4` chain ordered from LSB (`y_offset = 0`) to MSB (`CI` <- `CO3`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedCarryChain {
    pub slices: Vec<PackedCarrySlice>,
}

fn pin_net_either(
    pins: &helion_ir::PinIndex,
    cell: &str,
    flat_pin: &str,
    bracket_pin: &str,
) -> Option<Spur> {
    pins.net_on(cell, flat_pin)
        .or_else(|| pins.net_on(cell, bracket_pin))
}

/// Group vertical `CARRY4` chains (`CI` <- `CO3`/`CO[3]`) with their driving `S[0..3]`/`DI[0..3]`
/// `LUT6` cells and output `O[0..3]`/`CO[0..3]` `Hff` flip-flops into contiguous SLICE tiles.
pub fn pack_carry4_chains(design: &Design) -> Vec<PackedCarryChain> {
    let pins = design.pin_index();

    let mut lut_by_o: std::collections::HashMap<Spur, String> = std::collections::HashMap::new();
    let mut ff_by_d: std::collections::HashMap<Spur, String> = std::collections::HashMap::new();
    let mut carry_cells: Vec<String> = Vec::new();
    let mut carry_by_co3: std::collections::HashMap<Spur, String> = std::collections::HashMap::new();
    let mut ci_of_carry: std::collections::HashMap<String, Spur> = std::collections::HashMap::new();
    let mut next_carry_of: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for c in &design.cells {
        let cname = resolve(c.name).to_string();
        match c.kind {
            CellKind::Lut6 { .. } => {
                if let Some(onet) = pins.net_on(&cname, "O") {
                    lut_by_o.insert(onet, cname);
                }
            }
            CellKind::Hff => {
                if let Some(dnet) = pins.net_on(&cname, "D") {
                    ff_by_d.entry(dnet).or_insert(cname);
                }
            }
            CellKind::Carry4 => {
                if let Some(co3) = pin_net_either(&pins, &cname, "CO3", "CO[3]") {
                    carry_by_co3.insert(co3, cname.clone());
                }
                if let Some(ci) = pins.net_on(&cname, "CI") {
                    ci_of_carry.insert(cname.clone(), ci);
                }
                carry_cells.push(cname);
            }
            _ => {}
        }
    }

    let mut has_pred: std::collections::HashSet<String> = std::collections::HashSet::new();
    for cname in &carry_cells {
        if let Some(&ci_net) = ci_of_carry.get(cname) {
            if let Some(pred) = carry_by_co3.get(&ci_net) {
                if pred != cname {
                    next_carry_of.insert(pred.clone(), cname.clone());
                    has_pred.insert(cname.clone());
                }
            }
        }
    }

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut chains = Vec::new();

    for root in &carry_cells {
        if has_pred.contains(root) || visited.contains(root) {
            continue;
        }
        let mut slices = Vec::new();
        let mut cur_opt = Some(root.clone());
        let mut y_offset = 0usize;

        while let Some(cur) = cur_opt {
            if !visited.insert(cur.clone()) {
                break;
            }
            let mut prop_luts: [Option<String>; 4] = Default::default();
            let mut gen_luts: [Option<String>; 4] = Default::default();
            let mut sum_ffs: [Option<String>; 4] = Default::default();
            let mut carry_ffs: [Option<String>; 4] = Default::default();
            let mut accepted_ffs: Vec<String> = Vec::new();

            for lane in 0..4usize {
                if let Some(s_net) =
                    pin_net_either(&pins, &cur, &format!("S{lane}"), &format!("S[{lane}]"))
                {
                    if let Some(plut) = lut_by_o.get(&s_net) {
                        prop_luts[lane] = Some(plut.clone());
                    }
                }
                if let Some(di_net) =
                    pin_net_either(&pins, &cur, &format!("DI{lane}"), &format!("DI[{lane}]"))
                {
                    if let Some(glut) = lut_by_o.get(&di_net) {
                        if prop_luts[lane].as_ref() != Some(glut) {
                            gen_luts[lane] = Some(glut.clone());
                        }
                    }
                }
                if let Some(o_net) =
                    pin_net_either(&pins, &cur, &format!("O{lane}"), &format!("O[{lane}]"))
                {
                    if let Some(sff) = ff_by_d.get(&o_net) {
                        let mut try_ffs: Vec<&str> =
                            accepted_ffs.iter().map(|s| s.as_str()).collect();
                        try_ffs.push(sff.as_str());
                        if validate_slice_control_sets(design, &try_ffs).is_ok() {
                            accepted_ffs.push(sff.clone());
                            sum_ffs[lane] = Some(sff.clone());
                        }
                    }
                }
                if let Some(co_net) =
                    pin_net_either(&pins, &cur, &format!("CO{lane}"), &format!("CO[{lane}]"))
                {
                    if let Some(cff) = ff_by_d.get(&co_net) {
                        let mut try_ffs: Vec<&str> =
                            accepted_ffs.iter().map(|s| s.as_str()).collect();
                        try_ffs.push(cff.as_str());
                        if validate_slice_control_sets(design, &try_ffs).is_ok() {
                            accepted_ffs.push(cff.clone());
                            carry_ffs[lane] = Some(cff.clone());
                        }
                    }
                }
            }

            slices.push(PackedCarrySlice {
                carry4_cell: cur.clone(),
                y_offset,
                prop_luts,
                gen_luts,
                sum_ffs,
                carry_ffs,
            });
            y_offset += 1;
            cur_opt = next_carry_of.get(&cur).cloned();
        }
        chains.push(PackedCarryChain { slices });
    }
    chains
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design};

    #[test]
    fn packs_blinky_lutff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert_eq!(p.iobs.len(), 1);
        assert_eq!(p.lutffs[0].init, 0x5555_5555_5555_5555);
        assert_eq!(p.lutffs[0].lut_pins, vec![(0, "u_ff".into())]);
        assert_eq!(p.lutffs[0].q_net, "q");
    }

    #[test]
    fn packs_counter_four_lutffs_with_pins() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 4);
        assert_eq!(p.lutffs[3].lut_pins.len(), 4);
        assert_eq!(p.iobs[0].from_net, "q3");
        assert_eq!(p.lutffs[3].q_net, "q3");
        assert!(p.iobs[0].drive.is_none());
        assert!(p.iobs[0].slew.is_none());
        assert!(p.iobs[0].pulltype.is_none());
        assert!(p.iobs[0].diff_term.is_none());
        assert!(p.iobs[0].in_term.is_none());
        let mut d = Design::structural_counter();
        d.set_drive("led", "4").unwrap();
        d.set_slew("led", "FAST").unwrap();
        d.set_pulltype("led", "PULLUP").unwrap();
        d.set_diff_term("led", "TRUE").unwrap();
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.iobs[0].drive.as_deref(), Some("4"));
        assert_eq!(p.iobs[0].slew.as_deref(), Some("FAST"));
        assert_eq!(p.iobs[0].pulltype.as_deref(), Some("PULLUP"));
        assert_eq!(p.iobs[0].diff_term.as_deref(), Some("TRUE"));
        assert_eq!(p.iobs[0].in_term.as_deref(), Some("UNTUNED_SPLIT_50"));
    }

    #[test]
    fn packs_comb_lut_without_ff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("comb");
        d.add_port("a", helion_ir::PortDir::In);
        d.add_port("y", helion_ir::PortDir::Out);
        d.add_cell(
            "u_lut",
            CellKind::Lut6 {
                init: 0x5555_5555_5555_5555,
            },
        );
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("a", "u_lut", "I0");
        d.connect("n", "u_lut", "O");
        d.connect("n", "u_iob", "I");
        d.connect("y", "u_iob", "PAD");
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert!(
            p.lutffs[0].ff_cell.is_empty(),
            "comb LUT must not invent an FF"
        );
        assert_eq!(p.lutffs[0].q_net, "n");
        assert_eq!(p.iobs[0].from_net, "n");
    }

    #[test]
    fn packs_mac27() {
        let dev = Device::load_part("HL10T-DSP1").unwrap();
        let mut d = Design::new("mac");
        d.add_cell("u_mac", CellKind::Mac27);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.macs.len(), 1);
        assert!(p.lutffs.is_empty());
    }

    #[test]
    fn pack_thousands_of_lutff_pairs_is_subquadratic() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("wide");
        d.add_port("clk", helion_ir::PortDir::In);
        const N: u32 = 3_000;
        for i in 0..N {
            let lut = format!("lut{i}");
            let ff = format!("ff{i}");
            d.add_cell(
                &lut,
                CellKind::Lut6 {
                    init: 0x5555_5555_5555_5555,
                },
            );
            d.add_cell(&ff, CellKind::Hff);
            d.connect("clk", &ff, "CLK");
            d.connect(&format!("d{i}"), &lut, "O");
            d.connect(&format!("d{i}"), &ff, "D");
            d.connect(&format!("q{i}"), &ff, "Q");
            d.connect(&format!("q{i}"), &lut, "I0");
        }
        let t0 = std::time::Instant::now();
        let p = pack(&d, &dev).unwrap();
        let ms = t0.elapsed().as_millis();
        assert_eq!(p.lutffs.len(), N as usize, "every LUT/FF pair packs");
        assert!(
            p.lutffs
                .iter()
                .all(|l| !l.ff_cell.is_empty() && !l.q_net.is_empty()),
            "clusters keep FF and Q net"
        );
        assert!(
            ms < 2000,
            "pack of {N} LUT/FF pairs must stay subquadratic, took {ms}ms"
        );
    }

    #[test]
    fn packs_bram18() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("ram");
        d.add_cell("u_bram", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.brams.len(), 1);
    }

    #[test]
    fn packing_summary_groups_lutff_sites_by_had_id() {
        use helion_device::SiteKind;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let s0 = Site {
            x: dev.clb_x0,
            y: dev.clb_y0,
            kind: SiteKind::Clb,
        };
        let s1 = Site {
            x: dev.clb_x0,
            y: dev.clb_y0 + 1,
            kind: SiteKind::Clb,
        };
        assert!(dev.contains_site(s0) && dev.contains_site(s1));
        let sites = vec![(s0, 0), (s0, 1), (s0, 2), (s1, 0)];
        let sum = PackingSummary::from_lutff_sites(&sites);
        assert_eq!(sum.lutff_in(&s0.id()), 3);
        assert_eq!(sum.lutff_in(&s1.id()), 1);
        assert_eq!(dev.site_by_id(&s0.id()), Some(s0));
        assert_eq!(dev.site_by_id(&s1.id()), Some(s1));
        assert_eq!(sum.sites.len(), 2);
    }

    #[test]
    fn pack_rejects_multibackend_cells_for_hl10t() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        for kind in [CellKind::IobIn, CellKind::ClockBuf] {
            let mut d = Design::new("mb");
            d.add_cell("u_mb", kind);
            let err = pack(&d, &dev).unwrap_err();
            assert!(
                err.contains("unsupported for HL10T"),
                "expected HL10T rejection, got: {err}"
            );
        }
        // The gold counter (no multi-backend cells) still packs fine.
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 4);
    }

    #[test]
    fn lut6_2_fracture_packer_combines_compatible_5input_luts_and_preserves_truth_tables() {
        let mut d = Design::new("lut6_2_test");
        for p in ["a", "b", "c", "d", "e", "f"] {
            d.add_port(p, helion_ir::PortDir::In);
        }

        // lut_a: 3-input XOR of (a, b, c) on pins I0=a, I1=b, I2=c
        let mut xor3_init = 0u64;
        // lut_b: 4-input AND-OR `(b & c) | (d ^ e)` on pins I0=b, I1=c, I2=d, I3=e
        let mut and_xor_init = 0u64;
        for addr in 0..64u64 {
            let i0 = (addr & 1) != 0;
            let i1 = ((addr >> 1) & 1) != 0;
            let i2 = ((addr >> 2) & 1) != 0;
            let i3 = ((addr >> 3) & 1) != 0;
            if i0 ^ i1 ^ i2 {
                xor3_init |= 1u64 << addr;
            }
            if (i0 && i1) || (i2 ^ i3) {
                and_xor_init |= 1u64 << addr;
            }
        }

        d.add_cell("lut_a", CellKind::Lut6 { init: xor3_init });
        d.connect("a", "lut_a", "I0");
        d.connect("b", "lut_a", "I1");
        d.connect("c", "lut_a", "I2");
        d.connect("y_a", "lut_a", "O");

        d.add_cell("lut_b", CellKind::Lut6 { init: and_xor_init });
        d.connect("b", "lut_b", "I0");
        d.connect("c", "lut_b", "I1");
        d.connect("d", "lut_b", "I2");
        d.connect("e", "lut_b", "I3");
        d.connect("y_b", "lut_b", "O");

        // lut_6in: true 6-input AND of a..f -> cannot fracture with anything
        let and6_init = 1u64 << 63;
        d.add_cell("lut_6in", CellKind::Lut6 { init: and6_init });
        for (idx, net) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
            d.connect(*net, "lut_6in", &format!("I{idx}"));
        }
        d.connect("y_6", "lut_6in", "O");

        let rep = pack_lut6_2_pairs(&d);
        assert_eq!(rep.original_lut_count, 3);
        assert_eq!(rep.fractured_pair_count, 1);
        assert_eq!(rep.physical_lut6_count, 2);
        assert_eq!(rep.lut6_saved, 1);

        let pair = rep
            .sites
            .iter()
            .find(|s| s.o5_lut.is_some())
            .expect("lut_a and lut_b must fracture into one LUT6_2");
        assert_eq!(pair.o6_lut, "lut_a");
        assert_eq!(pair.o5_lut.as_deref(), Some("lut_b"));
        assert_eq!(pair.o6_net, "y_a");
        assert_eq!(pair.o5_net.as_deref(), Some("y_b"));

        // Exhaustively verify all 32 input assignments to shared_inputs[0..5]
        for addr5 in 0..32u32 {
            let val_of = |net_name: &str| -> bool {
                let pos = pair
                    .shared_inputs
                    .iter()
                    .position(|x| x.as_deref() == Some(net_name))
                    .expect("net in shared_inputs");
                ((addr5 >> pos) & 1) == 1
            };
            let va = val_of("a");
            let vb = val_of("b");
            let vc = val_of("c");
            let vd = val_of("d");
            let ve = val_of("e");
            let expect_o6 = va ^ vb ^ vc;
            let expect_o5 = (vb && vc) || (vd ^ ve);
            assert_eq!(
                eval_lut6_2_o6(pair.combined_init, addr5),
                expect_o6,
                "O6 mismatch at addr5={addr5:#b}"
            );
            assert_eq!(
                eval_lut6_2_o5(pair.combined_init, addr5),
                expect_o5,
                "O5 mismatch at addr5={addr5:#b}"
            );
        }
    }

    #[test]
    fn lut6_2_refuses_union_over_5_or_conflicting_ff_control_sets() {
        let mut d = Design::new("lut6_2_reject");
        // Two 3-input LUTs on disjoint nets {a,b,c} and {d,e,f} -> union size 6 > 5.
        d.add_cell("l0", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
        d.connect("a", "l0", "I0");
        d.connect("b", "l0", "I1");
        d.connect("c", "l0", "I2");
        d.connect("o0", "l0", "O");

        d.add_cell("l1", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
        d.connect("d", "l1", "I0");
        d.connect("e", "l1", "I1");
        d.connect("f", "l1", "I2");
        d.connect("o1", "l1", "O");

        let rep = pack_lut6_2_pairs(&d);
        assert_eq!(rep.fractured_pair_count, 0, "union size 6 must not fracture");

        // Now two 2-input LUTs sharing inputs {a, b} but driving FFs on different clocks.
        let mut d2 = Design::new("lut6_2_clk_conflict");
        d2.add_cell("la", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
        d2.connect("a", "la", "I0");
        d2.connect("b", "la", "I1");
        d2.connect("da", "la", "O");
        d2.add_cell("ffa", CellKind::Hff);
        d2.connect("clk1", "ffa", "CLK");
        d2.connect("da", "ffa", "D");
        d2.connect("qa", "ffa", "Q");

        d2.add_cell("lb", CellKind::Lut6 { init: 0x6666_6666_6666_6666 });
        d2.connect("a", "lb", "I0");
        d2.connect("b", "lb", "I1");
        d2.connect("db", "lb", "O");
        d2.add_cell("ffb", CellKind::Hff);
        d2.connect("clk2", "ffb", "CLK");
        d2.connect("db", "ffb", "D");
        d2.connect("qb", "ffb", "Q");

        let rep2 = pack_lut6_2_pairs(&d2);
        assert_eq!(
            rep2.fractured_pair_count, 0,
            "LUTs driving FFs with different clocks must not fracture into the same LUT6_2 site"
        );
    }

    #[test]
    fn carry4_co_packer_orders_vertical_chain_and_attaches_luts_and_ffs() {
        let mut d = Design::new("carry_chain_8bit");
        // Build a 2-stage (8-bit) CARRY4 chain: c_hi -> c_lo in cell list order to test root discovery.
        d.add_cell("c_hi", CellKind::Carry4);
        d.add_cell("c_lo", CellKind::Carry4);
        d.connect("0", "c_lo", "CYINIT");
        d.connect("carry_mid", "c_lo", "CO3");
        d.connect("carry_mid", "c_hi", "CI");

        for stage in 0..2usize {
            let cname = if stage == 0 { "c_lo" } else { "c_hi" };
            for lane in 0..4usize {
                let bit = stage * 4 + lane;
                let plut = format!("prop_lut_{bit}");
                let glut = format!("gen_lut_{bit}");
                let sff = format!("sum_ff_{bit}");
                let s_net = format!("s_net_{bit}");
                let di_net = format!("di_net_{bit}");
                let o_net = format!("o_net_{bit}");

                d.add_cell(&plut, CellKind::Lut6 { init: 0x6666_6666_6666_6666 });
                d.connect(&format!("a{bit}"), &plut, "I0");
                d.connect(&format!("b{bit}"), &plut, "I1");
                d.connect(&s_net, &plut, "O");

                d.add_cell(&glut, CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
                d.connect(&format!("a{bit}"), &glut, "I0");
                d.connect(&format!("b{bit}"), &glut, "I1");
                d.connect(&di_net, &glut, "O");

                d.connect(&s_net, cname, &format!("S{lane}"));
                d.connect(&di_net, cname, &format!("DI{lane}"));
                d.connect(&o_net, cname, &format!("O{lane}"));

                d.add_cell(&sff, CellKind::Hff);
                d.connect("clk", &sff, "CLK");
                d.connect(&o_net, &sff, "D");
                d.connect(&format!("q{bit}"), &sff, "Q");
            }
        }

        let chains = pack_carry4_chains(&d);
        assert_eq!(chains.len(), 1, "expected 1 contiguous CARRY4 chain");
        assert_eq!(chains[0].slices.len(), 2, "expected 2 vertical CARRY4 slices");
        assert_eq!(chains[0].slices[0].carry4_cell, "c_lo");
        assert_eq!(chains[0].slices[0].y_offset, 0);
        assert_eq!(chains[0].slices[1].carry4_cell, "c_hi");
        assert_eq!(chains[0].slices[1].y_offset, 1);

        for lane in 0..4usize {
            assert_eq!(
                chains[0].slices[0].prop_luts[lane].as_deref(),
                Some(format!("prop_lut_{lane}").as_str())
            );
            assert_eq!(
                chains[0].slices[0].gen_luts[lane].as_deref(),
                Some(format!("gen_lut_{lane}").as_str())
            );
            assert_eq!(
                chains[0].slices[0].sum_ffs[lane].as_deref(),
                Some(format!("sum_ff_{lane}").as_str())
            );
            let hi_bit = 4 + lane;
            assert_eq!(
                chains[0].slices[1].prop_luts[lane].as_deref(),
                Some(format!("prop_lut_{hi_bit}").as_str())
            );
            assert_eq!(
                chains[0].slices[1].sum_ffs[lane].as_deref(),
                Some(format!("sum_ff_{hi_bit}").as_str())
            );
        }
    }

    #[test]
    fn ug953_control_set_legalizer_enforces_clk_sr_and_two_half_slice_ces() {
        let mut d = Design::new("cs_test");
        // 4 FFs with (clk1, ce_a, rst1) and 4 FFs with (clk1, ce_b, rst1)
        for i in 0..4 {
            let ffa = format!("ff_a{i}");
            d.add_cell(&ffa, CellKind::Hff);
            d.connect("clk1", &ffa, "CLK");
            d.connect("ce_a", &ffa, "CE");
            d.connect("rst1", &ffa, "SR");
            d.connect(&format!("da{i}"), &ffa, "D");
            d.connect(&format!("qa{i}"), &ffa, "Q");

            let ffb = format!("ff_b{i}");
            d.add_cell(&ffb, CellKind::Hff);
            d.connect("clk1", &ffb, "CLK");
            d.connect("ce_b", &ffb, "CE");
            d.connect("rst1", &ffb, "SR");
            d.connect(&format!("db{i}"), &ffb, "D");
            d.connect(&format!("qb{i}"), &ffb, "Q");
        }
        // Extra FFs with conflicting CE / SR / CLK
        d.add_cell("ff_a4", CellKind::Hff);
        d.connect("clk1", "ff_a4", "CLK");
        d.connect("ce_a", "ff_a4", "CE");
        d.connect("rst1", "ff_a4", "SR");
        d.connect("da4", "ff_a4", "D");
        d.connect("qa4", "ff_a4", "Q");

        d.add_cell("ff_c0", CellKind::Hff);
        d.connect("clk1", "ff_c0", "CLK");
        d.connect("ce_c", "ff_c0", "CE");
        d.connect("rst1", "ff_c0", "SR");
        d.connect("dc0", "ff_c0", "D");
        d.connect("qc0", "ff_c0", "Q");

        d.add_cell("ff_bad_sr", CellKind::Hff);
        d.connect("clk1", "ff_bad_sr", "CLK");
        d.connect("ce_a", "ff_bad_sr", "CE");
        d.connect("rst2", "ff_bad_sr", "SR");
        d.connect("dbs", "ff_bad_sr", "D");
        d.connect("qbs", "ff_bad_sr", "Q");

        d.add_cell("ff_bad_clk", CellKind::Hff);
        d.connect("clk2", "ff_bad_clk", "CLK");
        d.connect("ce_a", "ff_bad_clk", "CE");
        d.connect("rst1", "ff_bad_clk", "SR");
        d.connect("dbc", "ff_bad_clk", "D");
        d.connect("qbc", "ff_bad_clk", "Q");

        // 1. Legal 8-FF slice: 4 FFs on ce_a + 4 FFs on ce_b
        let legal_8 = [
            "ff_a0", "ff_a1", "ff_a2", "ff_a3", "ff_b0", "ff_b1", "ff_b2", "ff_b3",
        ];
        let cs = validate_slice_control_sets(&d, &legal_8).expect("4+4 CE split must be legal");
        assert_eq!(cs.clk, "clk1");
        assert_eq!(cs.sr.as_deref(), Some("rst1"));
        assert_eq!(cs.lower_ce.as_deref(), Some("ce_a"));
        assert_eq!(cs.upper_ce.as_deref(), Some("ce_b"));
        assert_eq!(cs.lower_ffs.len(), 4);
        assert_eq!(cs.upper_ffs.len(), 4);

        // 2. Illegal: 5 FFs on ce_a + 1 FF on ce_b (spills across 4-FF half-slice boundary)
        let illegal_5_1 = ["ff_a0", "ff_a1", "ff_a2", "ff_a3", "ff_a4", "ff_b0"];
        assert!(matches!(
            validate_slice_control_sets(&d, &illegal_5_1),
            Err(ControlSetViolation::IncompatibleCe {
                distinct_ce_count: 2,
                ..
            })
        ));

        // 3. Illegal: 3 distinct CE nets in one SLICE
        let illegal_3ce = ["ff_a0", "ff_b0", "ff_c0"];
        assert!(matches!(
            validate_slice_control_sets(&d, &illegal_3ce),
            Err(ControlSetViolation::IncompatibleCe {
                distinct_ce_count: 3,
                ..
            })
        ));

        // 4. Illegal: conflicting SR
        assert!(matches!(
            validate_slice_control_sets(&d, &["ff_a0", "ff_bad_sr"]),
            Err(ControlSetViolation::ConflictingSr { .. })
        ));

        // 5. Illegal: conflicting CLK
        assert!(matches!(
            validate_slice_control_sets(&d, &["ff_a0", "ff_bad_clk"]),
            Err(ControlSetViolation::ConflictingClk { .. })
        ));

        // 6. `legalize_slice_ffs` partitions mixed FFs into legal SLICE buckets
        let all_mixed = [
            "ff_a0",
            "ff_a1",
            "ff_a2",
            "ff_a3",
            "ff_a4",
            "ff_b0",
            "ff_c0",
            "ff_bad_sr",
            "ff_bad_clk",
        ];
        let buckets = legalize_slice_ffs(&d, &all_mixed).expect("legalize_slice_ffs");
        assert_eq!(
            buckets.len(),
            4,
            "expected 4 legal SLICE buckets (2 for clk1/rst1, 1 for clk1/rst2, 1 for clk2/rst1)"
        );
    }
}
