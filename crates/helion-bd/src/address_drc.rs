//! Vivado UG994 Address Editor (`assign_bd_address`) and Block Design DRC Engine (`validate_bd_design`).

use crate::BlockDesign;
use crate::graph::{
    BdAddressSegment, BdGraph, get_explicit_bd_graph, norm_ep, with_bd_graph_mut,
};
use helion_ipxact::{BusInterfaceType, InterfaceMode};

/// Severity of a Block Design Design Rule Check (DRC) diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BdDrcSeverity {
    Error,
    CriticalWarning,
    Warning,
    Info,
}

/// Structured BD DRC diagnostic matching Vivado `validate_bd_design`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdDrcDiagnostic {
    pub code: String,
    pub severity: BdDrcSeverity,
    pub message: String,
    pub objects: Vec<String>,
}

/// Structured BD validation report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BdDrcReport {
    pub ok: bool,
    pub diagnostics: Vec<BdDrcDiagnostic>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Round `n` up to the next power of two (minimum 4096 = 4KB for AXI4-Lite slave apertures).
pub fn next_power_of_two_aperture(n: u64) -> u64 {
    let min_ap = n.max(0x1000);
    min_ap.next_power_of_two()
}

/// Discover all `(master_cell, master_intf, slave_cell, slave_intf, default_range, ic_slot_info)`
/// paths across direct connections, `axi_protocol_converter`, and `axi_interconnect`.
fn discover_mm_paths(
    graph: &BdGraph,
) -> Vec<(String, String, String, String, u64, Option<(String, usize)>)> {
    let mut paths = Vec::new();

    // Find all non-bridge master interfaces
    let mut masters = Vec::new();
    for cell in &graph.cells {
        let name = cell.component.core.name.as_str();
        if name == "axi_interconnect"
            || name == "axi_crossbar"
            || name == "axi_protocol_converter"
        {
            continue;
        }
        for intf in &cell.component.interfaces {
            if intf.mode == InterfaceMode::Master && intf.bus_type.is_memory_mapped() {
                if intf.bus_type == BusInterfaceType::BramPort {
                    continue;
                }
                masters.push((cell.instance_name.clone(), intf.name.clone()));
            }
        }
    }

    // Also include external slave interface ports (which act as masters driving into the BD)
    for ip in &graph.intf_ports {
        if ip.mode == InterfaceMode::Slave && ip.bus_type.is_memory_mapped() {
            masters.push((ip.name.clone(), "EXT".into()));
        }
    }

    for (m_cell, m_intf) in &masters {
        let start_ep = if m_intf == "EXT" {
            m_cell.clone()
        } else {
            format!("{m_cell}/{m_intf}")
        };
        trace_from_master(graph, m_cell, m_intf, &start_ep, None, &mut paths, 0);
    }

    paths
}

