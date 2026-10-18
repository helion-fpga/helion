//! Crossing detection for structural CDC analysis.
//!
//! Finds candidate net-level crossings: load FFs whose `D` cone is driven
//! from a different clock domain (transitively through `Lut6`s, stopping at
//! `Hff` Q boundaries).

use helion_ir::{intern, resolve, Design};
use std::collections::HashSet;

use super::domains::{DomainKind, DomainMap};
use crate::CdcSeverity;

/// The 11 structural CDC violation classes (design doc Part C section 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CdcClass {
    UnsyncCrossing,
    SingleFfSync,
    TappedSync,
    MuxedCrossing,
    MultiBitUnsync,
    SyncReconvergence,
    HandshakeNeedsReview,
    AsyncFifoUnverified,
    UnconstrainedClockNet,
    InputCrossing,
    BlackBoxBoundary,
}

impl CdcClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsyncCrossing => "UNSYNC_CROSSING",
            Self::SingleFfSync => "SINGLE_FF_SYNC",
            Self::TappedSync => "TAPPED_SYNC",
            Self::MuxedCrossing => "MUXED_CROSSING",
            Self::MultiBitUnsync => "MULTI_BIT_UNSYNC",
            Self::SyncReconvergence => "SYNC_RECONVERGENCE",
            Self::HandshakeNeedsReview => "HANDSHAKE_NEEDS_REVIEW",
            Self::AsyncFifoUnverified => "ASYNC_FIFO_UNVERIFIED",
            Self::UnconstrainedClockNet => "UNCONSTRAINED_CLOCK_NET",
            Self::InputCrossing => "INPUT_CROSSING",
            Self::BlackBoxBoundary => "BLACKBOX_BOUNDARY",
        }
    }

    /// Vivado `CDC-*` check id where semantics align, else `HELION-CDC-*`.
    pub fn check_id(self) -> &'static str {
        match self {
            Self::UnsyncCrossing => "CDC-10",
            Self::SingleFfSync => "CDC-10",
            Self::TappedSync => "HELION-CDC-1",
            Self::MuxedCrossing => "HELION-CDC-2",
            Self::MultiBitUnsync => "HELION-CDC-3",
            Self::SyncReconvergence => "HELION-CDC-4",
            Self::HandshakeNeedsReview => "HELION-CDC-5",
            Self::AsyncFifoUnverified => "HELION-CDC-6",
            Self::UnconstrainedClockNet => "HELION-CDC-7",
            Self::InputCrossing => "CDC-15-analog",
            Self::BlackBoxBoundary => "HELION-CDC-8",
        }
    }

    pub fn severity(self) -> CdcSeverity {
        match self {
            Self::UnsyncCrossing | Self::SingleFfSync => CdcSeverity::Critical,
            Self::TappedSync
            | Self::MuxedCrossing
            | Self::MultiBitUnsync
            | Self::SyncReconvergence
            | Self::HandshakeNeedsReview
            | Self::UnconstrainedClockNet => CdcSeverity::Warning,
            Self::AsyncFifoUnverified | Self::InputCrossing | Self::BlackBoxBoundary => {
                CdcSeverity::Info
            }
        }
    }
}

/// Where a crossing's driver lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CrossingSource {
    /// Index into [`DomainMap::domains`].
    Domain(usize),
    /// Driven by a top-level input port (designer's `set_input_delay` contract).
    PortInput,
}

/// One candidate crossing: a load FF whose `D` cone reaches another domain.
#[derive(Clone, Debug)]
pub struct RawCrossing {
    /// Net on the load FF's `D` pin.
    pub net: String,
    pub from: CrossingSource,
    /// Domain index of the load FF.
    pub to: usize,
    /// Cross-domain driver cells (launch FFs / port names).
    pub driver_cells: Vec<String>,
    /// The load FF.
    pub load_ff: String,
}

/// One classified structural CDC violation row.
#[derive(Clone, Debug)]
pub struct CdcCrossing {
    pub net: String,
    /// Bus stem for per-bit records of a bus crossing (`cnt` for `cnt[0]`).
    pub bus: Option<String>,
    pub from_domain: String,
    pub to_domain: String,
    pub class: CdcClass,
    pub severity: CdcSeverity,
    pub check: String,
    pub driver_cells: Vec<String>,
    pub load_cells: Vec<String>,
    pub sync_depth: u8,
    pub detail: String,
}

impl CdcCrossing {
    pub fn text(&self) -> String {
        format!(
            "CROSSING NET={} FROM={} TO={} CLASS={} SEVERITY={} CHECK={} DRIVERS={} LOADS={} SYNC_DEPTH={} DETAIL=\"{}\"",
            self.net,
            self.from_domain,
            self.to_domain,
            self.class.as_str(),
            self.severity.as_str(),
            self.check,
            self.driver_cells.join(","),
            self.load_cells.join(","),
            self.sync_depth,
            self.detail,
        )
    }
}

/// Bus stem of a bit net name: `cnt[3]` -> `cnt`, `data_2` -> `data`.
/// Returns `None` when the name has no bit suffix.
pub fn bus_stem(net: &str) -> Option<String> {
    if let Some(open) = net.rfind('[') {
        if net.ends_with(']') && net[open + 1..net.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            return Some(net[..open].to_string());
        }
    }
    if let Some(us) = net.rfind('_') {
        let suffix = &net[us + 1..];
        if !suffix.is_empty()
            && suffix.chars().all(|c| c.is_ascii_digit())
            && us > 0
        {
            return Some(net[..us].to_string());
        }
    }
    None
}

/// One driver of a net: where it lives plus the driving cell/port names.
#[derive(Clone, Debug)]
pub struct DriverInfo {
    pub source: CrossingSource,
    pub cells: Vec<String>,
}

