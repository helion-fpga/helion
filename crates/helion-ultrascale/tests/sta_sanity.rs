use helion_ultrascale::sta::{StaNetlist, TimingReport, sta_routed};
use helion_ultrascale::timing_db::{
    documented_xcku3p, preliminary_xcvu3p, SourcedDelay,
};
use std::collections::{HashMap, HashSet};

/// Synthetic 2-bit counter: ff0 -> lut -> ff1, ff1 -> lut -> ff0.
struct CounterNetlist {
    drivers: HashMap<String, (String, i32, i32)>,
    fanin: HashMap<String, Vec<String>>,
}

impl CounterNetlist {
    fn new() -> Self {
        let mut drivers = HashMap::new();
        let mut fanin = HashMap::new();
        drivers.insert("q0".to_string(), ("ff:ff0".to_string(), 0, 0));
        drivers.insert("q1".to_string(), ("ff:ff1".to_string(), 1, 0));
        drivers.insert("d1".to_string(), ("lut0".to_string(), 0, 1));
        drivers.insert("d0".to_string(), ("lut1".to_string(), 1, 1));
        fanin.insert("lut0".to_string(), vec!["q0".to_string()]);
        fanin.insert("lut1".to_string(), vec!["q1".to_string()]);
        Self { drivers, fanin }
    }
}

impl StaNetlist for CounterNetlist {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        vec![("ff0".to_string(), 0, 0), ("ff1".to_string(), 1, 0)]
    }
    fn luts(&self) -> Vec<(String, i32, i32)> {
        vec![("lut0".to_string(), 0, 1), ("lut1".to_string(), 1, 1)]
    }
    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        self.drivers.clone()
    }
    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        self.fanin.clone()
    }
    fn ff_q_net(&self, ff: &str) -> Option<String> {
        match ff {
            "ff0" => Some("q0".to_string()),
            "ff1" => Some("q1".to_string()),
            _ => None,
        }
    }
    fn ff_d_net(&self, ff: &str) -> Option<String> {
        match ff {
            "ff0" => Some("d0".to_string()),
            "ff1" => Some("d1".to_string()),
            _ => None,
        }
    }
}

/// STA on a trivial counter: finite, positive, plausible.
/// Model: lut=120ps, ckq=150ps, setup=80ps, hop=180ps.
/// Path: ckq(150) + wire 1 hop (180) + lut(120) + wire 2 hops (360) = 810ps.
#[test]
fn sta_counter_sane() {
    let nl = CounterNetlist::new();
    let db = preliminary_xcvu3p();
    let r: TimingReport = sta_routed(&nl, &db, 10_000).expect("sta");
    assert_eq!(r.endpoints, 2);
    assert_eq!(r.r2r_ps, 810);
    assert_eq!(r.worst_delay_ps, 810);
    assert_eq!(r.wns_ps, 10_000 - 80 - 810);
    assert_eq!(r.tns_ps, 0);
    // Preliminary DB is a ballpark estimate: the report says so.
    assert!(r.is_estimate, "preliminary STA must be labeled estimate");
    let line = r.hang_diag_line(5);
    assert!(line.contains("WNS_PS=9110"), "line: {line}");
    assert!(line.contains("endpoints=2"), "line: {line}");
    assert!(line.contains("estimate=1"), "line: {line}");
}

/// 4-LUT chain: critical path must name the right endpoints.
struct LongPathNetlist {
    drivers: HashMap<String, (String, i32, i32)>,
    fanin: HashMap<String, Vec<String>>,
}

impl LongPathNetlist {
    fn new() -> Self {
        let mut drivers = HashMap::new();
        let mut fanin = HashMap::new();
        drivers.insert("q0".to_string(), ("ff:src".to_string(), 0, 0));
        let nets = ["n_a", "n_b", "n_c", "n_d"];
        let luts = ["lut_a", "lut_b", "lut_c", "lut_d"];
        let mut prev = "q0";
        for (i, (net, lut)) in nets.iter().zip(luts.iter()).enumerate() {
            let x = (i + 1) as i32;
            drivers.insert(net.to_string(), (lut.to_string(), x, 0));
            fanin.insert(lut.to_string(), vec![prev.to_string()]);
            prev = net;
        }
        Self { drivers, fanin }
    }
}

