//! Clock Domain Crossing (CDC) analysis.
//!
//! Detects unsynchronized signals crossing clock domains.
//! Currently implements basic structural checks; full formal CDC
//! verification is future work.
//!
//! A CDC violation occurs when a signal generated in one clock domain
//! is sampled in another without proper synchronization (2-FF synchronizer,
//! handshake, or FIFO).

use helion_ir::{Design, Spur};
use std::collections::{HashMap, HashSet};

/// A clock domain: set of sequential elements clocked by the same clock.
#[derive(Clone, Debug)]
pub struct ClockDomain {
    /// Clock net name (interned).
    pub clock: Spur,
    /// Flip-flops in this domain (cell names).
    pub ffs: HashSet<Spur>,
}

/// CDC violation: unsynchronized crossing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdcViolation {
    /// Signal name crossing domains.
    pub signal: Spur,
    /// Source clock domain.
    pub src_clock: Spur,
    /// Destination clock domain.
    pub dst_clock: Spur,
    /// Violation type.
    pub kind: CdcKind,
}

/// Type of CDC violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CdcKind {
    /// No synchronizer detected (combinational path between domains).
    Unsynchronized,
    /// Single FF (needs 2 for metastability protection).
    SingleFlop,
}

/// Analyze design for CDC violations.
/// Returns list of violations (empty if clean).
pub fn analyze_cdc(design: &Design) -> Vec<CdcViolation> {
    let domains = build_clock_domains(design);
    let mut violations = Vec::new();

    // For each net, check if it crosses domains
    for net in design.nets.iter() {
        let net_name = net.name;
        let src_domain = find_driver_domain(design, &domains, net_name);
        let dst_domains = find_load_domains(design, &domains, net_name);

        if let Some(src) = src_domain {
            for dst in dst_domains {
                if src.clock != dst.clock {
                    // Clock domain crossing detected
                    // Check if there's a synchronizer (simplified: look for 2 FFs in series)
                    if !has_synchronizer(design, net_name, &dst) {
                        violations.push(CdcViolation {
                            signal: net_name,
                            src_clock: src.clock,
                            dst_clock: dst.clock,
                            kind: CdcKind::Unsynchronized,
                        });
                    }
                }
            }
        }
    }

    violations
}

/// Build clock domains from design FFs.
/// Groups flip-flops by their clock net.
fn build_clock_domains(design: &Design) -> Vec<ClockDomain> {
    use helion_ir::{intern, CellKind};

    let mut clock_to_ffs: HashMap<Spur, HashSet<Spur>> = HashMap::new();
    let clk_pin = intern("CLK");

    for cell in design.cells.iter() {
        // Check if cell is a flip-flop
        let is_ff = matches!(cell.kind, CellKind::Hff);
        if !is_ff {
            continue;
        }

        // Get the clock net for this FF
        if let Some(clk_net) = design.net_on(cell.name, clk_pin) {
            clock_to_ffs
                .entry(clk_net)
                .or_insert_with(HashSet::new)
                .insert(cell.name);
        }
    }

    clock_to_ffs
        .into_iter()
        .map(|(clock, ffs)| ClockDomain { clock, ffs })
        .collect()
}

/// Find which domain drives a net.
/// Traces the net driver back to a flip-flop output or primary input.
/// Returns None for primary inputs or combinational drivers (no domain).
fn find_driver_domain(
    design: &Design,
    domains: &[ClockDomain],
    net: Spur,
) -> Option<ClockDomain> {
    use helion_ir::{intern, CellKind};

    let q_pin = intern("Q");

    // Find the cell that drives this net (has Q connected to net)
    for cell in design.cells.iter() {
        let is_ff = matches!(cell.kind, CellKind::Hff);
        if !is_ff {
            continue;
        }
        if let Some(q_net) = design.net_on(cell.name, q_pin) {
            if q_net == net {
                // This FF drives the net. Find its domain.
                for domain in domains {
                    if domain.ffs.contains(&cell.name) {
                        return Some(domain.clone());
                    }
                }
            }
        }
    }
    // No FF driver found (could be PI or combinational)
    None
}

/// Find which domains load a net (sample it).
/// Returns list of domains for FFs whose D pin connects to the net.
fn find_load_domains(
    design: &Design,
    domains: &[ClockDomain],
    net: Spur,
) -> Vec<ClockDomain> {
    use helion_ir::{intern, CellKind};

    let d_pin = intern("D");
    let mut result = Vec::new();
    let mut seen_clocks = HashSet::new();

    for cell in design.cells.iter() {
        let is_ff = matches!(cell.kind, CellKind::Hff);
        if !is_ff {
            continue;
        }
        if let Some(d_net) = design.net_on(cell.name, d_pin) {
            if d_net == net {
                // This FF samples the net. Find its domain.
                for domain in domains {
                    if domain.ffs.contains(&cell.name) && !seen_clocks.contains(&domain.clock) {
                        seen_clocks.insert(domain.clock);
                        result.push(domain.clone());
                    }
                }
            }
        }
    }

    result
}

/// Check if net has a 2-FF synchronizer in the destination domain.
/// Looks for two flip-flops in series, both clocked by dst.clock,
/// where the first FF's D is driven by the net, and the second FF's
/// D is driven by the first FF's Q.
fn has_synchronizer(design: &Design, net: Spur, dst: &ClockDomain) -> bool {
    use helion_ir::{intern, CellKind};

    let d_pin = intern("D");
    let q_pin = intern("Q");
    let clk_pin = intern("CLK");

    // Find all FFs in the destination domain
    for cell in design.cells.iter() {
        // Check if it's a flip-flop
        let is_ff = matches!(cell.kind, CellKind::Hff);
        if !is_ff {
            continue;
        }

        // Check if it's in the destination domain (clocked by dst.clock)
        let clk_net = design.net_on(cell.name, clk_pin);
        if clk_net != Some(dst.clock) {
            continue;
        }

        // Check if this FF's D is driven by our net
        let d_net = design.net_on(cell.name, d_pin);
        if d_net != Some(net) {
            continue;
        }

        // Found first FF (FF1). Now look for second FF (FF2) whose D is driven by FF1.Q
        let ff1_q = design.net_on(cell.name, q_pin);
        if let Some(q_net) = ff1_q {
            for cell2 in design.cells.iter() {
                let is_ff2 = matches!(cell2.kind, CellKind::Hff);
                if !is_ff2 {
                    continue;
                }
                // Skip the same cell
                if cell2.name == cell.name {
                    continue;
                }
                // Check clock
                let clk2_net = design.net_on(cell2.name, clk_pin);
                if clk2_net != Some(dst.clock) {
                    continue;
                }
                // Check D input
                let d2_net = design.net_on(cell2.name, d_pin);
                if d2_net == Some(q_net) {
                    // Found 2-FF synchronizer!
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_design_no_violations() {
        let design = Design::new("empty".into());
        let violations = analyze_cdc(&design);
        assert!(violations.is_empty());
    }
}