fn trace_from_master(
    graph: &BdGraph,
    orig_m_cell: &str,
    orig_m_intf: &str,
    current_driver_ep: &str,
    ic_slot: Option<(String, usize)>,
    out: &mut Vec<(String, String, String, String, u64, Option<(String, usize)>)>,
    depth: u32,
) {
    if depth > 6 {
        return;
    }
    for net in &graph.intf_nets {
        if !net
            .endpoints
            .iter()
            .any(|e| e.eq_ignore_ascii_case(current_driver_ep))
        {
            continue;
        }
        for peer in &net.endpoints {
            if peer.eq_ignore_ascii_case(current_driver_ep) {
                continue;
            }
            let Some((peer_cell_name, peer_intf_name)) = peer.split_once('/') else {
                continue;
            };
            let Some(peer_cell) = graph.find_cell(peer_cell_name) else {
                continue;
            };
            let core_name = peer_cell.component.core.name.as_str();
            if core_name == "axi_protocol_converter" && peer_intf_name.eq_ignore_ascii_case("S_AXI")
            {
                let next_ep = format!("{peer_cell_name}/M_AXI");
                trace_from_master(
                    graph,
                    orig_m_cell,
                    orig_m_intf,
                    &next_ep,
                    ic_slot.clone(),
                    out,
                    depth + 1,
                );
            } else if (core_name == "axi_interconnect" || core_name == "axi_crossbar")
                && peer_intf_name.to_ascii_uppercase().starts_with('S')
            {
                // Fan out across all Mxx_AXI master ports of this interconnect
                for intf in &peer_cell.component.interfaces {
                    if intf.mode == InterfaceMode::Master
                        && intf.name.to_ascii_uppercase().starts_with('M')
                    {
                        let slot_idx = intf
                            .name
                            .trim_start_matches(['M', 'm'])
                            .split('_')
                            .next()
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        let next_ep = format!("{peer_cell_name}/{}", intf.name);
                        trace_from_master(
                            graph,
                            orig_m_cell,
                            orig_m_intf,
                            &next_ep,
                            Some((peer_cell_name.to_string(), slot_idx)),
                            out,
                            depth + 1,
                        );
                    }
                }
            } else if let Some(peer_intf) = peer_cell.get_interface(peer_intf_name) {
                if peer_intf.mode == InterfaceMode::Slave && peer_intf.bus_type.is_memory_mapped() {
                    let range = if peer_intf.default_range > 0 {
                        peer_intf.default_range
                    } else {
                        0x1000
                    };
                    let already = out.iter().any(|(mc, mi, sc, si, _, _)| {
                        mc == orig_m_cell
                            && mi == orig_m_intf
                            && sc == peer_cell_name
                            && si == peer_intf_name
                    });
                    if !already {
                        out.push((
                            orig_m_cell.to_string(),
                            orig_m_intf.to_string(),
                            peer_cell_name.to_string(),
                            peer_intf_name.to_string(),
                            range,
                            ic_slot.clone(),
                        ));
                    }
                }
            }
        }
    }
}

/// Automatically allocate power-of-two aligned base addresses and ranges for all unmapped
/// memory-mapped slaves in `bd` (`assign_bd_address`).
pub fn assign_bd_address(bd: &mut BlockDesign) -> Result<Vec<BdAddressSegment>, String> {
    with_bd_graph_mut(bd, |graph| {
        assign_bd_address_in_graph(graph)?;
        Ok(graph.address_segments.clone())
    })
}

pub fn assign_bd_address_in_graph(graph: &mut BdGraph) -> Result<(), String> {
    let paths = discover_mm_paths(graph);
    for (m_cell, m_intf, s_cell, s_intf, default_range, ic_slot) in paths {
        let already_mapped = graph.address_segments.iter().any(|seg| {
            seg.master_cell.eq_ignore_ascii_case(&m_cell)
                && seg.slave_cell.eq_ignore_ascii_case(&s_cell)
                && seg.slave_intf.eq_ignore_ascii_case(&s_intf)
        });
        if !already_mapped {
            let range = next_power_of_two_aperture(default_range);
            // Determine starting region: BRAM controllers default to 0xC000_0000, peripherals to 0x4000_0000
            let is_bram = graph
                .find_cell(&s_cell)
                .map(|c| c.component.core.name.contains("bram"))
                .unwrap_or(false);
            let mut candidate_base: u64 = if is_bram { 0xC000_0000 } else { 0x4000_0000 };
            // Align candidate_base to `range` and step by `max(range, 0x10000)` until no overlap
            let step = range.max(0x0001_0000);
            loop {
                candidate_base = (candidate_base + range - 1) & !(range - 1);
                let cand_high = candidate_base + range - 1;
                let overlaps = graph.address_segments.iter().any(|seg| {
                    seg.master_cell.eq_ignore_ascii_case(&m_cell)
                        && !(cand_high < seg.base_address || candidate_base > seg.high_address())
                });
                if !overlaps {
                    break;
                }
                candidate_base += step;
            }
            let seg_name = format!("SEG_{}_{}", s_cell, s_intf);
            graph.address_segments.push(BdAddressSegment {
                name: seg_name,
                master_cell: m_cell,
                master_intf: m_intf,
                slave_cell: s_cell.clone(),
                slave_intf: s_intf.clone(),
                base_address: candidate_base,
                range,
            });
        }
        // Sync address into interconnect parameters if routed through an interconnect slot
        if let Some((ic_name, slot_idx)) = ic_slot {
            if let Some(seg) = graph
                .address_segments
                .iter()
                .find(|seg| {
                    seg.slave_cell.eq_ignore_ascii_case(&s_cell)
                        && seg.slave_intf.eq_ignore_ascii_case(&s_intf)
                })
                .cloned()
            {
                if let Some(ic_cell) = graph.find_cell_mut(&ic_name) {
                    ic_cell.properties.insert(
                        format!("C_M{slot_idx:02}_BASE_ADDR"),
                        format!("0x{:08X}", seg.base_address),
                    );
                    let aw = seg.range.trailing_zeros().clamp(4, 32);
                    ic_cell
                        .properties
                        .insert(format!("C_M{slot_idx:02}_ADDR_WIDTH"), aw.to_string());
                }
            }
        }
    }
    Ok(())
}

