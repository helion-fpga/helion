//! Multi-seed timing-closure sweep for UltraScale+ (xcku3p control part).
//!
//! Port of the multiseed proof on fm/ultra-multiseed (e1c2e47, master
//! based), which lives in `helion-cli/src/main.rs` there: synth once,
//! place/route/STA per seed on a thread pool, append per-seed
//! `{seed, wns_ps, tns_ps, runtime_s}` JSONL records under a file mutex
//! for crash-safe checkpointing, skip completed seeds on resume, and pick
//! the best seed deterministically (highest WNS at or above the
//! threshold, tie-break lowest TNS then lowest seed).
//!
//! What changes on this base, and what does not:
//! - Seed determinism comes from the same SplitMix64 seeded Fisher-Yates
//!   in `place::seeded_order`, threaded through `UltraPlaceOpts.seed`.
//!   Same seed gives the same placement, hence the same STA result.
//! - Per-seed work here is pack-once, then place(seed) plus legality
//!   plus preliminary STA on the placed netlist. Pack is seed
//!   independent, so it runs once and is shared across seeds.
//! - ROUTE is blocked for every seed: the fabric WireDb is empty (no
//!   routing graph yet; routing-graph worker owns it), so `route_ultra`
//!   is never reached. Each record carries `route_status: "blocked"` and
//!   the summary report says so. A seed with place plus STA green is
//!   `status: "ok"` for place-plus-STA only, never for the full flow.
//! - BITGEN is not implemented on this base. No bitstream is produced
//!   by any seed.
//! - Timing numbers are ESTIMATES until the xcku3p delay DB exists
//!   (GAP-F5). STA uses `timing_db::preliminary_xcvu3p` (ballpark
//!   delays: LUT 120ps, FF ckq 150ps, setup 80ps, one INT hop 180ps).
//!   Every report that shows WNS must say so.
//! - The proof's 10-seed WNS range 8460 to 8820 is recorded in
//!   US-09-end-to-end-flow.md only; no run artifact for it exists in
//!   either repo, so it is not reproduced here and no number from it
//!   is reused. The runs in this crate produce their own numbers.
//!
//! Throughput matters more than per-run latency (Saksham's
//! requirement: 100-seed overnight runs), so the CLI runs seeds on a
//! rayon pool sized by `--jobs` and checkpoints every finished seed
//! to the JSONL file as soon as it lands.

use std::collections::HashSet;

/// One seed's result from the multiseed sweep.
///
/// JSONL shape (one line per seed, keys in this order):
/// `{"seed": N, "wns_ps": N, "tns_ps": N, "runtime_s": N.NNN,
/// "status": "ok"|"fail", "reason": "...", "route_status": "..."}`
/// `route_status` is `"blocked"` on this base (WireDb empty),
/// `"ok"` once routing lands, `"fail"` when routing itself errors.
/// Lines written by the master-based proof have no `route_status`
/// key; parsing one defaults it to `"unknown"`.
#[derive(Clone, Debug, PartialEq)]
pub struct SeedResult {
    pub seed: u64,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub runtime_s: f64,
    pub status: String,
    pub reason: String,
    pub route_status: String,
}

impl SeedResult {
    /// A place-plus-STA success for one seed. Route stays blocked.
    pub fn ok(seed: u64, wns_ps: i64, tns_ps: i64, runtime_s: f64) -> Self {
        Self {
            seed,
            wns_ps,
            tns_ps,
            runtime_s,
            status: "ok".to_string(),
            reason: String::new(),
            route_status: "blocked".to_string(),
        }
    }

    /// A failed seed (place, legality, or STA errored). WNS/TNS are 0.
    pub fn fail(seed: u64, runtime_s: f64, reason: String) -> Self {
        Self {
            seed,
            wns_ps: 0,
            tns_ps: 0,
            runtime_s,
            status: "fail".to_string(),
            reason,
            route_status: "blocked".to_string(),
        }
    }

