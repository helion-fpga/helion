//! Structural clock-domain crossing (CDC) analysis.
//!
//! Net-level violation detection layered on top of the pair-level
//! `report_cdc`: clock-domain extraction, crossing detection, synchronizer
//! recognition, and classification into the 11 `CdcClass` violation classes.
//! See `DESIGN.md` Part C in `~/workspace/helion-verify-plan`.

pub mod classify;
pub mod crossings;
pub mod domains;
pub mod synchronizers;
pub mod ug906_mtbf;

pub use classify::classify_crossings;
pub use crossings::{find_crossings, CdcClass, CdcCrossing, CrossingSource, RawCrossing};
pub use domains::{extract_domains, ClockDomain, DomainKind, DomainMap};
pub use synchronizers::{is_mux_select, sync_chain, SyncChain};
pub use ug906_mtbf::{
    classify_ug906_clock_pair, compute_cdc_mtbf_years, report_cdc_mtbf, CdcMtbfReport,
    CdcSyncMtbfEntry, ClockInteractionEntry, ClockInteractionMatrix, TechCorner,
    Ug906ClockPairClass, SECONDS_PER_YEAR,
};
// Legacy pair-level helpers, moved into `cdc::` (still crate-internal).
pub(crate) use domains::{clock_ff_loads, clock_net_name};
pub(crate) use synchronizers::cdc_synchronizer;

#[cfg(test)]
mod integration_tests {
    use super::crossings::CdcClass;
    use crate::{create_clock, report_cdc, CdcReport, CdcSeverity, Constraints};

    fn report_two_clocks(src: &str) -> CdcReport {
        let d = helion_sv::synth_sv(src, "cdc_test.sv").expect("synth failed");
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk_a", 10_000, "clk_a");
        create_clock(&mut clks, "clk_b", 8_000, "clk_b");
        report_cdc(&clks, &Constraints::default(), None, Some(&d))
    }

    fn of_class(r: &CdcReport, class: CdcClass) -> Vec<&super::crossings::CdcCrossing> {
        r.crossings.iter().filter(|c| c.class == class).collect()
    }

    #[test]
    fn unsync_crossing_single_bit() {
        // Note: tech-map inserts a buffer LUT on every D, so a *direct*
        // sample is depth 1 (SINGLE_FF_SYNC). A genuinely unsynchronized
        // single bit has combinational logic in its D-cone (depth 0).
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, output logic q);
  logic [3:0] cnt;
  always @(posedge clk_a) cnt <= cnt + 1'b1;
  always @(posedge clk_b) q <= cnt[0] ^ cnt[1];
endmodule"#,
        );
        let unsync = of_class(&r, CdcClass::UnsyncCrossing);
        assert_eq!(unsync.len(), 1, "{}", r.text());
        assert_eq!(unsync[0].severity, CdcSeverity::Critical);
        assert_eq!(unsync[0].check, "CDC-10");
        assert_eq!(unsync[0].sync_depth, 0);
        assert_eq!(unsync[0].from_domain, "clk_a");
        assert_eq!(unsync[0].to_domain, "clk_b");
        assert_eq!(r.structural_critical_count(), 1, "{}", r.text());
    }

    #[test]
    fn single_ff_sync_flagged() {
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, input logic d, output logic q);
  logic launch;
  always @(posedge clk_a) launch <= d;
  always @(posedge clk_b) q <= launch;
endmodule"#,
        );
        let single = of_class(&r, CdcClass::SingleFfSync);
        assert_eq!(single.len(), 1, "{}", r.text());
        assert_eq!(single[0].severity, CdcSeverity::Critical);
        assert_eq!(single[0].check, "CDC-10");
        assert_eq!(single[0].sync_depth, 1);
    }

    #[test]
    fn muxed_crossing_flagged() {
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, input logic sel_in, d0_in, d1_in, output logic q);
  logic sel_a, d0_b, d1_b, mux_q;
  always @(posedge clk_a) sel_a <= sel_in;
  always @(posedge clk_b) begin d0_b <= d0_in; d1_b <= d1_in; end
  always @(posedge clk_b) mux_q <= sel_a ? d0_b : d1_b;
  assign q = mux_q;
