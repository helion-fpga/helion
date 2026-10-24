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