    /// Encode one JSONL line. Manual JSON: no serde_json dependency
    /// is assumed here; the reason is sanitized to one line with
    /// quotes and backslashes escaped.
    pub fn to_jsonl(&self) -> String {
        let reason = self
            .reason
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ");
        format!(
            "{{\"seed\": {}, \"wns_ps\": {}, \"tns_ps\": {}, \"runtime_s\": {:.3}, \"status\": \"{}\", \"reason\": \"{}\", \"route_status\": \"{}\"}}",
            self.seed,
            self.wns_ps,
            self.tns_ps,
            self.runtime_s,
            self.status,
            reason,
            self.route_status
        )
    }

    /// Parse the seed field back from a JSONL line we wrote. Returns
    /// None if the line is not one of ours.
    pub fn parse_seed(line: &str) -> Option<u64> {
        let key = "\"seed\":";
        let i = line.find(key)?;
        let rest = line[i + key.len()..].trim_start();
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse().ok()
    }

    fn parse_i64_field(line: &str, key: &str) -> i64 {
        let k = format!("\"{key}\":");
        line.find(&k)
            .and_then(|i| {
                let rest = line[i + k.len()..].trim_start();
                let end = rest
                    .find(|c: char| !c.is_ascii_digit() && c != '-')
                    .unwrap_or(rest.len());
                rest[..end].parse().ok()
            })
            .unwrap_or(0)
    }

    fn parse_str_field(line: &str, key: &str) -> String {
        let k = format!("\"{key}\": \"");
        let Some(i) = line.find(&k) else {
            return String::new();
        };
        let rest = &line[i + k.len()..];
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                }
                '"' => break,
                _ => out.push(c),
            }
        }
        out
    }

    /// Parse one of our JSONL lines back into a record. Returns None
    /// if the line carries no seed field. A missing `route_status`
    /// key (proof-era lines) parses as `"unknown"`. `runtime_s` is
    /// not recovered: the report re-read needs seed, status, and
    /// the slack numbers only.
    pub fn from_jsonl(line: &str) -> Option<Self> {
        let seed = Self::parse_seed(line)?;
        let status = Self::parse_str_field(line, "status");
        let route_status = {
            let k = "\"route_status\":";
            if line.contains(k) {
                Self::parse_str_field(line, "route_status")
            } else {
                "unknown".to_string()
            }
        };
        Some(Self {
            seed,
            wns_ps: Self::parse_i64_field(line, "wns_ps"),
            tns_ps: Self::parse_i64_field(line, "tns_ps"),
            runtime_s: 0.0,
            status,
            reason: Self::parse_str_field(line, "reason"),
            route_status,
        })
    }
}

/// Read an existing JSONL results file and return the set of seeds
/// already recorded (ok or failed). Missing file = empty set.
pub fn read_completed_seeds(path: &str) -> HashSet<u64> {
    let mut set = HashSet::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return set;
    };
    for line in text.lines() {
        if let Some(seed) = SeedResult::parse_seed(line) {
            set.insert(seed);
        }
    }
    set
}

/// Deterministic best-seed selection. Among seeds with status "ok"
/// and wns_ps >= threshold, pick the highest wns_ps (most timing
/// margin); tie-break on lowest tns_ps, then lowest seed. Returns
/// None when no seed passes the threshold. Same rule as the proof.
pub fn select_best_seed<'a>(results: &'a [SeedResult], threshold: i64) -> Option<&'a SeedResult> {
    results
        .iter()
        .filter(|r| r.status == "ok" && r.wns_ps >= threshold)
        .max_by(|a, b| {
            a.wns_ps
                .cmp(&b.wns_ps)
                .then_with(|| b.tns_ps.cmp(&a.tns_ps))
                .then_with(|| b.seed.cmp(&a.seed))
        })
}

/// Summary of a sweep over the records on disk (checkpointed plus
/// this run). WNS numbers are preliminary estimates; see module docs.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiseedSummary {
    pub total: usize,
    pub n_ok: usize,
    pub n_fail: usize,
    pub n_route_blocked: usize,
    pub best_seed: Option<u64>,
    pub best_wns_ps: Option<i64>,
    pub best_tns_ps: Option<i64>,
    pub min_wns_ps: Option<i64>,
    pub max_wns_ps: Option<i64>,
}

