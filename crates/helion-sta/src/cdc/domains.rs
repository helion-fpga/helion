//! Clock-domain extraction for structural CDC analysis.
//!
//! Maps every `Hff` in the design to the SDC clock domain driving its `CLK`
//! pin. FFs on clock nets with no matching `create_clock` land in a shared
//! `Unknown` domain per net (never silently assigned).

use helion_ir::{intern, resolve, CellKind, Design};
use std::collections::HashMap;

use crate::{Clock, Constraints};

/// Origin of a clock domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainKind {
    /// `create_clock`.
    Primary,
    /// `create_generated_clock`.
    Generated,
    /// Clock net with no SDC clock; classification is impossible.
    Unknown,
}

/// One clock domain: an SDC clock (or an unconstrained clock net).
#[derive(Clone, Debug)]
pub struct ClockDomain {
    /// Clock name, or `unknown:<net>` for unconstrained nets.
    pub name: String,
    /// Netlist net driven by the clock.
    pub net: String,
    pub period_ps: u64,
    pub kind: DomainKind,
    /// Master clock name for generated clocks.
    pub master: Option<String>,
}

/// Result of [`extract_domains`].
#[derive(Clone, Debug, Default)]
pub struct DomainMap {
    pub domains: Vec<ClockDomain>,
    /// FF cell name -> index into `domains`.
    pub ff_domain: HashMap<String, usize>,
}

impl DomainMap {
    /// Domain index driving `ff`'s `CLK`, if the FF was mapped.
    pub fn domain_of_ff(&self, ff: &str) -> Option<usize> {
        self.ff_domain.get(ff).copied()
    }

    /// Domain with this clock net, if any.
    pub fn domain_of_net(&self, net: &str) -> Option<usize> {
        self.domains.iter().position(|d| d.net == net)
    }
}

/// Resolve the netlist net an SDC clock drives.
///
/// Handles `port/pin`, `cell/Q`, and bare port/net names. Moved verbatim
/// from `helion-sta/src/lib.rs` (was `clock_net_name`).
pub(crate) fn clock_net_name(design: &Design, clk: &Clock) -> String {
    if let Some((cell, pin)) = clk.source.split_once('/') {
        if let Some(n) = design.net_on(intern(cell), intern(pin)) {
            return resolve(n).to_string();
        }
    }
    if let Some(n) = design.net_on(intern(&clk.source), intern("Q")) {
        return resolve(n).to_string();
    }
    clk.source.clone()
}

/// Number of `Hff` `CLK` loads on `net`. Moved verbatim from
/// `helion-sta/src/lib.rs` (was `clock_ff_loads`).
pub(crate) fn clock_ff_loads(design: &Design, net: &str) -> usize {
    design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff) && design.net_on(c.name, intern("CLK")) == Some(intern(net)))
        .count()
}

