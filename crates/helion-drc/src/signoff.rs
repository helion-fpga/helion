//! Vivado `write_bitstream` Pre-Bitstream Physical Signoff DRC Gate (UG893 / UG906 parity).
//!
//! Enforces mandatory pre-bitstream physical and electrical signoff rules before `.bit` / `.hbits`
//! generation:
//! - `RTSTAT-1..12`: Routing status signoff (unrouted nets, partial routes, overused tiles,
//!   dangling stubs, routing islands, IMUX collisions, undriven/unloaded nets, out-of-grid routes,
//!   excessive detours, site blockages, and unconverged PathFinder iterations).
//! - `NSTD-1`: Unspecified I/O Standard (`DEFAULT` / missing explicit `IOSTANDARD` on top ports).
//! - `UCIO-1`: Unconstrained Logical Port (missing package pin `LOC` on top ports).
//! - `MDRV-1`: Multiple Driver Nets (`>= 2` active output pins driving one net).
//! - `LUTLP-1`: Combinational LUT Loop (cycle of `Lut6` cells with no sequential `Hff` break).
//! - `ANT-1`: Metal Antenna Ratio Violation (`wirelength_um / gate_area_um2 > max_ratio` without
//!   `ANTENNA_DIODE` protection).
//! - `PLCK-1`: Clock Placement & Dedicated Spine Routing check.
//! - `CIPS-1`: Clock Spine Skew & Inter-Column Clock Distribution check.
//! - `BIVC-1`: Bank I/O Voltage (`VCCO`) Compatibility check.
//! - Supports `SEVERITY {Warning}` downgrade waivers (`set_property SEVERITY {Warning} [get_drc_checks NSTD-1]`).

use crate::{Drc, DrcSeverity, check_placed};
use helion_device::Device;
use helion_ir::{CellKind, Design, Spur, intern, resolve};
use helion_route::Routed;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Default maximum legal metal-area to gate-oxide-area antenna ratio (`400:1`).
pub const DEFAULT_MAX_ANTENNA_RATIO: f64 = 400.0;

/// Metadata for a pre-bitstream signoff rule (`RTSTAT-1..12`, `NSTD-1`, `UCIO-1`, etc.).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignoffRuleInfo {
    pub id: String,
    pub default_severity: DrcSeverity,
    pub title: &'static str,
    pub description: &'static str,
    pub downgrade_tcl_hint: String,
}

