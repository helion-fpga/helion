//! Routed-design INT PIP verification against the full segbits DB.
//!
//! This test loads the real prjuray-db `zynqusp/segbits_int.db` (3778
//! lines, CC0 1.0, commit affbc5e) and checks the INT PIP layer end to end:
//! a net is routed across one INT tile using only real DB edges, the PIPs
//! are emitted, and every emitted bit is verified by an INDEPENDENT lookup
//! (a second parser written in this file, not the `FileSegbitsDb` loader),
//! never by re-reading the emission's own output.
//!
//! The DB is located, in order: the `HELION_SEGBITS_INT_DB` env var, the
//! reference clone at `~/workspace/prjuray-db-ref/zynqusp/segbits_int.db`,
//! or the vendored copy at `crates/helion-ultrascale/data/segbits_int.db`
//! (segbits-vendor's deliverable). If none exists the test fails loud and
//! names every path it searched: a missing ground-truth file is a blocker,
//! not a skip.
//!
//! The routed design is "blinky": a net whose driver and sink are BEL pins
//! mapped to INT wires, routed across one INT tile (tile `INT_X12Y45`, the
//! tile used in the day-1 report examples). This is a single-INT-tile route
//! segment, not a full placed-and-routed blinky: placement and the
//! production router are other workers' scope, and the day-1 router
//! scaffold's mock WireDb cannot name wires, so it cannot name PIPs. The
//! graph here is built from the real DB edges, which is the same input the
//! production router produces once the WireDb naming seam exists.

use helion_ultrascale::{
    FileSegbitsDb, IntPip, SegbitsDb, emit_pips, resolve_pips, verify_zero_bits,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

/// The INT tile used for the routed test (day-1 report tile).
const TILE: &str = "INT_X12Y45";
/// Driver wire of the blinky net inside the tile.
const DRIVER: &str = "BOUNCE_E_0_FT1";
/// Sink wire: the net leaves the tile north on a long wire.
const SINK: &str = "NN12_END7";

/// Shortest path from DRIVER to SINK in the DB graph, verified independently
/// with a Python BFS over the same file (file-order adjacency, visited on
/// enqueue). The test asserts the Rust BFS reproduces it exactly.
const EXPECTED_PATH: &[&str] = &[
    "BOUNCE_E_0_FT1",
    "INODE_E_1_FT1",
    "BYPASS_E4",
    "INT_NODE_IMUX_5_INT_OUT1",
    "BYPASS_E8",
    "INT_NODE_IMUX_17_INT_OUT1",
    "BYPASS_E14",
    "INT_NODE_IMUX_27_INT_OUT1",
    "INT_NODE_GLOBAL_9_INT_OUT1",
    "INT_INT_SDQ_75_INT_OUT0",
    "INT_NODE_SDQ_37_INT_OUT1",
    "NN12_END7",
];

fn locate_int_db() -> PathBuf {
    let mut searched: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("HELION_SEGBITS_INT_DB") {
        searched.push(format!("HELION_SEGBITS_INT_DB={}", p));
        let path = PathBuf::from(&p);
        if path.exists() {
            return path;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join("workspace/prjuray-db-ref/zynqusp/segbits_int.db");
        searched.push(p.display().to_string());
        if p.exists() {
            return p;
        }
    }
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/segbits_int.db");
    searched.push(p.display().to_string());
    if p.exists() {
        return p;
    }
    panic!(
        "segbits_int.db not found; searched:\n  {}\n\
         Clone prjuray-db (CC0) or set HELION_SEGBITS_INT_DB.",
        searched.join("\n  ")
    );
}

/// Independent parse result: feature -> (set bits, zero bits), each sorted.
type ParsedSegbits = HashMap<String, (Vec<(u32, u16)>, Vec<(u32, u16)>)>;

/// Independent segbits parser, written for this test only. Maps a feature
/// string to (set bits, zero bits), each a sorted Vec of (frame, bit).
/// Deliberately shares no code with `FileSegbitsDb`.
fn independent_parse(raw: &str) -> ParsedSegbits {
    let mut map: ParsedSegbits = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut toks = line.split_whitespace();
        let head = toks.next().unwrap();
        let feature = head.strip_prefix("INT.").unwrap().to_string();
        let mut set = Vec::new();
        let mut zero = Vec::new();
        for tok in toks {
            let (neg, body) = match tok.strip_prefix('!') {
                Some(b) => (true, b),
                None => (false, tok),
            };
            let (f, b) = body.split_once('_').unwrap();
            let pair = (f.parse::<u32>().unwrap(), b.parse::<u16>().unwrap());
            if neg {
                zero.push(pair);
            } else {
                set.push(pair);
            }
        }
        set.sort();
        zero.sort();
        map.insert(feature, (set, zero));
    }
    map
}

/// Directed graph of INT wires from the DB: src wire -> dst wires, in file
/// order. Returns (adjacency, edge features).
fn build_graph(parsed: &ParsedSegbits, raw: &str) -> HashMap<String, Vec<String>> {
    let _ = parsed;
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let head = line.split_whitespace().next().unwrap();
        let rest = head.strip_prefix("INT.PIP.").unwrap();
        let (src, dst) = rest.rsplit_once('.').unwrap();
        adj.entry(src.to_string())
            .or_default()
            .push(dst.to_string());
    }
    adj
}