/// Build the clock-domain map for `design` from `clocks` (falling back to
/// the XDC clocks when the slice is empty, mirroring `report_cdc`).
pub fn extract_domains(design: &Design, clocks: &[Clock], xdc: &Constraints) -> DomainMap {    let clocks: &[Clock] = if clocks.is_empty() {
        xdc.clocks.as_slice()
    } else {
        clocks
    };
    let mut map = DomainMap::default();
    // One Unknown domain per distinct unconstrained clock net.
    let mut unknown: HashMap<String, usize> = HashMap::new();
    for clk in clocks {
        let net = clock_net_name(design, clk);
        map.domains.push(ClockDomain {
            name: clk.name.clone(),
            net,
            period_ps: clk.period_ps,
            kind: if clk.generated {
                DomainKind::Generated
            } else {
                DomainKind::Primary
            },
            master: clk.master.clone(),
        });
    }
    for cell in &design.cells {
        if !matches!(cell.kind, CellKind::Hff) {
            continue;
        }
        let Some(clk_net) = design.net_on(cell.name, intern("CLK")) else {
            continue;
        };
        let idx = match map.domain_of_net(resolve(clk_net)) {
            Some(i) => i,
            None => *unknown.entry(resolve(clk_net).to_string()).or_insert_with(|| {
                let i = map.domains.len();
                map.domains.push(ClockDomain {
                    name: format!("unknown:{}", resolve(clk_net)),
                    net: resolve(clk_net).to_string(),
                    period_ps: 0,
                    kind: DomainKind::Unknown,
                    master: None,
                });
                i
            }),
        };
        map.ff_domain.insert(resolve(cell.name).to_string(), idx);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_clock, create_generated_clock, CdcSeverity};
    use helion_ir::PortDir;

    fn ff_on(d: &mut Design, name: &str, clk_net: &str) {
        d.add_cell(name, CellKind::Hff);
        d.connect(clk_net, name, "CLK");
        d.connect(&format!("{name}_d"), name, "D");
        d.connect(&format!("{name}_q"), name, "Q");
    }

    #[test]
    fn two_clocks_and_unknown() {
        let mut d = Design::new("t");
        for p in ["clk_a", "clk_b", "clk_x"] {
            d.add_port(p, PortDir::In);
        }
        ff_on(&mut d, "fa", "clk_a");
        ff_on(&mut d, "fb", "clk_b");
        ff_on(&mut d, "fx", "clk_x");
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk_a", 10_000, "clk_a");
        create_clock(&mut clks, "clk_b", 8_000, "clk_b");
        let m = extract_domains(&d, &clks, &Constraints::default());
        let dom_name = |ff: &str| {
            m.domain_of_ff(ff)
                .map(|i| m.domains[i].name.clone())
        };
        assert_eq!(dom_name("fa").as_deref(), Some("clk_a"));
        assert_eq!(dom_name("fb").as_deref(), Some("clk_b"));
        let ux = m.domain_of_ff("fx").expect("fx mapped");
        assert_eq!(m.domains[ux].kind, DomainKind::Unknown);
        assert_eq!(m.domains[ux].name, "unknown:clk_x");
        // One Unknown domain shared per net, not per FF.
        assert_eq!(
            m.domains.iter().filter(|x| x.kind == DomainKind::Unknown).count(),
            1
        );
    }

    #[test]
    fn unconstrained_clock_net_warns() {
        let mut d = Design::new("t");
        for p in ["clk_a", "clk_x"] {
            d.add_port(p, PortDir::In);
        }
        ff_on(&mut d, "fa", "clk_a");
        ff_on(&mut d, "fx0", "clk_x");
        ff_on(&mut d, "fx1", "clk_x");
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk_a", 10_000, "clk_a");
        let r = crate::report_cdc(&clks, &Constraints::default(), None, Some(&d));
        let warns: Vec<_> = r
            .crossings
            .iter()
            .filter(|c| {
                c.class == super::super::crossings::CdcClass::UnconstrainedClockNet
            })
            .collect();
        assert_eq!(warns.len(), 1, "{}", r.text());
        assert_eq!(warns[0].severity, CdcSeverity::Warning);
        assert_eq!(warns[0].check, "HELION-CDC-7");
        assert_eq!(warns[0].load_cells.len(), 2);
    }

    #[test]
    fn generated_clock_same_tree_no_crossing() {
        let mut d = Design::new("t");
        d.add_port("clk", PortDir::In);
        // Divider FF on clk; its Q is the generated clock's source.
        d.add_cell("div_ff", CellKind::Hff);
        d.connect("clk", "div_ff", "CLK");
        d.connect("div_d", "div_ff", "D");
        d.connect("div_q", "div_ff", "Q");
        // Data path: src (clk) -> dst (clkdiv).
        ff_on(&mut d, "src", "clk");
        d.add_cell("dst", CellKind::Hff);
        d.connect("div_q", "dst", "CLK");
        d.connect("src_q", "dst", "D");
        d.connect("dst_q", "dst", "Q");
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        create_generated_clock(&mut clks, "clkdiv", "clk", 2, "div_ff/Q").unwrap();
        let m = extract_domains(&d, &clks, &Constraints::default());
        let dom_name =
            |ff: &str| m.domain_of_ff(ff).map(|i| m.domains[i].name.clone());
        assert_eq!(dom_name("src").as_deref(), Some("clk"));
        assert_eq!(dom_name("dst").as_deref(), Some("clkdiv"));
        assert_ne!(
            m.domain_of_ff("src"),
            m.domain_of_ff("dst"),
            "generated clock is its own domain"
        );
        // The declared generated/master pair is synchronous: no crossing, and
        // the pair-level row stays CDC-0 Safe.
        let r = crate::report_cdc(&clks, &Constraints::default(), None, Some(&d));
        assert!(r.crossings.is_empty(), "{}", r.text());
        let pair = r.violation("clk", "clkdiv").expect("pair row");
        assert_eq!(pair.check, "CDC-0");
        assert_eq!(pair.severity, CdcSeverity::Safe);
    }
}