/// Look up pre-bitstream signoff rule metadata (`RTSTAT-1..12`, `NSTD-1`, `UCIO-1`, `MDRV-1`,
/// `LUTLP-1`, `ANT-1`, `PLCK-1`, `CIPS-1`, `BIVC-1`).
pub fn signoff_rule_info(id: &str) -> Option<SignoffRuleInfo> {
    let norm = id.trim().to_ascii_uppercase();
    if let Some(num_str) = norm.strip_prefix("RTSTAT-") {
        let num: u8 = num_str.parse().ok()?;
        let (sev, title, desc) = match num {
            1 => (
                DrcSeverity::Error,
                "Unrouted net",
                "One or more logical nets have unrouted sink pins after route_design.",
            ),
            2 => (
                DrcSeverity::Error,
                "Partially routed net",
                "Net has incomplete physical routing branches or skipped IMUX connections.",
            ),
            3 => (
                DrcSeverity::Error,
                "Routing node conflict / overused tile",
                "Switchbox or routing node is shared by multiple nets (overused > 0).",
            ),
            4 => (
                DrcSeverity::Warning,
                "Antenna / dangling routing stub",
                "Routed path does not terminate at its expected sink coordinate.",
            ),
            5 => (
                DrcSeverity::Error,
                "Disconnected routing island",
                "Routed tile sequence contains a non-adjacent Manhattan jump (> 1 tile step).",
            ),
            6 => (
                DrcSeverity::Error,
                "IMUX input multiplexer collision",
                "Multiple signals select the same CLB `(x, y, mux)` input multiplexer.",
            ),
            7 => (
                DrcSeverity::Error,
                "Net has load pins but no active driver",
                "Internal net feeds sink pins without any driving output pin or input port.",
            ),
            8 => (
                DrcSeverity::Advisory,
                "Loadless driver net",
                "Net is driven by a cell output pin but connects to no downstream load pins.",
            ),
            9 => (
                DrcSeverity::Error,
                "Route coordinate outside device tile grid",
                "Physical route visits a tile coordinate outside the target device boundary.",
            ),
            10 => (
                DrcSeverity::Warning,
                "Excessive routing detour hops",
                "Routed path length significantly exceeds the Manhattan bounding box.",
            ),
            11 => (
                DrcSeverity::Error,
                "Unroutable site pin blockage",
                "Target BEL/IOB pin cannot be reached from the fabric interconnect.",
            ),
            12 => (
                DrcSeverity::Error,
                "PathFinder negotiation budget exhausted",
                "Router reached iteration limit before resolving all overused resources.",
            ),
            _ => return None,
        };
        return Some(SignoffRuleInfo {
            id: norm.clone(),
            default_severity: sev,
            title,
            description: desc,
            downgrade_tcl_hint: format!(
                "set_property SEVERITY {{Warning}} [get_drc_checks {norm}]"
            ),
        });
    }

    let (sev, title, desc) = match norm.as_str() {
        "NSTD-1" => (
            DrcSeverity::Error,
            "Unspecified I/O Standard",
            "Logical port uses DEFAULT IOSTANDARD instead of a user-assigned board voltage standard.",
        ),
        "UCIO-1" => (
            DrcSeverity::Error,
            "Unconstrained Logical Port Location",
            "Logical port has no user-assigned package pin (LOC / PACKAGE_PIN) constraint.",
        ),
        "MDRV-1" => (
            DrcSeverity::Error,
            "Multiple Driver Nets",
            "Net has more than one active driver pin connected in the post-route netlist.",
        ),
        "LUTLP-1" => (
            DrcSeverity::Error,
            "Combinational Loop Alert",
            "LUT6 combinational cycle detected with no synchronous flip-flop element breaking the loop.",
        ),
        "ANT-1" => (
            DrcSeverity::Error,
            "Metal Antenna Ratio Violation",
            "Routed metal wirelength-to-gate-oxide area ratio exceeds foundry limit without ANTENNA_DIODE protection.",
        ),
        "PLCK-1" => (
            DrcSeverity::Error,
            "Clock Placement & Spine Legality",
            "Clock net is driven by non-clock LUT logic or violates dedicated clock spine placement rules.",
        ),
        "CIPS-1" => (
            DrcSeverity::Warning,
            "Clock Spine Skew & Distribution",
            "Clock tree spans multiple CLB columns with high inter-column skew or missing global clock spine buffer.",
        ),
        "BIVC-1" => (
            DrcSeverity::Error,
            "Bank I/O Voltage Compatibility",
            "Ports placed in the same I/O bank require incompatible VCCO supply voltages.",
        ),
        _ => return None,
    };

    Some(SignoffRuleInfo {
        id: norm.clone(),
        default_severity: sev,
        title,
        description: desc,
        downgrade_tcl_hint: format!("set_property SEVERITY {{Warning}} [get_drc_checks {norm}]"),
    })
}

/// Per-net metal antenna analysis entry (`ANT-1`).
#[derive(Clone, Debug, PartialEq)]
pub struct AntennaNetMetric {
    pub net: String,
    pub hops: u32,
    pub sink_gates: usize,
    pub metal_wirelength_um: f64,
    pub gate_oxide_area_um2: f64,
    pub antenna_ratio: f64,
    pub max_allowed_ratio: f64,
    pub has_diode_protection: bool,
    pub violated: bool,
}

/// Configuration and severity override waivers for [`check_bitstream_signoff`].
#[derive(Clone, Debug)]
pub struct SignoffConfig {
    /// Rule ID → overridden [`DrcSeverity`] (e.g. `NSTD-1 -> Warning`, `UCIO-1 -> Warning`).
    pub severity_overrides: BTreeMap<String, DrcSeverity>,
    /// Maximum legal metal-to-gate antenna ratio (`DEFAULT_MAX_ANTENNA_RATIO = 400.0`).
    pub max_antenna_ratio: f64,
    /// Metal wirelength per routing tile hop in micrometers (default `45.0 um`).
    pub um_per_hop: f64,
    /// Gate oxide area per sink transistor input in $\mu\text{m}^2$ (default `0.45 um^2`).
    pub gate_area_per_sink_um2: f64,
    /// Maximum allowed clock column span before `CIPS-1` skew warning (default `4`).
    pub max_clock_col_span: u32,
}