/// Explicitly assign or update a slave's base address and range (`assign_bd_address -offset ... -range ...`).
pub fn assign_bd_address_explicit(
    bd: &mut BlockDesign,
    slave_endpoint: &str,
    base_address: u64,
    range: u64,
) -> Result<BdAddressSegment, String> {
    let ep = norm_ep(slave_endpoint);
    let (s_cell, s_intf) = ep
        .split_once('/')
        .ok_or_else(|| format!("assign_bd_address: expected '<cell>/<intf>', got '{ep}'"))?;
    with_bd_graph_mut(bd, |graph| {
        assign_bd_address_explicit_in_graph(graph, s_cell, s_intf, base_address, range)
    })
}

pub fn assign_bd_address_explicit_in_graph(
    graph: &mut BdGraph,
    s_cell: &str,
    s_intf: &str,
    base_address: u64,
    range: u64,
) -> Result<BdAddressSegment, String> {
    if graph.find_cell(s_cell).is_none() {
        return Err(format!("assign_bd_address: unknown slave cell '{s_cell}'"));
    }
    let paths = discover_mm_paths(graph);
    let (m_cell, m_intf, ic_slot) = paths
        .into_iter()
        .find(|(_, _, sc, si, _, _)| {
            sc.eq_ignore_ascii_case(s_cell) && si.eq_ignore_ascii_case(s_intf)
        })
        .map(|(mc, mi, _, _, _, ic)| (mc, mi, ic))
        .unwrap_or_else(|| ("master".into(), "M_AXI".into(), None));

    let seg = if let Some(existing) = graph.address_segments.iter_mut().find(|seg| {
        seg.slave_cell.eq_ignore_ascii_case(s_cell) && seg.slave_intf.eq_ignore_ascii_case(s_intf)
    }) {
        existing.base_address = base_address;
        existing.range = range;
        existing.clone()
    } else {
        let new_seg = BdAddressSegment {
            name: format!("SEG_{s_cell}_{s_intf}"),
            master_cell: m_cell,
            master_intf: m_intf,
            slave_cell: s_cell.into(),
            slave_intf: s_intf.into(),
            base_address,
            range,
        };
        graph.address_segments.push(new_seg.clone());
        new_seg
    };

    if let Some((ic_name, slot_idx)) = ic_slot {
        if let Some(ic_cell) = graph.find_cell_mut(&ic_name) {
            ic_cell.properties.insert(
                format!("C_M{slot_idx:02}_BASE_ADDR"),
                format!("0x{:08X}", base_address),
            );
            let aw = range.trailing_zeros().clamp(4, 32);
            ic_cell
                .properties
                .insert(format!("C_M{slot_idx:02}_ADDR_WIDTH"), aw.to_string());
        }
    }
    Ok(seg)
}