impl StaNetlist for LongPathNetlist {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        vec![("src".to_string(), 0, 0), ("dst".to_string(), 5, 0)]
    }
    fn luts(&self) -> Vec<(String, i32, i32)> {
        vec![
            ("lut_a".to_string(), 1, 0),
            ("lut_b".to_string(), 2, 0),
            ("lut_c".to_string(), 3, 0),
            ("lut_d".to_string(), 4, 0),
        ]
    }
    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        self.drivers.clone()
    }
    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        self.fanin.clone()
    }
    fn ff_q_net(&self, ff: &str) -> Option<String> {
        if ff == "src" {
            Some("q0".to_string())
        } else {
            None
        }
    }
    fn ff_d_net(&self, ff: &str) -> Option<String> {
        if ff == "dst" {
            Some("n_d".to_string())
        } else {
            None
        }
    }
}

#[test]
fn sta_long_path_names_endpoints() {
    let nl = LongPathNetlist::new();
    let db = preliminary_xcvu3p();
    let r: TimingReport = sta_routed(&nl, &db, 10_000).expect("sta");
    assert_eq!(r.endpoints, 1);
    assert_eq!(r.worst_startpoint, "src");
    assert_eq!(r.worst_endpoint, "dst");
    // ckq(150) + 5 hops (900) + 4 luts (480) = 1530.
    assert_eq!(r.worst_delay_ps, 1530);
    assert_eq!(r.wns_ps, 10_000 - 80 - 1530);
}

/// UNKNOWN delays fail loud.
#[test]
fn sta_unknown_delay_fails_loud() {
    let nl = CounterNetlist::new();
    let mut db = preliminary_xcvu3p();
    db.cells.lut6.ps = None;
    let err = sta_routed(&nl, &db, 10_000).expect_err("must fail");
    assert!(err.contains("UNKNOWN"), "err: {err}");
}

/// The documented-only xcku3p DB cannot run STA: it has no LUT6 delay.
/// It fails loud on the first required field instead of producing a
/// number from a partially documented DB.
#[test]
fn sta_documented_db_fails_loud_on_lut6() {
    let nl = CounterNetlist::new();
    let db = documented_xcku3p();
    assert_eq!(db.bram.clk_to_do.ps, Some(1020));
    let err = sta_routed(&nl, &db, 10_000).expect_err("must fail");
    assert!(err.contains("LUT6 delay UNKNOWN"), "err: {err}");
}

/// A DB with only documented values behind the four required delays is
/// not an estimate: no estimate flag, no estimate token on the line.
#[test]
fn sta_fully_documented_db_is_not_an_estimate() {
    let nl = CounterNetlist::new();
    let mut db = documented_xcku3p();
    db.cells.lut6 = SourcedDelay::documented(120, "test fixture");
    db.cells.ff_ckq = SourcedDelay::documented(150, "test fixture");
    db.cells.ff_setup = SourcedDelay::documented(80, "test fixture");
    db.interconnect.single = SourcedDelay::documented(180, "test fixture");
    assert!(!db.is_estimate());
    let r: TimingReport = sta_routed(&nl, &db, 10_000).expect("sta");
    assert!(!r.is_estimate);
    assert_eq!(r.r2r_ps, 810);
    let line = r.hang_diag_line(5);
    assert!(!line.contains("estimate"), "line: {line}");
}

/// Carry-chain path (GAP-F5 V-2): ff0 -> propagate LUT -> CARRY8 -> ff1,
/// the shape of a counter incrementer. The carry cell is charged the
/// CARRY8 s_to_o delay (100 ps), not the LUT6 delay (120 ps).
/// Model: ckq=150, hop=180, lut=120, carry s_to_o=100, setup=80.
/// Path: ckq(150) + wire 1 hop (180) + lut(120) + wire 0 + carry(100)
///   + wire 0 = 550 ps.
struct CarryChainNetlist {
    drivers: HashMap<String, (String, i32, i32)>,
    fanin: HashMap<String, Vec<String>>,
}

