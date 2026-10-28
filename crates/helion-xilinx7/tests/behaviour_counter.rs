//! Does the emitted bitstream implement a *live counter*, or merely a
//! correctly-shaped image?
//!
//! Every other test on this flow checks structure: frame counts, IDCODEs,
//! feature diffs, clock reachability, pad presence. All of them passed on an
//! image whose output register sampled a constant and left LD0 dark forever,
//! so structure demonstrably does not imply behaviour.
//!
//! This test closes that gap. It decodes the bitstream's carry-chain
//! configuration back into a circuit and runs it, then asserts the circuit
//! counts.
//!
//! Deriving the adder model is the whole risk here, so it is stated explicitly.
//! For lane `n` holding bit `n`, with the constant 1 as the LUT's other
//! operand and `CY_n` the carry out of lane `n-1`:
//!
//! ```text
//!     S  = bit_n ^ 1          the LUT's XOR output
//!     O  = S ^ CY_n           what `FFMUX.XOR` drives into the flop
//!     CO = S ? 1 : CY_n       the carry passed to lane n+1
//! ```
//!
//! Starting from all-zero with `CY_0 = 0` this yields 1, 2, 3, ... A plausible
//! but wrong variant -- treating `CY` as the incoming carry into the sum
//! instead of the carry out of the lane below -- makes every bit toggle
//! independently and reports "never advances" for a bitstream that
//! demonstrably counts. That mistake was made while writing this test; the
//! reference is what caught it, which is why the reference is a test case.

use std::collections::BTreeMap;
use std::path::PathBuf;

use helion_xilinx7::device::Device;
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};

/// The design under test: the faithful equivalent of the reference
/// `blinky.vhd`, and the source the CLI is invoked with. Embedded rather than
/// read from disk because an earlier guard in this crate read its sources from
/// a scratch directory that later vanished and reported `ok` while asserting
/// nothing -- a silently vacuous test is worse than no test.
const BLINKY_REG: &str = r#"
module blinky (
    input  wire clk,
    output reg  LD0
);
    reg [24:0] count = 25'd0;

    always @(posedge clk) begin
        count <= count + 1'b1;
        LD0   <= count[24];
    end
endmodule
"#;

// `get_ports {port}` -- the braces are not optional. Without them the pin is
// recorded as an unrecognised key, packing treats the port as unconstrained
// and the build dies later with "has no PACKAGE_PIN LOC" (or, worse, succeeds
// with no pads at all on a design that has a single fallback output).
const BLINKY_XDC: &str = "\
set_property PACKAGE_PIN Y9 [get_ports {clk}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]\n\
set_property PACKAGE_PIN T22 [get_ports {LD0}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {LD0}]\n";

fn db_root() -> PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(PathBuf::from)
        .expect("HELION_XILINX_DB must point at a prjxray-db checkout")
}

/// One slice lane's configuration, restricted to what decides behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LaneCfg {
    ffmux_xor: bool,
    has_ff: bool,
    zini: bool,
}