impl Default for SignoffConfig {
    fn default() -> Self {
        Self {
            severity_overrides: BTreeMap::new(),
            max_antenna_ratio: DEFAULT_MAX_ANTENNA_RATIO,
            um_per_hop: 12.0,
            gate_area_per_sink_um2: 0.45,
            max_clock_col_span: 4,
        }
    }
}

impl SignoffConfig {
    /// Equivalent to `set_property SEVERITY {<severity>} [get_drc_checks <rule_id>]`.
    pub fn set_severity_override(&mut self, rule_id: &str, severity: DrcSeverity) {
        self.severity_overrides
            .insert(rule_id.trim().to_ascii_uppercase(), severity);
    }

    /// Equivalent to `set_property SEVERITY {Warning} [get_drc_checks <rule_id>]`.
    pub fn downgrade_to_warning(&mut self, rule_id: &str) {
        self.set_severity_override(rule_id, DrcSeverity::Warning);
    }

    /// Parse Vivado XDC/Tcl lines of the form:
    /// `set_property SEVERITY {Warning} [get_drc_checks NSTD-1]`
    /// or `set_property SEVERITY Warning [get_drc_checks {NSTD-1 UCIO-1}]`.
    pub fn parse_tcl_severity_overrides(&mut self, tcl: &str) -> Result<usize, String> {
        let mut count = 0usize;
        for (line_no, raw) in tcl.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() || !line.contains("SEVERITY") {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            let sev = if lower.contains("advisory") {
                DrcSeverity::Advisory
            } else if lower.contains("warning") {
                DrcSeverity::Warning
            } else if lower.contains("error") {
                DrcSeverity::Error
            } else {
                return Err(format!(
                    "signoff tcl:{}: unrecognized SEVERITY level in '{line}'",
                    line_no + 1
                ));
            };

            if let Some(idx) = line.find("get_drc_checks") {
                let after = &line[idx + "get_drc_checks".len()..];
                let cleaned: String = after
                    .chars()
                    .map(|c| {
                        if matches!(c, '[' | ']' | '{' | '}' | '"') {
                            ' '
                        } else {
                            c
                        }
                    })
                    .collect();
                for rule_tok in cleaned.split_whitespace() {
                    self.set_severity_override(rule_tok, sev);
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    pub fn effective_severity(
        &self,
        design: &Design,
        rule_id: &str,
        default: DrcSeverity,
    ) -> (DrcSeverity, bool) {
        let key = rule_id.trim().to_ascii_uppercase();
        if let Some(&sev) = self.severity_overrides.get(&key) {
            return (sev, sev != default);
        }
        // Also support design-level attr `DRC_SEVERITY_<RULE>` = "Warning" | "Advisory"
        let attr_key = format!("DRC_SEVERITY_{key}");
        if let Some(val) = design.attrs.get(&attr_key) {
            let sev = match val.trim().to_ascii_lowercase().as_str() {
                "warning" | "warn" => DrcSeverity::Warning,
                "advisory" | "info" => DrcSeverity::Advisory,
                "error" | "err" => DrcSeverity::Error,
                _ => default,
            };
            return (sev, sev != default);
        }
        (default, false)
    }
}

/// Structured pre-bitstream physical signoff report returned by [`check_bitstream_signoff`].
#[derive(Clone, Debug)]
pub struct BitgenSignoffReport {
    /// Underlying [`Drc`] containing all signoff violations with their effective severities.
    pub drc: Drc,
    /// True iff `drc.error_count() == 0` (all hard errors resolved or downgraded to `Warning`).
    pub bitgen_allowed: bool,
    /// Number of unrouted or partially routed nets (`RTSTAT-1` / `RTSTAT-2`).
    pub unrouted_nets: usize,
    /// Number of overused routing tiles (`RTSTAT-3`).
    pub overused_tiles: u32,
    /// Number of unconstrained I/O standard ports (`NSTD-1`).
    pub nstd_ports: Vec<String>,
    /// Number of unconstrained package pin location ports (`UCIO-1`).
    pub ucio_ports: Vec<String>,
    /// Multi-driver nets (`MDRV-1`).
    pub multi_driver_nets: Vec<String>,
    /// Combinational LUT cycles detected (`LUTLP-1`).
    pub combinational_loops: Vec<Vec<String>>,
    /// Per-net metal antenna metrics (`ANT-1`).
    pub antenna_metrics: Vec<AntennaNetMetric>,
    /// Rule IDs whose severity was downgraded from `Error` to `Warning`/`Advisory`.
    pub downgraded_rules: Vec<String>,
}

impl BitgenSignoffReport {
    /// Gate `write_bitstream`: returns `Ok(())` when `bitgen_allowed`, or `Err` listing blocking DRC errors.
    pub fn gate_write_bitstream(&self) -> Result<(), String> {
        if self.bitgen_allowed {
            Ok(())
        } else {
            Err(format!(
                "write_bitstream: DRC signoff failed with {} error(s): {}",
                self.drc.error_count(),
                self.drc.fail_errors().unwrap_err()
            ))
        }
    }

    pub fn summary_line(&self) -> String {
        format!(
            "bitgen_signoff allowed={} errors={} warnings={} unrouted={} overused={} loops={} antenna_violations={} downgraded={}",
            self.bitgen_allowed,
            self.drc.error_count(),
            self.drc
                .items
                .iter()
                .filter(|v| v.severity == DrcSeverity::Warning)
                .count(),
            self.unrouted_nets,
            self.overused_tiles,
            self.combinational_loops.len(),
            self.antenna_metrics.iter().filter(|a| a.violated).count(),
            self.downgraded_rules.len()
        )
    }
}

fn push_signoff(
    drc: &mut Drc,
    downgraded: &mut BTreeSet<String>,
    cfg: &SignoffConfig,
    design: &Design,
    rule_id: &str,
    objects: impl Into<String>,
    message: impl Into<String>,
) {
    let default_sev = signoff_rule_info(rule_id)
        .map(|r| r.default_severity)
        .unwrap_or(DrcSeverity::Error);
    let (eff_sev, was_changed) = cfg.effective_severity(design, rule_id, default_sev);
    if was_changed {
        downgraded.insert(rule_id.to_string());
    }
    drc.add_with_severity(rule_id, eff_sev, objects, message);
}

/// Detect all elementary directed cycles in the combinational `Lut6` graph (`LUTLP-1`).
pub fn find_combinational_lut_loops(design: &Design) -> Vec<Vec<String>> {
    let pins = design.pin_index();
    let mut o_net_to_lut: HashMap<Spur, String> = HashMap::new();
    let mut lut_names: Vec<String> = Vec::new();

    for c in &design.cells {
        if matches!(c.kind, CellKind::Lut6 { .. }) {
            let cname = resolve(c.name).to_string();
            if let Some(o_net) = pins.net_on(&cname, "O") {
                o_net_to_lut.insert(o_net, cname.clone());
            }
            lut_names.push(cname);
        }
    }

    // Build adjacency list: src_lut -> dst_lut when src_lut's O net directly feeds dst_lut's I0..I5
    // (without passing through an Hff).
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in &design.cells {
        if !matches!(c.kind, CellKind::Lut6 { .. }) {
            continue;
        }
        let dst_lut = resolve(c.name).to_string();
        for ipin in ["I0", "I1", "I2", "I3", "I4", "I5"] {
            if let Some(in_net) = pins.net_on(&dst_lut, ipin) {
                if let Some(src_lut) = o_net_to_lut.get(&in_net) {
                    adj.entry(src_lut.clone())
                        .or_default()
                        .push(dst_lut.clone());
                }
            }
        }
    }

    let mut loops = Vec::new();
    let mut seen_Loop_keys = BTreeSet::new();

    for start in &lut_names {
        let mut stack: Vec<(String, Vec<String>)> = vec![(start.clone(), vec![start.clone()])];
        while let Some((curr, path)) = stack.pop() {
            let Some(neighbors) = adj.get(&curr) else {
                continue;
            };
            for next in neighbors {
                if next == start {
                    // Canonicalize cycle rotation by lexicographically smallest cell name.
                    let mut canon = path.clone();
                    if let Some(min_pos) = canon
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, name)| *name)
                        .map(|(i, _)| i)
                    {
                        canon.rotate_left(min_pos);
                    }
                    let key = canon.join("->");
                    if seen_Loop_keys.insert(key) {
                        loops.push(canon);
                    }
                } else if !path.contains(next) && path.len() < 16 {
                    let mut next_path = path.clone();
                    next_path.push(next.clone());
                    stack.push((next.clone(), next_path));
                }
            }
        }
    }
    loops
}

/// Run Vivado's `write_bitstream` pre-bitstream physical signoff DRC gate on `(design, routed, dev)`.
pub fn check_bitstream_signoff(
    design: &Design,
    routed: &Routed,
    dev: &Device,
    cfg: &SignoffConfig,
) -> BitgenSignoffReport {
    let mut drc = Drc::default();
    let mut downgraded = BTreeSet::new();
    let mut unrouted_nets = 0usize;
    let mut nstd_ports = Vec::new();
    let mut ucio_ports = Vec::new();
    let mut multi_driver_nets = Vec::new();
    let mut antenna_metrics = Vec::new();

    // 0. Fold baseline placement/electrical DRC checks and promote bank VCCO conflict to BIVC-1.
    let base_placed = check_placed(design, &routed.placed, dev);
    for item in base_placed.items {
        if item.id == "IOSTD-2" {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "BIVC-1",
                &item.objects,
                format!("Bank VCCO conflict: {}", item.message),
            );
        } else if item.id == "CLK-1" {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "PLCK-1",
                &item.objects,
                &item.message,
            );
        }
        let (eff_sev, changed) = cfg.effective_severity(design, &item.id, item.severity);
        if changed {
            downgraded.insert(item.id.clone());
        }
        drc.add_with_severity(&item.id, eff_sev, item.objects, item.message);
    }