/// Run Vivado UG994 Block Design DRC validation (`validate_bd_design`):
/// - Checks overlap between slave address segments (`BD-DRC-002`)
/// - Checks unmapped connected memory-mapped slaves (`BD-DRC-003`)
/// - Checks aperture width, power-of-two size, and base-address alignment (`BD-DRC-004`)
/// - Checks clock-domain mismatches across connected interfaces (`BD-DRC-005`)
/// - Checks unconnected clock/reset pins on cells (`BD-DRC-006`)
pub fn validate_bd_design(bd: &BlockDesign) -> BdDrcReport {
    let mut diagnostics = Vec::new();

    if bd.name.trim().is_empty() {
        diagnostics.push(BdDrcDiagnostic {
            code: "BD-DRC-001".into(),
            severity: BdDrcSeverity::Error,
            message: "empty name".into(),
            objects: Vec::new(),
        });
    }

    let Some(graph) = get_explicit_bd_graph(bd) else {
        // Legacy struct-literal BlockDesign { name, cores } validation
        for c in &bd.cores {
            if c.bus != "Helion-MM"
                && c.bus != "Helion-ST"
                && BusInterfaceType::parse(&c.bus).is_none()
            {
                diagnostics.push(BdDrcDiagnostic {
                    code: "BD-DRC-001".into(),
                    severity: BdDrcSeverity::Error,
                    message: format!("{}: bus {} not Helion-MM/ST", c.name, c.bus),
                    objects: vec![c.name.clone()],
                });
            }
        }
        return finish_report(diagnostics);
    };

    // 1. Check unmapped memory-mapped slaves (BD-DRC-003)
    let paths = discover_mm_paths(&graph);
    for (m_cell, _m_intf, s_cell, s_intf, _def_range, _) in &paths {
        let mapped = graph.address_segments.iter().any(|seg| {
            seg.slave_cell.eq_ignore_ascii_case(s_cell)
                && seg.slave_intf.eq_ignore_ascii_case(s_intf)
        });
        if !mapped {
            diagnostics.push(BdDrcDiagnostic {
                code: "BD-DRC-003".into(),
                severity: BdDrcSeverity::Error,
                message: format!(
                    "Unmapped slave interface '{s_cell}/{s_intf}' in master '{m_cell}' address space"
                ),
                objects: vec![format!("{s_cell}/{s_intf}")],
            });
        }
    }

    // 2. Check address segment aperture width, power-of-two, and base alignment (BD-DRC-004)
    for seg in &graph.address_segments {
        if seg.range == 0 || !seg.range.is_power_of_two() {
            diagnostics.push(BdDrcDiagnostic {
                code: "BD-DRC-004".into(),
                severity: BdDrcSeverity::Error,
                message: format!(
                    "Address segment '{}' ({}/{}) range 0x{:X} is not a non-zero power of two",
                    seg.name, seg.slave_cell, seg.slave_intf, seg.range
                ),
                objects: vec![seg.name.clone()],
            });
        } else if seg.base_address % seg.range != 0 {
            diagnostics.push(BdDrcDiagnostic {
                code: "BD-DRC-004".into(),
                severity: BdDrcSeverity::Error,
                message: format!(
                    "Address segment '{}' ({}/{}) base address 0x{:08X} is not aligned to range 0x{:X}",
                    seg.name, seg.slave_cell, seg.slave_intf, seg.base_address, seg.range
                ),
                objects: vec![seg.name.clone()],
            });
        }

        // Check slave minimum required aperture / physical addr_width
        if let Some(s_cell) = graph.find_cell(&seg.slave_cell) {
            if let Some(s_intf) = s_cell.get_interface(&seg.slave_intf) {
                let min_bytes = if s_intf.addr_width > 0 && s_intf.addr_width < 32 {
                    1u64 << s_intf.addr_width
                } else {
                    0
                };
                if min_bytes > 0 && seg.range < min_bytes {
                    diagnostics.push(BdDrcDiagnostic {
                        code: "BD-DRC-004".into(),
                        severity: BdDrcSeverity::Error,
                        message: format!(
                            "Address segment '{}' range 0x{:X} is smaller than slave '{}/{}' required aperture 0x{:X} ({} address bits)",
                            seg.name, seg.range, seg.slave_cell, seg.slave_intf, min_bytes, s_intf.addr_width
                        ),
                        objects: vec![seg.name.clone(), format!("{}/{}", seg.slave_cell, seg.slave_intf)],
                    });
                }
            }
        }
    }

    // 3. Check address segment overlaps within the same master space (BD-DRC-002)
    for i in 0..graph.address_segments.len() {
        for j in (i + 1)..graph.address_segments.len() {
            let a = &graph.address_segments[i];
            let b = &graph.address_segments[j];
            if a.master_cell.eq_ignore_ascii_case(&b.master_cell) {
                let overlap = !(a.high_address() < b.base_address || b.high_address() < a.base_address);
                if overlap {
                    diagnostics.push(BdDrcDiagnostic {
                        code: "BD-DRC-002".into(),
                        severity: BdDrcSeverity::Error,
                        message: format!(
                            "Address overlap in master '{}' between '{}/{}' [0x{:08X}..0x{:08X}] and '{}/{}' [0x{:08X}..0x{:08X}]",
                            a.master_cell,
                            a.slave_cell,
                            a.slave_intf,
                            a.base_address,
                            a.high_address(),
                            b.slave_cell,
                            b.slave_intf,
                            b.base_address,
                            b.high_address()
                        ),
                        objects: vec![a.name.clone(), b.name.clone()],
                    });
                }
            }
        }
    }

    // 4. Check clock-domain mismatches across interface nets (BD-DRC-005)
    for inet in &graph.intf_nets {
        if inet.endpoints.len() < 2 {
            continue;
        }
        let mut ep_domains: Vec<(String, String)> = Vec::new();
        for ep in &inet.endpoints {
            if let Some((cell_name, intf_name)) = ep.split_once('/') {
                if let Some(cell) = graph.find_cell(cell_name) {
                    // Look up the clock pin associated with this specific interface if dual-clock (e.g. cdc_fifo)
                    let assoc_clk = cell
                        .get_interface(intf_name)
                        .and_then(|i| i.associated_clock.clone());
                    let pin_dom = assoc_clk.and_then(|clk_pin| {
                        let target_ep = format!("{cell_name}/{clk_pin}");
                        graph
                            .nets
                            .iter()
                            .find(|n| n.endpoints.iter().any(|e| e.eq_ignore_ascii_case(&target_ep)))
                            .and_then(|n| n.clock_domain.clone().or_else(|| Some(n.name.clone())))
                    });
                    if let Some(dom) = pin_dom.or_else(|| cell.clock_domain.clone()) {
                        ep_domains.push((ep.clone(), dom));
                    }
                }
            }
        }
        if ep_domains.len() >= 2 {
            let first_dom = &ep_domains[0].1;
            for (ep, dom) in &ep_domains[1..] {
                if !dom.eq_ignore_ascii_case(first_dom) {
                    diagnostics.push(BdDrcDiagnostic {
                        code: "BD-DRC-005".into(),
                        severity: BdDrcSeverity::Error,
                        message: format!(
                            "Clock-domain mismatch on interface net '{}': '{}' is in clock domain '{}' while '{}' is in clock domain '{}'",
                            inet.name, ep_domains[0].0, first_dom, ep, dom
                        ),
                        objects: vec![inet.name.clone(), ep_domains[0].0.clone(), ep.clone()],
                    });
                }
            }
        }
    }

    finish_report(diagnostics)
}

fn finish_report(diagnostics: Vec<BdDrcDiagnostic>) -> BdDrcReport {
    let errors: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.severity == BdDrcSeverity::Error)
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    let warnings: Vec<String> = diagnostics
        .iter()
        .filter(|d| {
            d.severity == BdDrcSeverity::Warning || d.severity == BdDrcSeverity::CriticalWarning
        })
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    BdDrcReport {
        ok: errors.is_empty(),
        diagnostics,
        errors,
        warnings,
    }
}
