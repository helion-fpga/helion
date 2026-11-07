use helion_ultrascale::sta::{StaNetlist, TimingReport, sta_routed};
use helion_ultrascale::timing_db::preliminary_xcvu3p;
use std::collections::HashMap;

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
    let line = r.hang_diag_line(5);
    assert!(line.contains("WNS_PS=9110"), "line: {line}");
    assert!(line.contains("endpoints=2"), "line: {line}");
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