/// BFS shortest path (file-order adjacency, visited on enqueue).
fn bfs_path(adj: &HashMap<String, Vec<String>>, start: &str, goal: &str) -> Option<Vec<String>> {
    let mut prev: HashMap<String, String> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut q: VecDeque<String> = VecDeque::new();
    seen.insert(start.to_string());
    q.push_back(start.to_string());
    while let Some(n) = q.pop_front() {
        if n == goal {
            let mut path = vec![goal.to_string()];
            let mut cur = goal;
            while cur != start {
                let p = prev.get(cur).unwrap();
                path.push(p.clone());
                cur = p;
            }
            path.reverse();
            return Some(path);
        }
        if let Some(nexts) = adj.get(&n) {
            for m in nexts {
                if seen.insert(m.clone()) {
                    prev.insert(m.clone(), n.clone());
                    q.push_back(m.clone());
                }
            }
        }
    }
    None
}

fn sorted_bits(bits: &[helion_ultrascale::BitRef]) -> Vec<(u32, u16)> {
    let mut v: Vec<(u32, u16)> = bits.iter().map(|b| (b.frame, b.bit)).collect();
    v.sort();
    v
}

#[test]
fn blinky_int_tile_route_matches_db_bit_exact() {
    let db_path = locate_int_db();
    let raw = std::fs::read_to_string(&db_path).unwrap();
    let db = FileSegbitsDb::from_file(&db_path).unwrap();

    // Ground truth shape: 3778 INT PIP entries, no other tile type.
    assert_eq!(
        db.feature_count(),
        3778,
        "segbits_int.db must have 3778 lines"
    );
    assert_eq!(db.tile_types(), &["INT".to_string()]);

    // Route the blinky net across the tile using only real DB edges.
    let parsed = independent_parse(&raw);
    assert_eq!(parsed.len(), 3778);
    let adj = build_graph(&parsed, &raw);
    let path = bfs_path(&adj, DRIVER, SINK).expect("blinky route must exist in the DB graph");
    let expected: Vec<String> = EXPECTED_PATH.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        path, expected,
        "BFS route must match the independently verified path"
    );

    // Emit PIPs for the route.
    let pips: Vec<IntPip> = path
        .windows(2)
        .map(|w| IntPip::new(TILE, &w[0], &w[1]))
        .collect();
    assert_eq!(pips.len(), 11, "12-wire path has 11 PIP hops");
    let res = resolve_pips("blinky", &pips, &db);
    assert!(
        res.missing.is_empty(),
        "routed blinky PIPs must all resolve; missing: {:?}",
        res.missing
    );
    let emitted = emit_pips("blinky", &pips, &db).unwrap();
    assert_eq!(emitted.len(), 11);

    // Verify every emitted bit by INDEPENDENT lookup from the raw file text.
    let mut total_set = 0usize;
    let mut total_zero = 0usize;
    for e in &emitted {
        let (want_set, want_zero) = parsed
            .get(&e.pip.feature())
            .unwrap_or_else(|| panic!("independent lookup missed {}", e.pip.feature()));
        assert_eq!(
            sorted_bits(&e.set_bits),
            *want_set,
            "set bits differ for {}",
            e.pip.feature()
        );
        assert_eq!(
            sorted_bits(&e.zero_bits),
            *want_zero,
            "zero bits differ for {}",
            e.pip.feature()
        );
        total_set += e.set_bits.len();
        total_zero += e.zero_bits.len();
    }
    // Each of the 11 mux selections sets exactly one bit.
    assert_eq!(total_set, 11, "each hop sets exactly one bit");
    assert_eq!(total_zero, 48, "11 hops carry 48 zero bits total");

    // Assert every `!` bit reads 0 in the all-zero base frameset.
    verify_zero_bits(&emitted, &|_, _| false).unwrap();

    // FASM spot checks: first and last lines.
    assert_eq!(
        emitted[0].fasm_line,
        "INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"
    );
    assert_eq!(
        emitted[10].fasm_line,
        "INT_X12Y45.PIP.INT_NODE_SDQ_37_INT_OUT1.NN12_END7"
    );
}

#[test]
fn full_db_every_line_parses_and_resolves() {
    // Every one of the 3778 DB lines resolves through the loader, and the
    // loader's bits agree with the independent parse on every line.
    let db_path = locate_int_db();
    let raw = std::fs::read_to_string(&db_path).unwrap();
    let db = FileSegbitsDb::from_file(&db_path).unwrap();
    let parsed = independent_parse(&raw);
    let mut checked = 0usize;
    for (feature, (want_set, want_zero)) in &parsed {
        let bits = db.lookup("INT", feature).unwrap_or_else(|| {
            panic!("loader missed feature {}", feature);
        });
        let mut got_set: Vec<(u32, u16)> = bits
            .iter()
            .filter(|b| !b.must_be_zero)
            .map(|b| (b.frame, b.bit))
            .collect();
        got_set.sort();
        let mut got_zero: Vec<(u32, u16)> = bits
            .iter()
            .filter(|b| b.must_be_zero)
            .map(|b| (b.frame, b.bit))
            .collect();
        got_zero.sort();
        assert_eq!(got_set, *want_set, "set bits differ for {}", feature);
        assert_eq!(got_zero, *want_zero, "zero bits differ for {}", feature);
        checked += 1;
    }
    assert_eq!(checked, 3778);
}