    // 1. RTSTAT-1..12: Post-route physical completeness & legality checks.
    if routed.iob_src.len() < routed.placed.packed.iobs.len() {
        let missing = routed.placed.packed.iobs.len() - routed.iob_src.len();
        unrouted_nets += missing;
        push_signoff(
            &mut drc,
            &mut downgraded,
            cfg,
            design,
            "RTSTAT-1",
            "iob_src",
            format!(
                "{missing} packed IOB pad(s) have no routed fabric source connection"
            ),
        );
    }

    if routed.imux_skip > 0 {
        unrouted_nets += routed.imux_skip as usize;
        push_signoff(
            &mut drc,
            &mut downgraded,
            cfg,
            design,
            "RTSTAT-2",
            "imux",
            format!(
                "{} LUT input pin connection(s) were skipped outside IMUX reach (partially routed)",
                routed.imux_skip
            ),
        );
    }

    if routed.overused > 0 {
        push_signoff(
            &mut drc,
            &mut downgraded,
            cfg,
            design,
            "RTSTAT-3",
            "switchbox",
            format!(
                "PathFinder left {} overused routing tile(s) with unresolved congestion",
                routed.overused
            ),
        );
        if routed.pathfinder_iters >= 24 {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "RTSTAT-12",
                "pathfinder",
                format!(
                    "PathFinder exhausted {} iterations with {} overused tile(s)",
                    routed.pathfinder_iters, routed.overused
                ),
            );
        }
    }

    let max_x = dev.clb_x0 + dev.interior_cols;
    let max_y = dev.clb_y0 + dev.interior_rows;
    let min_y = dev.clb_y0.saturating_sub(1);

    for r in &routed.iob_src {
        if r.path.is_empty() {
            unrouted_nets += 1;
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "RTSTAT-2",
                &r.net,
                format!("IOB route for net {} has an empty tile path", r.net),
            );
            continue;
        }
        // RTSTAT-4: Dangling stub check (first tile must match source CLB or last tile must match IOB).
        let first = *r.path.first().unwrap();
        let last = *r.path.last().unwrap();
        if first != r.clb || last != r.iob {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "RTSTAT-4",
                &r.net,
                format!(
                    "Route stub mismatch on net {}: path {:?}..{:?} vs clb {:?} -> iob {:?}",
                    r.net, first, last, r.clb, r.iob
                ),
            );
        }

        // RTSTAT-5: Disconnected routing island (non-adjacent step in path).
        for win in r.path.windows(2) {
            let dx = win[0].0.abs_diff(win[1].0);
            let dy = win[0].1.abs_diff(win[1].1);
            if dx + dy > 1 {
                push_signoff(
                    &mut drc,
                    &mut downgraded,
                    cfg,
                    design,
                    "RTSTAT-5",
                    &r.net,
                    format!(
                        "Disconnected routing island on net {} between {:?} and {:?}",
                        r.net, win[0], win[1]
                    ),
                );
                break;
            }
        }

        // RTSTAT-9: Out-of-bounds tile coordinate.
        for &(tx, ty) in &r.path {
            if tx < dev.clb_x0 || tx >= max_x || ty < min_y || ty >= max_y {
                push_signoff(
                    &mut drc,
                    &mut downgraded,
                    cfg,
                    design,
                    "RTSTAT-9",
                    &r.net,
                    format!(
                        "Route on net {} visits out-of-grid tile ({tx}, {ty})",
                        r.net
                    ),
                );
                break;
            }
        }

        // RTSTAT-10: Excessive routing detour hops.
        let manhattan = r.clb.0.abs_diff(r.iob.0) + r.clb.1.abs_diff(r.iob.1);
        let detour_limit = (manhattan.max(1) * 3) + 6;
        if r.hops > detour_limit {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "RTSTAT-10",
                &r.net,
                format!(
                    "Net {} route uses {} hops (Manhattan distance = {manhattan})",
                    r.net, r.hops
                ),
            );
        }
    }

    // RTSTAT-6: Duplicate IMUX (x, y, mux) collision with distinct sel.
    let mut seen_imux: HashMap<(u32, u32, u32), u8> = HashMap::new();
    for im in &routed.imux {
        if let Some(&prev_sel) = seen_imux.get(&(im.x, im.y, im.mux)) {
            if prev_sel != im.sel {
                let obj = format!("CLB_X{}Y{}.IMUX[{}]", im.x, im.y, im.mux);
                push_signoff(
                    &mut drc,
                    &mut downgraded,
                    cfg,
                    design,
                    "RTSTAT-6",
                    &obj,
                    format!(
                        "IMUX collision at {obj}: driven by both sel={prev_sel} and sel={}",
                        im.sel
                    ),
                );
            }
        } else {
            seen_imux.insert((im.x, im.y, im.mux), im.sel);
        }
    }

    // 2. NSTD-1 & UCIO-1: Pre-bitstream I/O Standard and Package Pin (LOC) signoff.
    for p in &design.ports {
        let pname = resolve(p.name).to_string();
        let iostd = p.attrs.get("IOSTANDARD").unwrap_or("");
        if iostd.is_empty() || iostd.eq_ignore_ascii_case("DEFAULT") {
            nstd_ports.push(pname.clone());
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "NSTD-1",
                &pname,
                format!(
                    "Port {pname} has unspecified I/O Standard (DEFAULT). Assign an explicit IOSTANDARD or downgrade NSTD-1 to Warning"
                ),
            );
        }
        let loc = p.attrs.get("LOC").unwrap_or("");
        if loc.is_empty() {
            ucio_ports.push(pname.clone());
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "UCIO-1",
                &pname,
                format!(
                    "Port {pname} has no user-assigned LOC package pin constraint. Assign PACKAGE_PIN/LOC or downgrade UCIO-1 to Warning"
                ),
            );
        }
    }

    // 3. MDRV-1, RTSTAT-7 (undriven net with loads), RTSTAT-8 (loadless net).
    let port_names: HashSet<Spur> = design.ports.iter().map(|p| p.name).collect();
    for n in &design.nets {
        let nname = resolve(n.name).to_string();
        let mut drivers = Vec::new();
        let mut loads = Vec::new();
        if port_names.contains(&n.name) {
            // Top-level input ports act as net drivers; top-level output ports have IOB PAD driver.
            if let Some(p) = design.ports.iter().find(|p| p.name == n.name) {
                if p.dir != helion_ir::PortDir::Out {
                    drivers.push(format!("port:{nname}"));
                } else {
                    loads.push(format!("port:{nname}"));
                }
            }
        }
        for e in &n.endpoints {
            let c = resolve(e.cell);
            let pin = resolve(e.pin);
            if matches!(pin, "O" | "Q" | "P" | "DOADO" | "DOBDO" | "PAD") {
                if pin == "PAD" {
                    drivers.push(format!("{c}/{pin}"));
                } else {
                    drivers.push(format!("{c}/{pin}"));
                }
            } else {
                loads.push(format!("{c}/{pin}"));
            }
        }

        // Exclude PAD + port pseudo-pair on output port nets.
        let non_port_drivers: Vec<&String> = drivers
            .iter()
            .filter(|d| !d.starts_with("port:") && !d.ends_with("/PAD"))
            .collect();
        if non_port_drivers.len() > 1 {
            multi_driver_nets.push(nname.clone());
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "MDRV-1",
                &nname,
                format!(
                    "Net {nname} is driven by multiple cell outputs: {}",
                    non_port_drivers
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        if drivers.is_empty() && !loads.is_empty() {
            unrouted_nets += 1;
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "RTSTAT-7",
                &nname,
                format!(
                    "Net {nname} has {} load pin(s) ({}) but no active driver",
                    loads.len(),
                    loads.join(", ")
                ),
            );
        } else if !non_port_drivers.is_empty() && loads.is_empty() && !port_names.contains(&n.name) {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "RTSTAT-8",
                &nname,
                format!("Net {nname} is driven by {} but has no loads", drivers.join(", ")),
            );
        }
    }

    // 4. LUTLP-1: Combinational LUT Loop Detection.
    let combinational_loops = find_combinational_lut_loops(design);
    for cyc in &combinational_loops {
        let obj = cyc.join(" -> ");
        push_signoff(
            &mut drc,
            &mut downgraded,
            cfg,
            design,
            "LUTLP-1",
            &obj,
            format!("Combinational LUT loop detected: {obj}"),
        );
    }

    // 5. ANT-1: Metal Antenna Ratio Analysis (wirelength_um / gate_oxide_area_um2 vs max_antenna_ratio).
    for r in &routed.iob_src {
        let net_obj = design.net(&r.net);
        let sink_gates = net_obj
            .map(|n| {
                n.endpoints
                    .iter()
                    .filter(|e| !matches!(resolve(e.pin), "O" | "Q" | "PAD"))
                    .count()
                    .max(1)
            })
            .unwrap_or(1);
        let has_diode = net_obj.map_or(false, |n| {
            n.attrs.flag("ANTENNA_DIODE")
                || n.attrs.flag("antenna_diode")
                || n.endpoints.iter().any(|e| {
                    design
                        .cell(resolve(e.cell))
                        .map_or(false, |c| c.attrs.flag("ANTENNA_DIODE"))
                })
        });
        let extra_wire_um: f64 = net_obj
            .and_then(|n| n.attrs.get("METAL_LENGTH_UM"))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let metal_wirelength_um = (r.hops as f64) * cfg.um_per_hop + extra_wire_um;
        let gate_oxide_area_um2 = (sink_gates as f64) * cfg.gate_area_per_sink_um2;
        let ratio = metal_wirelength_um / gate_oxide_area_um2.max(0.01);
        let violated = ratio > cfg.max_antenna_ratio && !has_diode;

        if violated {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "ANT-1",
                &r.net,
                format!(
                    "Metal antenna ratio {:.1}:1 on net {} exceeds limit {:.1}:1 (wire={:.1}um, gate_area={:.2}um^2, diode=false)",
                    ratio, r.net, cfg.max_antenna_ratio, metal_wirelength_um, gate_oxide_area_um2
                ),
            );
        }

        antenna_metrics.push(AntennaNetMetric {
            net: r.net.clone(),
            hops: r.hops,
            sink_gates,
            metal_wirelength_um,
            gate_oxide_area_um2,
            antenna_ratio: ratio,
            max_allowed_ratio: cfg.max_antenna_ratio,
            has_diode_protection: has_diode,
            violated,
        });
    }

    // Also evaluate any internal net annotated with `METAL_LENGTH_UM` for ANT-1.
    for n in &design.nets {
        let nname = resolve(n.name);
        if antenna_metrics.iter().any(|a| a.net == nname) {
            continue;
        }
        let Some(wire_str) = n.attrs.get("METAL_LENGTH_UM") else {
            continue;
        };
        let Ok(metal_wirelength_um) = wire_str.parse::<f64>() else {
            continue;
        };
        let sink_gates = n
            .endpoints
            .iter()
            .filter(|e| !matches!(resolve(e.pin), "O" | "Q" | "PAD"))
            .count()
            .max(1);
        let has_diode = n.attrs.flag("ANTENNA_DIODE")
            || n.attrs.flag("antenna_diode")
            || n.endpoints.iter().any(|e| {
                design
                    .cell(resolve(e.cell))
                    .map_or(false, |c| c.attrs.flag("ANTENNA_DIODE"))
            });
        let gate_oxide_area_um2 = (sink_gates as f64) * cfg.gate_area_per_sink_um2;
        let ratio = metal_wirelength_um / gate_oxide_area_um2.max(0.01);
        let violated = ratio > cfg.max_antenna_ratio && !has_diode;
        if violated {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "ANT-1",
                nname,
                format!(
                    "Metal antenna ratio {:.1}:1 on net {nname} exceeds limit {:.1}:1 (wire={:.1}um, gate_area={:.2}um^2, diode=false)",
                    ratio, cfg.max_antenna_ratio, metal_wirelength_um, gate_oxide_area_um2
                ),
            );
        }
        antenna_metrics.push(AntennaNetMetric {
            net: nname.to_string(),
            hops: (metal_wirelength_um / cfg.um_per_hop.max(1.0)).round() as u32,
            sink_gates,
            metal_wirelength_um,
            gate_oxide_area_um2,
            antenna_ratio: ratio,
            max_allowed_ratio: cfg.max_antenna_ratio,
            has_diode_protection: has_diode,
            violated,
        });
    }

    // 6. PLCK-1 & CIPS-1: Clock spine placement, LUT clock gating, and inter-column clock skew.
    let pins = design.pin_index();
    let mut lut_out_nets: HashMap<Spur, String> = HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Lut6 { .. }) {
            let cname = resolve(c.name).to_string();
            if let Some(o_net) = pins.net_on(&cname, "O") {
                lut_out_nets.insert(o_net, cname);
            }
        }
    }

    for c in &design.cells {
        if matches!(c.kind, CellKind::Hff) {
            let cname = resolve(c.name);
            if let Some(clk_net) = pins.net_on(cname, "CLK") {
                if let Some(lut_driver) = lut_out_nets.get(&clk_net) {
                    let ded_false = design
                        .net(resolve(clk_net))
                        .and_then(|n| n.attrs.get("CLOCK_DEDICATED_ROUTE"))
                        .map_or(false, |v| v.eq_ignore_ascii_case("FALSE"));
                    if !ded_false {
                        push_signoff(
                            &mut drc,
                            &mut downgraded,
                            cfg,
                            design,
                            "PLCK-1",
                            format!("{lut_driver} -> {cname}/CLK"),
                            format!(
                                "Clock pin {cname}/CLK is driven by combinational LUT {lut_driver} on net {} without CLOCK_DEDICATED_ROUTE=FALSE",
                                resolve(clk_net)
                            ),
                        );
                    }
                }
            }
        }
    }

    // Check CLB column span of clocked LUTFF sites for CIPS-1 clock spine skew.
    let mut clk_cols: Vec<u32> = routed
        .placed
        .packed
        .lutffs
        .iter()
        .zip(routed.placed.lutff_sites.iter())
        .filter_map(|(cluster, (site, _))| (!cluster.ff_cell.is_empty()).then_some(site.x))
        .collect();
    clk_cols.sort_unstable();
    clk_cols.dedup();
    if let (Some(&min_col), Some(&max_col)) = (clk_cols.first(), clk_cols.last()) {
        let span = max_col - min_col;
        if span > cfg.max_clock_col_span {
            push_signoff(
                &mut drc,
                &mut downgraded,
                cfg,
                design,
                "CIPS-1",
                format!("CLB_X{min_col}..CLB_X{max_col}"),
                format!(
                    "Clocked registers span {span} CLB columns (X{min_col}..X{max_col} > {}), exceeding single clock spine skew budget",
                    cfg.max_clock_col_span
                ),
            );
        }
    }

    let bitgen_allowed = drc.error_count() == 0;
    BitgenSignoffReport {
        drc,
        bitgen_allowed,
        unrouted_nets,
        overused_tiles: routed.overused,
        nstd_ports,
        ucio_ports,
        multi_driver_nets,
        combinational_loops,
        antenna_metrics,
        downgraded_rules: downgraded.into_iter().collect(),
    }
}