impl CarryChainNetlist {
    fn new() -> Self {
        let mut drivers = HashMap::new();
        let mut fanin = HashMap::new();
        drivers.insert("q0".to_string(), ("ff:ff0".to_string(), 0, 0));
        drivers.insert("p".to_string(), ("lut_p".to_string(), 0, 1));
        drivers.insert("o".to_string(), ("c0".to_string(), 0, 1));
        fanin.insert("lut_p".to_string(), vec!["q0".to_string()]);
        fanin.insert("c0".to_string(), vec!["p".to_string()]);
        Self { drivers, fanin }
    }
}

impl StaNetlist for CarryChainNetlist {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        vec![("ff0".to_string(), 0, 0), ("ff1".to_string(), 0, 1)]
    }
    fn luts(&self) -> Vec<(String, i32, i32)> {
        vec![("lut_p".to_string(), 0, 1)]
    }
    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        self.drivers.clone()
    }
    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        self.fanin.clone()
    }
    fn ff_q_net(&self, ff: &str) -> Option<String> {
        if ff == "ff0" {
            Some("q0".to_string())
        } else {
            None
        }
    }
    fn ff_d_net(&self, ff: &str) -> Option<String> {
        if ff == "ff1" {
            Some("o".to_string())
        } else {
            None
        }
    }
    fn carry_cells(&self) -> HashSet<String> {
        HashSet::from(["c0".to_string()])
    }
}

#[test]
fn sta_carry_chain_uses_carry_delay_not_lut_delay() {
    let nl = CarryChainNetlist::new();
    let db = preliminary_xcvu3p();
    let r: TimingReport = sta_routed(&nl, &db, 10_000).expect("sta");
    assert_eq!(r.endpoints, 1);
    assert_eq!(r.r2r_ps, 550);
    assert_eq!(r.worst_delay_ps, 550);
    assert_eq!(r.worst_startpoint, "ff0");
    assert_eq!(r.worst_endpoint, "ff1");
    assert_eq!(r.wns_ps, 10_000 - 80 - 550);
    assert!(r.is_estimate, "preliminary STA must be labeled estimate");
}

/// A carry chain with UNKNOWN s_to_o fails loud instead of silently
/// charging the LUT delay or zero.
#[test]
fn sta_carry_unknown_s_to_o_fails_loud() {
    let nl = CarryChainNetlist::new();
    let mut db = preliminary_xcvu3p();
    db.carry8.s_to_o.ps = None;
    let err = sta_routed(&nl, &db, 10_000).expect_err("must fail");
    assert!(err.contains("CARRY8"), "err: {err}");
    assert!(err.contains("UNKNOWN"), "err: {err}");
}

/// A top-level input port feeding an FF D directly (the chain16 /
/// shift32 shape: `pipe[0] <= din`, `sr[0] <= din`). The port net
/// has no cell driver; it must be a timing startpoint, not a
/// missing-driver error.
struct DirectPortNetlist {
    declare_port: bool,
}

impl StaNetlist for DirectPortNetlist {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        vec![("ff0".to_string(), 0, 0)]
    }
    fn luts(&self) -> Vec<(String, i32, i32)> {
        vec![]
    }
    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        HashMap::new()
    }
    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        HashMap::new()
    }
    fn ff_q_net(&self, _ff: &str) -> Option<String> {
        None
    }
    fn ff_d_net(&self, ff: &str) -> Option<String> {
        if ff == "ff0" {
            Some("din".to_string())
        } else {
            None
        }
    }
    fn input_ports(&self) -> HashSet<String> {
        if self.declare_port {
            HashSet::from(["din".to_string()])
        } else {
            HashSet::new()
        }
    }
}

/// A declared input port feeding an FF D directly arrives at 0 and
/// names the port as the startpoint: worst_delay 0, WNS 10000-80.
#[test]
fn sta_input_port_feeding_ff_directly_is_startpoint() {
    let nl = DirectPortNetlist { declare_port: true };
    let db = preliminary_xcvu3p();
    let r: TimingReport = sta_routed(&nl, &db, 10_000).expect("sta");
    assert_eq!(r.endpoints, 1);
    assert_eq!(r.worst_delay_ps, 0);
    assert_eq!(r.worst_startpoint, "din");
    assert_eq!(r.worst_endpoint, "ff0");
    assert_eq!(r.wns_ps, 10_000 - 80);
    assert_eq!(r.tns_ps, 0);
}

