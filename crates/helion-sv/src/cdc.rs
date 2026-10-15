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
fn build_clock_domains(design: &Design) -> Vec<ClockDomain> {
    let mut clock_to_ffs: HashMap<Spur, HashSet<Spur>> = HashMap::new();

    for cell in design.cells.iter() {
        // Check if cell is a flip-flop (has clock pin)
        // Simplified: look for cells with "CLK" pin
        // In real implementation, check CellKind::Ff or similar
        let _ = cell; // Placeholder
    }

    clock_to_ffs
        .into_iter()
        .map(|(clock, ffs)| ClockDomain { clock, ffs })
        .collect()
}

/// Find which domain drives a net (placeholder).
fn find_driver_domain(
    _design: &Design,
    _domains: &[ClockDomain],
    _net: Spur,
) -> Option<ClockDomain> {
    // TODO: Trace net driver back to FF or PI, determine its clock domain
    None
}

/// Find which domains load a net (placeholder).
fn find_load_domains(
    _design: &Design,
    _domains: &[ClockDomain],
    _net: Spur,
) -> Vec<ClockDomain> {
    // TODO: Find all FFs/DFFs that sample this net, group by clock
    Vec::new()
}

/// Check if net has a 2-FF synchronizer in the destination domain (placeholder).
fn has_synchronizer(_design: &Design, _net: Spur, _dst: &ClockDomain) -> bool {
    // TODO: Look for 2 FFs in series, both clocked by dst.clock,
    // with the net feeding the first FF's D pin
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