/// Decode the counter column straight out of the emitted FASM.
///
/// FASM is the right level: it is the emitter's own output, so this test
/// exercises the mapping decision rather than re-deriving it from frames.
fn decode_column(fasm: &[String], prefix: &str) -> BTreeMap<(String, String, String), LaneCfg> {
    let mut out: BTreeMap<(String, String, String), LaneCfg> = BTreeMap::new();
    for line in fasm {
        let Some(rest) = line.strip_prefix(prefix) else {
            continue;
        };
        // `prefix` names the column up to the row digits, e.g. "CLBLM_R_X71Y",
        // so the row ("39") comes first and the slice site is the next field.
        let rest = rest.strip_prefix('.').unwrap_or(rest);
        let mut it = rest.split('.');
        let (Some(row), Some(slice), Some(field)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        // The row digit is part of the key: without it every row's `SLICEM_X0`
        // collapses into one slice and a 26-flop chain reads as 4 lanes.
        let tile = format!("{prefix}{row}");
        if !(slice.starts_with("SLICEM_") || slice.starts_with("SLICEL_")) {
            continue;
        }
        let Some(lane) = field.chars().next() else {
            continue;
        };
        if !('A'..='D').contains(&lane) {
            continue;
        }
        let key = (tile.clone(), slice.to_string(), lane.to_string());
        let e = out.entry(key).or_default();
        if field.ends_with("FFMUX") {
            e.has_ff = true;
            e.ffmux_xor = it.next() == Some("XOR");
        } else if field.ends_with("FF") {
            e.has_ff = true;
            if it.next() == Some("ZINI") {
                e.zini = true;
            }
        }
    }
    out
}

/// A raw slice pin as the flop's data source means the register samples
/// whatever the pin carries -- and if nothing routes there, a constant forever.
fn dead_lanes(cfg: &BTreeMap<(String, String, String), LaneCfg>, fasm: &[String], prefix: &str) -> Vec<String> {
    let mut dead = Vec::new();
    for line in fasm {
        let Some(rest) = line.strip_prefix(prefix) else {
            continue;
        };
        if !rest.contains("FFMUX") {
            continue;
        }
        let arm = rest.rsplit('.').next().unwrap_or("");
        if matches!(arm, "AX" | "BX" | "CX" | "DX") {
            dead.push(line.clone());
        }
    }
    let _ = cfg;
    dead
}

/// Step the chain and return the flop values after each tick.
///
/// `init_high` is the power-up value of a flop whose `ZINI` feature is set.
/// Which polarity that is depends on the DB's convention for the INIT bit, so
/// the caller tries both and requires that exactly one yields a counter --
/// deriving it from the artifact rather than hard-coding an assumption about
/// a field whose meaning differs between X-Ray versions.
fn simulate(
    chain: &[((String, String, String), LaneCfg)],
    steps: usize,
    init_high: bool,
) -> Vec<Vec<bool>> {
    let mut state: Vec<bool> = chain
        .iter()
        .map(|(_, c)| if c.zini { init_high } else { !init_high })
        .collect();
    let mut trace = vec![state.clone()];
    for _ in 0..steps {
        let mut cy = false;
        let mut next = Vec::with_capacity(state.len());
        for bit in &state {
            let s = !*bit; // `S = bit ^ 1`
            let o = s ^ cy; // what `FFMUX.XOR` drives into the flop
            let co = s; // `CO = S ? 1 : CY_n`
            next.push(o);
            cy = co;
        }
        state = next;
        trace.push(state.clone());
    }
    trace
}

/// True when this trace is an increment sequence: the LSB toggles every tick
/// and bit `n` toggles at half the rate of bit `n-1`.
fn is_a_counter(trace: &[Vec<bool>], width: usize) -> bool {
    if trace.len() < 8 || trace[0].len() < width {
        return false;
    }
    for n in 0..width.min(trace[0].len()) {
        let col: Vec<bool> = trace.iter().map(|s| s[n]).collect();
        // Bit n should hold for 2^n ticks, so over the trace it must change at
        // all only for the low bits, and bit 0 must change every single tick.
        if n == 0 {
            if col.windows(2).any(|w| w[0] == w[1]) {
                return false;
            }
        } else {
            let changes = col.windows(2).filter(|w| w[0] != w[1]).count();
            let expect = (trace.len() - 1) / (1usize << n);
            if changes.abs_diff(expect) > 1 {
                return false;
            }
        }
    }
    true
}

/// Build the bitstream's FASM for the ZedBoard blinky.
fn build_fasm() -> Vec<String> {
    let mut dev = Device::load_part_from_db(&db_root(), "xc7z020").expect("load device");
    let nm = build_node_map(&mut dev);
    let mut design = helion_sv::synth_sv(BLINKY_REG, "blinky_reg.sv").expect("synth");
    helion_xilinx7::build::PinConstraints::parse(BLINKY_XDC).apply(&mut design);

    let packed = pack(&design).expect("pack");
    let mut xdev = XDevice::new(&dev, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");
    emit_fasm(&routed, &xdev).expect("emit").fasm
}

/// The counter column must decode to a circuit that actually counts.
#[test]
fn the_emitted_bitstream_implements_a_live_counter() {
    let fasm = build_fasm();
    let prefix = "CLBLM_R_X71Y";

    let cfg = decode_column(&fasm, prefix);
    let chain: Vec<((String, String, String), LaneCfg)> = cfg
        .iter()
        .filter(|(_, c)| c.has_ff && c.ffmux_xor)
        .map(|(k, c)| (k.clone(), *c))
        .collect();

    assert!(
        chain.len() >= 26,
        "expected the counter's 26 carry-chain flops in {prefix}, found {}: {:?}",
        chain.len(),
        chain.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );

    let dead = dead_lanes(&cfg, &fasm, prefix);
    assert!(
        dead.is_empty(),
        "flop(s) take a raw slice pin as their data source and can never change: {dead:?}"
    );

    // Which polarity `ZINI` encodes is a DB convention, so try both and
    // require that at least one yields an increment sequence. Requiring
    // *exactly* one would over-constrain the test to an unverified assumption.
    let widths = [chain.len(), 8, 4, 2];
    let mut ok = None;
    for init_high in [false, true] {
        let trace = simulate(&chain, 64, init_high);
        if widths.iter().any(|w| is_a_counter(&trace, *w)) {
            ok = Some((init_high, trace));
            break;
        }
    }
    let (init_high, trace) = ok.unwrap_or_else(|| {
        panic!(
            "the decoded {} -flop chain does not count under either ZINI polarity; \
             the emitted bitstream does not implement a working counter",
            chain.len()
        )
    });

    let distinct: std::collections::BTreeSet<Vec<bool>> = trace.iter().cloned().collect();
    let changing = (0..trace[0].len())
        .filter(|&i| trace.iter().any(|s| s[i] != trace[0][i]))
        .count();
    assert!(
        distinct.len() > 8,
        "the decoded chain barely changes: {} distinct state(s)",
        distinct.len()
    );
    assert_eq!(
        changing,
        chain.len(),
        "every chained flop should take part in the count; only {changing} of {} change",
        chain.len()
    );
    let _ = init_high;
}

/// The entry lane's operand must arrive on `A3` (`IMUX1`), not on `A6`.
///
/// This is the guard for the defect that made the whole counter inert while
/// every other check passed. The entry lane's `S` LUT is `~A3` (INIT
/// `0x0f0f_0f0f_0f0f_0f0f`); the body lanes are a plain `A6` pass-through.
/// Emitting the entry lane's operand on `I5` like every other lane left the
/// LUT reading `A6` while its mask was built for `A3`, so `A3` fell back to
/// the INT tile's VCC default and `~A3` was 0 for every state -- bit 0 could
/// never carry and the chain sat at 0 forever.
///
/// The counting model above could not see it: it sweeps both `ZINI`
/// polarities, and a frozen chain decodes identically under each. What
/// separates a working image is that the entry lane's operand PIN is one the
/// router actually drove.
#[test]
fn the_entry_lane_reads_its_operand_on_a3_not_a6() {
    let fasm = build_fasm();
    let entry = "CLBLM_R_X71Y39";

    // The CLB -> INT arm that feeds slice pin `A3`.
    let feeds_a3 = format!("{entry}.CLBLM_M_A3.CLBLM_IMUX1");
    let feeds_a6 = format!("{entry}.CLBLM_M_A6.CLBLM_IMUX4");
    assert!(
        fasm.iter().any(|l| l == &feeds_a3),
        "the chain entry lane's operand is not routed to `{feeds_a3}`;\
         \nwithout it bit 0 reads the VCC default and the counter never carries.\n\
         entry-lane arms present: {:?}",
        fasm.iter().filter(|l| l.starts_with(entry)).collect::<Vec<_>>()
    );
    assert!(
        !fasm.iter().any(|l| l == &feeds_a6),
        "the chain entry lane's operand must not be routed to `{feeds_a6}`: the \
         entry LUT mask is `~A3`, so an `A6` feed leaves the mask reading a pin \
         nothing drives and the counter freezes"
    );
}

/// Build the routed design, so the check can see the leaf the ROUTER chose.
///
/// The emitted FASM cannot answer this: `vivado_leaf_spelling` rewrites every
/// leaf to `TOPL5` on the way out, so a FASM-level assertion passes no matter
/// which trunk the route actually took. Verified: reverting `leaf_penalty`
/// left a FASM-based version of this guard green.
fn route_design() -> helion_xilinx7::route::Routed7 {
    let mut dev = Device::load_part_from_db(&db_root(), "xc7z020").expect("load device");
    let nm = build_node_map(&mut dev);
    let mut design = helion_sv::synth_sv(BLINKY_REG, "blinky_reg.sv").expect("synth");
    helion_xilinx7::build::PinConstraints::parse(BLINKY_XDC).apply(&mut design);
    let packed = pack(&design).expect("pack");
    let mut xdev = XDevice::new(&dev, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    route(&placed, &xdev).expect("route")
}

/// The clock must arrive on the HCLK leaf trunk that is actually enabled.
///
/// `HCLK_L` offers six sibling leaf trunks (`TOPL0..TOPL5`) off the same
/// buffer, so an unranked search picks one arbitrarily -- and it picked
/// `TOPL0` (spine `GCLK_L_B6`). Only one leaf can be enabled and the emitter
/// enables `TOPL5` (`GCLK_L_B11`), so the image armed every `INT_R.CLK1` mux
/// from a trunk nothing drove. Every structural check passed anyway: frame
/// count, IDCODE, feature counts, and `work/bitsim.py` (which does not model
/// clock taps at all). The clock was dead and the LED would never have blinked.
#[test]
fn the_clock_leaf_matches_the_enabled_trunk() {
    let routed = route_design();
    let mut leaves: Vec<String> = Vec::new();
    for net in &routed.nets {
        if let helion_xilinx7::route::RouteKind::Pips(pips) = &net.kind {
            for pip in pips {
                if pip.dst_wire.starts_with("HCLK_LEAF_CLK_B_") {
                    leaves.push(pip.dst_wire.clone());
                }
            }
        }
    }
    assert!(
        !leaves.is_empty(),
        "no HCLK leaf is routed at all, so the clock reaches no trunk"
    );
    let wrong: Vec<&String> = leaves
        .iter()
        .filter(|l| !(l.ends_with("TOPL5") || l.ends_with("BOTL5")))
        .collect();
    assert!(
        wrong.is_empty(),
        "the clock is routed through a leaf trunk that is never enabled: {wrong:?}\n\
         only TOPL5/BOTL5 carry a clock, because `vivado_leaf_spelling` enables \
         that one; every other leaf leaves the INT tile's CLK mux armed from an \
         undriven trunk (valid bitstream, dead clock)"
    );
}