endmodule"#,
        );
        let muxed = of_class(&r, CdcClass::MuxedCrossing);
        assert!(!muxed.is_empty(), "{}", r.text());
        assert_eq!(muxed[0].severity, CdcSeverity::Warning);
        assert_eq!(muxed[0].check, "HELION-CDC-2");
    }

    #[test]
    fn multibit_bus_single_ff() {
        // Note: tech-map inserts a buffer LUT on every D, so each synth'd
        // bit is depth 1 (SINGLE_FF_SYNC). The depth-0 UNSYNC + aggregate
        // path is covered by the hand-built test in classify.rs; the synth
        // D-net names (d4..d7) carry no bus stem to aggregate on.
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, output logic [3:0] q);
  logic [3:0] cnt;
  always @(posedge clk_a) cnt <= cnt + 1'b1;
  always @(posedge clk_b) q <= cnt;
endmodule"#,
        );
        let single = of_class(&r, CdcClass::SingleFfSync);
        assert_eq!(single.len(), 4, "{}", r.text());
        for s in &single {
            assert_eq!(s.severity, CdcSeverity::Critical);
            assert_eq!(s.check, "CDC-10");
            assert_eq!(s.sync_depth, 1);
            assert_eq!(s.from_domain, "clk_a");
            assert_eq!(s.to_domain, "clk_b");
        }
        assert_eq!(r.structural_critical_count(), 4, "{}", r.text());
    }

    #[test]
    fn two_ff_sync_clean() {
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, input logic d, output logic q);
  logic launch, s0, s1;
  always @(posedge clk_a) launch <= d;
  always @(posedge clk_b) begin s0 <= launch; s1 <= s0; end
  assign q = s1;
endmodule"#,
        );
        assert_eq!(r.structural_critical_count(), 0, "{}", r.text());
        assert_eq!(r.structural_warning_count(), 0, "{}", r.text());
    }

    #[test]
    fn three_ff_sync_clean() {
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, input logic d, output logic q);
  logic launch, s0, s1, s2;
  always @(posedge clk_a) launch <= d;
  always @(posedge clk_b) begin s0 <= launch; s1 <= s0; s2 <= s1; end
  assign q = s2;
endmodule"#,
        );
        assert_eq!(r.structural_critical_count(), 0, "{}", r.text());
        assert_eq!(r.structural_warning_count(), 0, "{}", r.text());
        // Only the benign input-port Info may remain.
        assert!(
            r.crossings
                .iter()
                .all(|c| c.class == CdcClass::InputCrossing),
            "3FF chains are clean: {}",
            r.text()
        );
    }

    #[test]
    fn handshake_needs_review() {
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, input logic start, done_b_in, output logic ack_a);
  logic req_a, req_s0, req_s1;
  logic ack_b, ack_t0, ack_t1;
  always @(posedge clk_a) req_a <= start | (req_a & ~ack_t1);
  always @(posedge clk_b) begin req_s0 <= req_a; req_s1 <= req_s0; end
  always @(posedge clk_b) ack_b <= req_s1 & done_b_in;
  always @(posedge clk_a) begin ack_t0 <= ack_b; ack_t1 <= ack_t0; end
  assign ack_a = ack_t1;
endmodule"#,
        );
        let hs = of_class(&r, CdcClass::HandshakeNeedsReview);
        assert_eq!(hs.len(), 2, "{}", r.text());
        for h in &hs {
            assert_eq!(h.severity, CdcSeverity::Warning);
            assert_eq!(h.check, "HELION-CDC-5");
        }
        // The protocol is flagged, never declared safe: no bare skips hide it.
        assert_eq!(r.structural_warning_count(), hs.len(), "{}", r.text());
    }
}