/// The same net with no port declared still fails loud: the
/// startpoint model must not let an undriven internal net through.
#[test]
fn sta_undriven_internal_net_still_fails_loud() {
    let nl = DirectPortNetlist {
        declare_port: false,
    };
    let db = preliminary_xcvu3p();
    let err = sta_routed(&nl, &db, 10_000).expect_err("must fail");
    assert!(err.contains("timing: no driver for net din"), "err: {err}");
}

/// The chain16 head shape: ff0 D is `din` directly, ff1 D comes
/// through a LUT on ff0 Q and `din`. Model: ckq=150, hop=180,
/// lut=120, setup=80. Path to ff1: ckq(150) + wire 1 hop (180) +
/// lut(120) + wire 2 hops (360) = 810; ff0 arrives at 0 from the
/// port, so the worst path starts at ff0.
struct ChainHeadNetlist;

impl StaNetlist for ChainHeadNetlist {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        vec![("ff0".to_string(), 0, 0), ("ff1".to_string(), 1, 0)]
    }
    fn luts(&self) -> Vec<(String, i32, i32)> {
        vec![("lut0".to_string(), 0, 1)]
    }
    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        HashMap::from([
            ("q0".to_string(), ("ff:ff0".to_string(), 0, 0)),
            ("d1".to_string(), ("lut0".to_string(), 0, 1)),
        ])
    }
    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        HashMap::from([(
            "lut0".to_string(),
            vec!["q0".to_string(), "din".to_string()],
        )])
    }
    fn ff_q_net(&self, ff: &str) -> Option<String> {
        if ff == "ff0" {
            Some("q0".to_string())
        } else {
            None
        }
    }
    fn ff_d_net(&self, ff: &str) -> Option<String> {
        match ff {
            "ff0" => Some("din".to_string()),
            "ff1" => Some("d1".to_string()),
            _ => None,
        }
    }
    fn input_ports(&self) -> HashSet<String> {
        HashSet::from(["din".to_string()])
    }
}

#[test]
fn sta_chain_head_with_port_input() {
    let nl = ChainHeadNetlist;
    let db = preliminary_xcvu3p();
    let r: TimingReport = sta_routed(&nl, &db, 10_000).expect("sta");
    assert_eq!(r.endpoints, 2);
    assert_eq!(r.worst_delay_ps, 810);
    assert_eq!(r.worst_startpoint, "ff0");
    assert_eq!(r.worst_endpoint, "ff1");
    assert_eq!(r.wns_ps, 10_000 - 80 - 810);
    assert_eq!(r.tns_ps, 0);
}

/// A LUT fed only by an input port still names the port as the
/// startpoint: lut(120) + wire 2 hops (360) = 480, startpoint din.
struct PortOnlyLutNetlist;

impl StaNetlist for PortOnlyLutNetlist {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        vec![("ff0".to_string(), 1, 0)]
    }
    fn luts(&self) -> Vec<(String, i32, i32)> {
        vec![("lut0".to_string(), 0, 1)]
    }
    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        HashMap::from([("d0".to_string(), ("lut0".to_string(), 0, 1))])
    }
    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        HashMap::from([("lut0".to_string(), vec!["din".to_string()])])
    }
    fn ff_q_net(&self, _ff: &str) -> Option<String> {
        None
    }
    fn ff_d_net(&self, ff: &str) -> Option<String> {
        if ff == "ff0" {
            Some("d0".to_string())
        } else {
            None
        }
    }
    fn input_ports(&self) -> HashSet<String> {
        HashSet::from(["din".to_string()])
    }
}

#[test]
fn sta_lut_fed_only_by_port_names_port_startpoint() {
    let nl = PortOnlyLutNetlist;
    let db = preliminary_xcvu3p();
    let r: TimingReport = sta_routed(&nl, &db, 10_000).expect("sta");
    assert_eq!(r.endpoints, 1);
    assert_eq!(r.worst_delay_ps, 480);
    assert_eq!(r.worst_startpoint, "din");
    assert_eq!(r.worst_endpoint, "ff0");
    assert_eq!(r.wns_ps, 10_000 - 80 - 480);
}