/// Summarize `results` against `threshold`. WNS range covers ok
/// seeds only; failed seeds carry wns 0 and must not drag the range.
pub fn summarize_multiseed(results: &[SeedResult], threshold: i64) -> MultiseedSummary {
    let n_ok = results.iter().filter(|r| r.status == "ok").count();
    let n_fail = results.len() - n_ok;
    let n_route_blocked = results
        .iter()
        .filter(|r| r.route_status == "blocked")
        .count();
    let best = select_best_seed(results, threshold);
    let (min_wns_ps, max_wns_ps) = {
        let mut vals: Vec<i64> = results
            .iter()
            .filter(|r| r.status == "ok")
            .map(|r| r.wns_ps)
            .collect();
        if vals.is_empty() {
            (None, None)
        } else {
            vals.sort_unstable();
            (Some(vals[0]), Some(vals[vals.len() - 1]))
        }
    };
    MultiseedSummary {
        total: results.len(),
        n_ok,
        n_fail,
        n_route_blocked,
        best_seed: best.map(|r| r.seed),
        best_wns_ps: best.map(|r| r.wns_ps),
        best_tns_ps: best.map(|r| r.tns_ps),
        min_wns_ps,
        max_wns_ps,
    }
}

/// One-line best-seed verdict, same wording as the proof, plus the
/// route-blocked count so the verdict never reads as a full flow.
/// WNS here is a preliminary estimate (see module docs).
pub fn best_seed_line(summary: &MultiseedSummary, threshold: i64) -> String {
    match (summary.best_seed, summary.best_wns_ps, summary.best_tns_ps) {
        (Some(seed), Some(wns), Some(tns)) => format!(
            "multiseed: BEST seed={} wns_ps={} tns_ps={} ({}/{} ok, {}/{} fail, route_blocked={}/{}, threshold_wns_ps={}) [WNS is a preliminary estimate; route blocked for all seeds]",
            seed,
            wns,
            tns,
            summary.n_ok,
            summary.total,
            summary.n_fail,
            summary.total,
            summary.n_route_blocked,
            summary.total,
            threshold
        ),
        _ => format!(
            "multiseed: NO seed met threshold_wns_ps={} ({}/{} ok, {}/{} fail, route_blocked={}/{}) [WNS is a preliminary estimate; route blocked for all seeds]",
            threshold,
            summary.n_ok,
            summary.total,
            summary.n_fail,
            summary.total,
            summary.n_route_blocked,
            summary.total
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(seed: u64, wns: i64, tns: i64) -> SeedResult {
        SeedResult::ok(seed, wns, tns, 0.5)
    }

    #[test]
    fn best_seed_picks_highest_wns_above_threshold() {
        let results = vec![ok(1, 8000, 0), ok(2, 8800, 0), ok(3, 8500, 0)];
        let best = select_best_seed(&results, 0).unwrap();
        assert_eq!(best.seed, 2);
        assert_eq!(best.wns_ps, 8800);
    }

    #[test]
    fn best_seed_threshold_filters() {
        let results = vec![ok(1, 8000, 0), ok(2, 8800, 0)];
        assert!(select_best_seed(&results, 9000).is_none());
        assert_eq!(select_best_seed(&results, 8500).unwrap().seed, 2);
    }

    #[test]
    fn best_seed_tie_breaks_lowest_tns_then_lowest_seed() {
        let results = vec![ok(5, 8800, 100), ok(3, 8800, 0), ok(7, 8800, 0)];
        let best = select_best_seed(&results, 0).unwrap();
        assert_eq!(best.seed, 3, "lowest TNS wins, then lowest seed");
        let results = vec![ok(9, 8800, 50), ok(4, 8800, 50)];
        assert_eq!(select_best_seed(&results, 0).unwrap().seed, 4);
    }

    #[test]
    fn best_seed_ignores_failed_seeds() {
        let mut results = vec![ok(1, 8000, 0)];
        results.push(SeedResult::fail(2, 0.1, "ULTRA PLACE: boom".to_string()));
        assert_eq!(select_best_seed(&results, 0).unwrap().seed, 1);
    }

    #[test]
    fn jsonl_roundtrip_with_escaping() {
        let r = SeedResult {
            seed: 7,
            wns_ps: -120,
            tns_ps: 300,
            runtime_s: 1.234,
            status: "fail".to_string(),
            reason: "bad \"quoted\" \\ path\nsecond line".to_string(),
            route_status: "blocked".to_string(),
        };
        let line = r.to_jsonl();
        assert!(line.contains("\\\"quoted\\\""), "line: {line}");
        assert!(!line.contains('\n'), "line: {line}");
        let back = SeedResult::from_jsonl(&line).unwrap();
        assert_eq!(back.seed, 7);
        assert_eq!(back.wns_ps, -120);
        assert_eq!(back.tns_ps, 300);
        assert_eq!(back.status, "fail");
        assert_eq!(back.route_status, "blocked");
        assert!(
            back.reason.contains("\"quoted\""),
            "reason: {}",
            back.reason
        );
    }

    #[test]
    fn jsonl_parse_proof_line_without_route_status() {
        let line = "{\"seed\": 3, \"wns_ps\": 8460, \"tns_ps\": 0, \"runtime_s\": 1.000, \"status\": \"ok\", \"reason\": \"\"}";
        let back = SeedResult::from_jsonl(line).unwrap();
        assert_eq!(back.seed, 3);
        assert_eq!(back.wns_ps, 8460);
        assert_eq!(back.route_status, "unknown");
        assert_eq!(SeedResult::parse_seed("not a record"), None);
        assert_eq!(SeedResult::from_jsonl("not a record"), None);
    }

    #[test]
    fn checkpoint_read_skips_missing_file_and_counts_done() {
        assert!(read_completed_seeds("/tmp/helion-multiseed-no-such-file.jsonl").is_empty());
        let path = "/tmp/helion-multiseed-checkpoint-test.jsonl";
        std::fs::write(
            path,
            format!(
                "{}\n{}\nnot a record\n",
                ok(1, 8000, 0).to_jsonl(),
                SeedResult::fail(2, 0.1, "x".to_string()).to_jsonl()
            ),
        )
        .unwrap();
        let done = read_completed_seeds(path);
        assert_eq!(done.len(), 2);
        assert!(done.contains(&1) && done.contains(&2));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn summary_counts_and_wns_range_exclude_failures() {
        let mut results = vec![ok(1, 8000, 0), ok(2, 8800, 10), ok(3, 8500, 0)];
        results.push(SeedResult::fail(4, 0.1, "x".to_string()));
        let s = summarize_multiseed(&results, 0);
        assert_eq!(s.total, 4);
        assert_eq!(s.n_ok, 3);
        assert_eq!(s.n_fail, 1);
        assert_eq!(s.n_route_blocked, 4);
        assert_eq!(s.best_seed, Some(2));
        assert_eq!(s.min_wns_ps, Some(8000));
        assert_eq!(s.max_wns_ps, Some(8800));
        let line = best_seed_line(&s, 0);
        assert!(line.contains("BEST seed=2"), "line: {line}");
        assert!(line.contains("preliminary estimate"), "line: {line}");
    }

    #[test]
    fn summary_empty_and_no_seed_meets_threshold() {
        let s = summarize_multiseed(&[], 0);
        assert_eq!(s.total, 0);
        assert_eq!(s.min_wns_ps, None);
        let line = best_seed_line(&s, 0);
        assert!(line.contains("NO seed met"), "line: {line}");
        let results = vec![ok(1, 100, 0)];
        let s = summarize_multiseed(&results, 5000);
        assert_eq!(s.best_seed, None);
        assert_eq!(s.min_wns_ps, Some(100));
        assert_eq!(s.max_wns_ps, Some(100));
    }
}
