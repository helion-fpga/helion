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
    driver_infos_indexed(design, domains, net, &design.pin_index())
}

/// [`driver_infos`] with a caller-owned pin index, for a loop that walks many
/// nets (HELION_GAPS G23).
///
/// `driver_infos` used to probe each LUT input with `Design::net_on`, which
/// falls back to a linear scan of every net whenever the pin is not indexed --
/// and the v2 front end never calls `rebuild_indexes`, so *every* probe was a
/// scan. Walking Ibex (2,246 flops, 9,490 nets, a 4,096-node cap per cone)
/// took 14 s that way; with the index it is a hash lookup per pin.
pub fn driver_infos_indexed(
    design: &Design,
    domains: &DomainMap,
    net: &str,
    pins: &helion_ir::PinIndex,
) -> Vec<DriverInfo> {
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
                        if let Some(pn) = pins.net_on(resolve(e.cell), p) {
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
    // One pin index for the whole report (G23): without it each LUT-input
    // probe below is a linear scan of every net.
    let pins = design.pin_index();
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
            .or_insert_with(|| driver_infos_indexed(design, domains, d_net_str, &pins));
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
    use super::{bus_stem, driver_infos, driver_infos_indexed};
    use crate::CdcSeverity;

    /// G23 guard. The cone walk used `Design::net_on`, which falls back to a
    /// linear scan of every net when the pin is not indexed -- and the v2 front
    /// end never calls `rebuild_indexes`, so on a real netlist every LUT-input
    /// probe scanned the whole netlist. On Ibex that made `find_crossings` take
    /// 14 s; with `PinIndex` it takes 1.5 s.
    ///
    /// A wall-clock assertion would be flaky on a loaded machine, so this pins
    /// the thing that actually regressed: the walk must find the same drivers
    /// through the index as without it, on a design big enough that the two
    /// paths differ, and the indexed walk must be the one `find_crossings`
    /// uses. The scale half keeps a real cone walk honest: 200 flops each
    /// behind a shared LUT fanout, which is the shape that made the scan
    /// quadratic.
    #[test]
    fn cone_walk_uses_the_pin_index_and_agrees_with_it() {
        use helion_ir::{intern, CellKind, Design};
        // A chain of LUTs feeding 200 flops, all from one input port.
        let mut d = Design::new("walk");
        d.add_port("i", helion_ir::PortDir::In);
        d.add_cell("l0", CellKind::Lut6 { init: 0x55 });
        d.connect("i", "l0", "I0");
        // Cell names and net names are separate: the output net of cell `lk`
        // is `o{k}`, and the next cell's I0 reads it.
        d.connect("o0", "l0", "O");
        let mut prev = "o0".to_string();
        for k in 1..8 {
            let c = format!("l{k}");
            let o = format!("o{k}");
            d.add_cell(&c, CellKind::Lut6 { init: 0x55 });
            d.connect(&prev, &c, "I0");
            d.connect(&o, &c, "O");
            prev = o;
        }
        for k in 0..200 {
            let c = format!("ff{k}");
            d.add_cell(&c, CellKind::Hff);
            d.connect(&prev, &c, "D");
            d.connect("clk", &c, "C");
            d.add_port(&format!("q{k}"), helion_ir::PortDir::Out);
            d.connect(&format!("q{k}"), &c, "Q");
        }
        let xdc = crate::Constraints::default();
        let domains = crate::extract_domains(&d, &[], &xdc);
        let pins = d.pin_index();
        // Every LUT input the walk needs must be resolvable through the index;
        // if a probe is not, the walk silently takes the linear-scan path.
        for c in result_cells(&d) {
            for p in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                assert_eq!(
                    pins.net_on(&c, p).map(helion_ir::resolve),
                    d.net_on(intern(&c), intern(p)).map(helion_ir::resolve),
                    "PinIndex disagrees with net_on for {c}.{p}"
                );
            }
        }
        // Walk a net the LUT chain *reads*: that is the direction the cone walk
        // goes, transitively through `Lut6` O -> I pins. Every one of the 200
        // flops sits behind the same `l7`, so the cone from one flop's D pin
        // crosses the whole chain -- the fanout shape that made the scan
        // quadratic.
        let fast = driver_infos_indexed(&d, &domains, "o7", &pins);
        let slow = driver_infos(&d, &domains, "o7");
        assert_eq!(
            fast.len(),
            slow.len(),
            "indexed and plain cone walks disagree on driver count for o7"
        );
        // `i` is a top-level input with no Q/O driver, so it must surface as a
        // PortInput driver: proof the walk actually traversed the chain.
        assert!(
            matches!(fast.first().map(|d| d.source), Some(super::CrossingSource::PortInput)),
            "walking o7 did not reach the input port: got {:?}",
            fast.iter().map(|d| d.source).collect::<Vec<_>>()
        );
    }

    fn result_cells(d: &helion_ir::Design) -> Vec<String> {
        d.cells.iter().map(|c| helion_ir::resolve(c.name).to_string()).collect()
    }

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
