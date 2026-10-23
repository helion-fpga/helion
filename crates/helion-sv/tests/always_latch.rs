//! `always_latch` has no event control. It used to fall into the flop branch,
//! where the missing edge term meant a default clock was invented: the design
//! got an Hff whose CLK was a net `clk` that exists nowhere in the module, and
//! the latch enable was dropped on the floor.

mod common;
use helion_ir::CellKind;
use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

#[test]
fn always_latch_does_not_invent_a_clock_net() {
    let d = synth(
        "module t(input logic en, input logic d, output logic q);
           always_latch if (en) q <= d;
         endmodule",
    );
    // No flop may exist: there is no clock to drive it.
    let ffs: Vec<_> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .collect();
    assert!(
        ffs.is_empty(),
        "always_latch must not synthesise a flop with no clock"
    );
    // And no net named `clk` may be referenced by any pin.
    let dangling = d.nets.iter().any(|n| {
        n.name == "clk"
            && n.endpoints
                .iter()
                .any(|e| e.pin == "CLK" || e.pin == "I0")
    });
    assert!(!dangling, "no pin may be driven by an invented `clk` net");
    assert!(
        d.ports.iter().all(|p| p.name != "clk"),
        "no clk port may be invented"
    );
}

#[test]
fn always_latch_that_holds_is_refused_loudly() {
    let d = synth(
        "module t(input logic en, input logic d, output logic q);
           always_latch if (en) q <= d;
         endmodule",
    );
    assert_eq!(
        d.attrs.get("ASSIGN_NOT_LOWERED"),
        Some("1"),
        "a latch must be refused, not turned into a clocked flop"
    );
}

#[test]
fn always_latch_with_a_full_assignment_is_just_combinational() {
    // Every path assigns, so there is no hold and no latch.
    let d = synth(
        "module t(input logic en, input logic d, input logic e, output logic q);
           always_latch begin q = e; if (en) q = d; end
         endmodule",
    );
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(ffs, 0, "not a latch: no flop either");
    assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
}
