//! A for-loop needing more than 4096 unrolled iterations returned an empty
//! statement list with no diagnostic of its own, so the loop body simply
//! vanished. It is now reported as `unroll_cap` and stamped on the module.

use helion_sv::synth_sv;

fn synth(src: &str) -> helion_ir::Design {
    synth_sv(src, "t.sv").expect("synth")
}

fn loop_module(n: u64) -> String {
    format!(
        "module t(output logic [31:0] y); integer i;
           always_comb begin y = 0; for (i = 0; i < {n}; i = i + 1) y = y + 1; end
         endmodule"
    )
}

#[test]
fn loop_over_the_unroll_budget_is_stamped() {
    // Before: no attribute, no dedicated diagnostic -- the body was dropped
    // and the module only looked "empty".
    let d = synth(&loop_module(5000));
    assert_eq!(
        d.attrs.get("UNROLL_CAP"),
        Some("1"),
        "the 4096 unroll cap must be visible on the module"
    );
    assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
}

#[test]
fn just_over_the_budget_is_also_stamped() {
    let d = synth(&loop_module(4097));
    assert_eq!(d.attrs.get("UNROLL_CAP"), Some("1"), "4097 exceeds 4096");
}

#[test]
fn loop_within_the_budget_is_not_stamped() {
    // The same loop shape below the budget must not be flagged, so the
    // attribute really tracks the cap rather than any loop at all.
    let d = synth(&loop_module(8));
    assert_ne!(
        d.attrs.get("UNROLL_CAP"),
        Some("1"),
        "a small loop is unrolled, not capped"
    );
}