/// Transitively collect the drivers of `net`: backward through `Lut6`s
/// (cycle-safe), stopping at `Hff` `Q` boundaries. Nets with no `Q`/`O`
/// driver whose name matches a top-level input port are `PortInput` drivers.
pub fn driver_infos(design: &Design, domains: &DomainMap, net: &str) -> Vec<DriverInfo> {
    use std::collections::HashMap;
    let input_ports: HashSet<&str> = design
        .ports
        .iter()
        .filter(|p| p.dir == helion_ir::PortDir::In)
        .map(|p| resolve(p.name))
        .collect();
    let mut by_source: HashMap<CrossingSource, Vec<String>> = HashMap::new();
    let mut stack = vec![net.to_string()];
    let mut done: HashSet<String> = HashSet::new();
    while let Some(n) = stack.pop() {
        if !done.insert(n.clone()) {
            continue;
        }
        if done.len() > 4096 {
            break;
        }
        let Some(netobj) = design.net(&n) else {
            continue;
        };
        let mut saw_driver = false;
        for e in &netobj.endpoints {
            let Some(cell) = design.cell(resolve(e.cell)) else {
                continue;
            };
            match (&cell.kind, resolve(e.pin)) {
                (helion_ir::CellKind::Hff, "Q") => {
                    saw_driver = true;
                    if let Some(d) = domains.domain_of_ff(resolve(e.cell)) {
                        by_source
                            .entry(CrossingSource::Domain(d))
                            .or_default()
                            .push(resolve(e.cell).to_string());
                    }
                }
                (helion_ir::CellKind::Lut6 { .. }, "O") => {
                    saw_driver = true;
                    for p in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                        if let Some(pn) = design.net_on(e.cell, intern(p)) {
                            stack.push(resolve(pn).to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        if !saw_driver && input_ports.contains(n.as_str()) {
            by_source
                .entry(CrossingSource::PortInput)
                .or_default()
                .push(n.clone());
        }
    }
    let mut out: Vec<DriverInfo> = by_source
        .into_iter()
        .map(|(source, mut cells)| {
            cells.sort();
            cells.dedup();
            DriverInfo { source, cells }
        })
        .collect();
    out.sort_by(|a, b| {
        fn rank(s: &CrossingSource) -> u8 {
            match s {
                CrossingSource::Domain(_) => 0,
                CrossingSource::PortInput => 1,
            }
        }
        rank(&a.source).cmp(&rank(&b.source))
    });
    out
}

/// `true` when domains `a`/`b` form a generated-clock/master pair, i.e. the
/// pair-level `TimedGenerated` relation: synchronous by declaration, so no
/// crossing is reported. Tracing divider logic to *prove* synchronicity is
/// Phase-2 (SAT) work; only the declared pair is filtered.
fn synchronous_pair(domains: &DomainMap, a: usize, b: usize) -> bool {
    let (da, db) = (&domains.domains[a], &domains.domains[b]);
    (da.kind == DomainKind::Generated && da.master.as_deref() == Some(db.name.as_str()))
        || (db.kind == DomainKind::Generated && db.master.as_deref() == Some(da.name.as_str()))
}

/// Find candidate crossings: one [`RawCrossing`] per `(D net, load FF,
/// driver domain)` where the driver domain differs from the load FF's
/// domain. Same-domain drivers, generated-clock/master pairs, and
/// unconstrained (`Unknown`) domains are filtered here; the latter are
/// reported as `UNCONSTRAINED_CLOCK_NET` instead of guessed crossings.
pub fn find_crossings(design: &Design, domains: &DomainMap) -> Vec<RawCrossing> {
    let mut out = Vec::new();
    let mut info_cache: std::collections::HashMap<helion_ir::Spur, Vec<DriverInfo>> =
        std::collections::HashMap::new();
    let pin_d = intern("D");
    for cell in &design.cells {
        if !matches!(cell.kind, helion_ir::CellKind::Hff) {
            continue;
        }
        let cell_name = resolve(cell.name);
        let Some(to) = domains.domain_of_ff(cell_name) else {
            continue;
        };
        let Some(d_net) = design.net_on(cell.name, pin_d) else {
            continue;
        };
        let d_net_str = resolve(d_net);
        let infos = info_cache
            .entry(d_net)
            .or_insert_with(|| driver_infos(design, domains, d_net_str));
        for di in infos {
            match di.source {
                CrossingSource::PortInput => out.push(RawCrossing {
                    net: d_net_str.to_string(),
                    from: CrossingSource::PortInput,
                    to,
                    driver_cells: di.cells.clone(),
                    load_ff: cell_name.to_string(),
                }),
                CrossingSource::Domain(from) => {
                    if from == to || synchronous_pair(domains, from, to) {
                        continue;
                    }
                    if domains.domains[from].kind == DomainKind::Unknown
                        || domains.domains[to].kind == DomainKind::Unknown
                    {
                        continue;
                    }
                    out.push(RawCrossing {
                        net: d_net_str.to_string(),
                        from: CrossingSource::Domain(from),
                        to,
                        driver_cells: di.cells.clone(),
                        load_ff: cell_name.to_string(),
                    });
                }
            }
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::bus_stem;

    #[test]
    fn bus_stem_shapes() {
        assert_eq!(bus_stem("cnt[3]").as_deref(), Some("cnt"));
        assert_eq!(bus_stem("data_2").as_deref(), Some("data"));
        assert_eq!(bus_stem("a[10]").as_deref(), Some("a"));
        assert_eq!(bus_stem("clk").as_deref(), None);
        assert_eq!(bus_stem("ab12").as_deref(), None);
        assert_eq!(bus_stem("x_").as_deref(), None);
        assert_eq!(bus_stem("q[").as_deref(), None);
    }
}
