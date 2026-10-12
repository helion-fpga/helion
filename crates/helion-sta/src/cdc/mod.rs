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

pub use classify::classify_crossings;
pub use crossings::{find_crossings, CdcClass, CdcCrossing, CrossingSource, RawCrossing};
pub use domains::{extract_domains, ClockDomain, DomainKind, DomainMap};
pub use synchronizers::{is_mux_select, sync_chain, SyncChain};
// Legacy pair-level helpers, moved into `cdc::` (still crate-internal).
pub(crate) use domains::{clock_ff_loads, clock_net_name};
pub(crate) use synchronizers::cdc_synchronizer;

#[cfg(test)]
mod scratch_tests {    use super::synchronizers::ff_d_source_ff;
    use helion_ir::Design;

    fn dump(design: &Design) {
        for c in &design.cells {
            let clk = design.net_on(&c.name, "CLK").unwrap_or("-");
            let dd = design.net_on(&c.name, "D").unwrap_or("-");
            let q = design.net_on(&c.name, "Q").unwrap_or("-");
            eprintln!("CELL {} {:?} CLK={} D={} Q={}", c.name, c.kind, clk, dd, q);
            if let helion_ir::CellKind::Lut6 { init } = c.kind {
                let mut ins = Vec::new();
                for p in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                    if let Some(n) = design.net_on(&c.name, p) {
                        ins.push(format!("{p}={n}"));
                    }
                }
                eprintln!("    init={init:#018x} {}", ins.join(" "));
            }
        }
        eprintln!("--- nets ---");
        for n in &design.nets {
            let eps: Vec<String> =
                n.endpoints.iter().map(|e| format!("{}/{}", e.cell, e.pin)).collect();
            eprintln!("NET {} :: {}", n.name, eps.join(" "));
        }
        eprintln!("--- ports ---");
        for p in &design.ports {
            eprintln!("PORT {} {:?}", p.name, p.dir);
        }
    }

    #[test]
    fn scratch_dump_netlists() {
        let cases: &[(&str, &str)] = &[
            (
                "unsync",
                r#"module top(input logic clk_a, clk_b, output logic q);
  logic [3:0] cnt;
  always @(posedge clk_a) cnt <= cnt + 1'b1;
  always @(posedge clk_b) q <= cnt[0];
endmodule"#,
            ),
            (
                "single_ff",
                r#"module top(input logic clk_a, clk_b, input logic d, output logic q);
  logic launch;
  always @(posedge clk_a) launch <= d;
  always @(posedge clk_b) q <= launch;
endmodule"#,
            ),
            (
                "two_ff",
                r#"module top(input logic clk_a, clk_b, input logic d, output logic q);
  logic launch, s0;
  always @(posedge clk_a) launch <= d;
  always @(posedge clk_b) s0 <= launch;
  assign q = s0;
endmodule"#,
            ),
            (
                "muxed",
                r#"module top(input logic clk_a, clk_b, input logic sel_in, d0_in, d1_in, output logic q);
  logic sel_a, d0_b, d1_b, mux_q;
  always @(posedge clk_a) sel_a <= sel_in;
  always @(posedge clk_b) begin d0_b <= d0_in; d1_b <= d1_in; end
  always @(posedge clk_b) mux_q <= sel_a ? d0_b : d1_b;
  assign q = mux_q;
endmodule"#,
            ),
            (
                "bus4",
                r#"module top(input logic clk_a, clk_b, output logic [3:0] q);
  logic [3:0] cnt;
  always @(posedge clk_a) cnt <= cnt + 1'b1;
  always @(posedge clk_b) q <= cnt;
endmodule"#,
            ),
        ];
        for (name, src) in cases {
            eprintln!("=========== {name} ===========");
            let d = helion_sv::synth_sv(src, "scratch.sv").expect("synth failed");
            dump(&d);
            for c in &d.cells {
                if matches!(c.kind, helion_ir::CellKind::Hff) {
                    eprintln!(
                        "ff_d_source_ff({}) = {:?}",
                        c.name,
                        ff_d_source_ff(&d, &c.name)
                    );
                }
            }
        }
    }
}

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
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, output logic q);
  logic [3:0] cnt;
  always @(posedge clk_a) cnt <= cnt + 1'b1;
  always @(posedge clk_b) q <= cnt[0];
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
    fn multibit_bus_aggregate() {
        let r = report_two_clocks(
            r#"module top(input logic clk_a, clk_b, output logic [3:0] q);
  logic [3:0] cnt;
  always @(posedge clk_a) cnt <= cnt + 1'b1;
  always @(posedge clk_b) q <= cnt;
endmodule"#,
        );
        let unsync = of_class(&r, CdcClass::UnsyncCrossing);
        assert_eq!(unsync.len(), 4, "{}", r.text());
        let agg = of_class(&r, CdcClass::MultiBitUnsync);
        assert_eq!(agg.len(), 1, "{}", r.text());
        assert_eq!(agg[0].bus.as_deref(), Some("cnt"));
        assert_eq!(agg[0].severity, CdcSeverity::Warning);
        assert_eq!(agg[0].check, "HELION-CDC-3");
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
        assert!(
            r.crossings.is_empty(),
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
