//! Pack stage: [`helion_ir::Design`] → [`PackedIce`].
//!
//! Maps the vendor-neutral IR onto iCE40 primitives:
//! - `Lut6` is Shannon-decomposed into LUT4s (iCE40 LUTs are 4-input).
//! - `Hff` becomes a logic-cell flip-flop, paired with its driver LUT when
//!   possible.
//! - `IobIn`/`IobOut` become `SB_IO` blocks; `ClockBuf` becomes a global
//!   clock net; `Bram18` becomes `SB_RAM40_4K`; a `BlackBox` named
//!   `SB_PLL40*` becomes a PLL.
//! - `Mac27` and `Ila` are loud errors: HX8K has no DSP blocks and no ILA.

use helion_ir::{CellKind, Design};

/// A packed iCE40 LUT4 (one logic cell, combinational).
#[derive(Clone, Debug)]
pub struct PackedLut {
    /// Source IR cell name (synthesized names for decomposition products).
    pub name: String,
    /// 16-bit truth table: `init[n]` is the output for input value `n`
    /// where `n = in_3*8 + in_2*4 + in_1*2 + in_0`.
    pub init: u16,
    /// Nets on in_0..in_3 (`None` = unconnected, tied low at emission).
    pub input_nets: [Option<String>; 4],
    /// Net on out.
    pub output_net: String,
}

/// A packed iCE40 flip-flop (one logic cell, sequential).
#[derive(Clone, Debug)]
pub struct PackedFf {
    pub name: String,
    pub d_net: String,
    pub q_net: String,
    pub clk_net: String,
}

/// A packed SB_IO block.
#[derive(Clone, Debug)]
pub struct PackedIo {
    pub name: String,
    pub port: String,
    pub dir: IoDir,
    /// Fabric-side net (`D_OUT_0` for outputs, `D_IN_0` for inputs).
    pub net: String,
    /// CT256 package pin, e.g. `"B5"`. Resolved from the port name via the
    /// breakout-board map; explicit `LOC` attributes win when present.
    pub pin: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoDir {
    In,
    Out,
}

/// A packed global clock: the net driven onto `lutff_global/clk`.
#[derive(Clone, Debug)]
pub struct PackedClk {
    pub name: String,
    /// Net into the clock root (the IO pin net).
    pub in_net: String,
    /// Global clock net name.
    pub out_net: String,
}

/// A packed SB_RAM40_4K block.
#[derive(Clone, Debug)]
pub struct PackedRam {
    pub name: String,
    /// 16-bit write/read mode: 0 = 16-bit, 1 = 8-bit, 2 = 4-bit, 3 = 2-bit.
    pub write_mode: u8,
    pub read_mode: u8,
    pub rdata_net: [String; 16],
    pub wdata_net: [String; 16],
    pub raddr_net: [String; 11],
    pub waddr_net: [String; 11],
    pub rclk_net: String,
    pub wclk_net: String,
    pub re_net: Option<String>,
    pub we_net: Option<String>,
}

/// A packed SB_PLL40.
#[derive(Clone, Debug)]
pub struct PackedPll {
    pub name: String,
    pub kind: PllKind,
    pub ref_net: String,
    pub out_net: String,
    /// Desired output frequency in Hz (from the `FREQ` attribute, if any).
    pub out_freq_hz: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PllKind {
    Pad,
    Core,
}

/// Backend-owned packed netlist for the iCE40 HX8K.
#[derive(Clone, Debug, Default)]
pub struct PackedIce {
    pub luts: Vec<PackedLut>,
    pub ffs: Vec<PackedFf>,
    pub ios: Vec<PackedIo>,
    pub clks: Vec<PackedClk>,
    pub rams: Vec<PackedRam>,
    pub plls: Vec<PackedPll>,
}

/// Decompose a 6-LUT truth table into LUT4s via Shannon decomposition.
///
/// Returns `(luts, top_output_net)`: the list of LUT4s (with fresh internal
/// nets) and the net carrying the function output. Uses at most 7 LUT4s;
/// fewer when the function ignores inputs.
fn decompose_lut6(name: &str, init: u64, inputs: &[Option<String>; 6]) -> Vec<PackedLut> {
    // Collect the actually-used input positions.
    let used: Vec<usize> = (0..6).filter(|&i| inputs[i].is_some()).collect();
    if used.is_empty() {
        // Constant function: single LUT4 tied off.
        let out = (init & 1) != 0;
        return vec![PackedLut {
            name: format!("{name}$const"),
            init: if out { 0xFFFF } else { 0x0000 },
            input_nets: [None, None, None, None],
            output_net: format!("{name}$O"),
        }];
    }
    if used.len() <= 4 {
        // Direct map onto one LUT4, remapping the truth table.
        let mut lut_init: u16 = 0;
        for n in 0..16u16 {
            // n's bits address used[0..k]; build the 6-bit address.
            let mut addr6: u64 = 0;
            for (k, &pos) in used.iter().enumerate() {
                if (n >> k) & 1 == 1 {
                    addr6 |= 1 << pos;
                }
            }
            if (init >> addr6) & 1 == 1 {
                lut_init |= 1 << n;
            }
        }
        let mut in_nets: [Option<String>; 4] = [None, None, None, None];
        for (k, &pos) in used.iter().enumerate() {
            in_nets[k] = inputs[pos].clone();
        }
        return vec![PackedLut {
            name: format!("{name}$lut"),
            init: lut_init,
            input_nets: in_nets,
            output_net: format!("{name}$O"),
        }];
    }
    // Shannon decomposition on the highest used input.
    // f = sel ? f1 : f0, each cofactor has one fewer input.
    let sel_pos = used[used.len() - 1];
    let sel_net = inputs[sel_pos].clone().unwrap();
    // Cofactor truth tables, with the sel bit compressed out: for each
    // minterm where f = 1, drop bit sel_pos to get the 5-bit cofactor
    // address, and set it in c0 (sel = 0) or c1 (sel = 1).
    let mut c0: u64 = 0;
    let mut c1: u64 = 0;
    for a in 0..64u64 {
        if (init >> a) & 1 == 0 {
            continue;
        }
        let sel = (a >> sel_pos) & 1;
        let lo = a & ((1 << sel_pos) - 1);
        let hi = a >> (sel_pos + 1);
        let ca = lo | (hi << sel_pos);
        if sel == 0 {
            c0 |= 1 << ca;
        } else {
            c1 |= 1 << ca;
        }
    }
    let mut in0 = inputs.clone();
    let mut in1 = inputs.clone();
    in0[sel_pos] = None;
    in1[sel_pos] = None;
    let mut luts = decompose_lut6(&format!("{name}$f0"), c0, &in0);
    let out0 = luts.last().unwrap().output_net.clone();
    let mut luts1 = decompose_lut6(&format!("{name}$f1"), c1, &in1);
    let out1 = luts1.last().unwrap().output_net.clone();
    luts.append(&mut luts1);
    // Mux: sel ? out1 : out0. LUT4(sel, out0, out1) = (sel & out1) | (!sel & out0).
    // init bit n (n = in_2*4 + in_1*2 + in_0): in_0=sel, in_1=out0, in_2=out1.
    let mut mux_init: u16 = 0;
    for n in 0..16u16 {
        let sel = n & 1;
        let a = (n >> 1) & 1;
        let b = (n >> 2) & 1;
        let o = if sel == 1 { b } else { a };
        if o == 1 {
            mux_init |= 1 << n;
        }
    }
    luts.push(PackedLut {
        name: format!("{name}$mux"),
        init: mux_init,
        input_nets: [Some(sel_net), Some(out0), Some(out1), None],
        output_net: format!("{name}$O"),
    });
    luts
}

/// Pack a [`Design`] into iCE40 primitives.
pub fn pack(netlist: &Design) -> Result<PackedIce, String> {
    let pins = netlist.pin_index();
    let mut out = PackedIce::default();

    for cell in &netlist.cells {
        match &cell.kind {
            CellKind::Lut6 { init } => {
                let mut inputs: [Option<String>; 6] = Default::default();
                for (i, slot) in inputs.iter_mut().enumerate() {
                    *slot = pins
                        .net_on(&cell.name, &format!("I{i}"))
                        .map(|s| s.to_string());
                }
                let output_net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: LUT {} has no O net", cell.name))?
                    .to_string();
                let mut luts = decompose_lut6(&cell.name, *init, &inputs);
                // Rename the top output net to the real one.
                if let Some(top) = luts.last_mut() {
                    top.output_net = output_net;
                }
                out.luts.append(&mut luts);
            }
            CellKind::Hff => {
                let need = |pin: &str| {
                    pins.net_on(&cell.name, pin)
                        .map(|s| s.to_string())
                        .ok_or_else(|| format!("pack: FF {} has no {pin} net", cell.name))
                };
                out.ffs.push(PackedFf {
                    name: cell.name.clone(),
                    d_net: need("D")?,
                    q_net: need("Q")?,
                    clk_net: need("CLK")?,
                });
            }
            CellKind::IobOut => {
                let net = pins
                    .net_on(&cell.name, "I")
                    .ok_or_else(|| format!("pack: OBUF {} has no I net", cell.name))?
                    .to_string();
                let port = pins
                    .net_on(&cell.name, "PAD")
                    .unwrap_or(&cell.name)
                    .to_string();
                out.ios.push(PackedIo {
                    name: cell.name.clone(),
                    pin: resolve_pin(netlist, &port)?,
                    port,
                    dir: IoDir::Out,
                    net,
                });
            }
            CellKind::IobIn => {
                let net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: IBUF {} has no O net", cell.name))?
                    .to_string();
                let port = pins
                    .net_on(&cell.name, "PAD")
                    .unwrap_or(&cell.name)
                    .to_string();
                out.ios.push(PackedIo {
                    name: cell.name.clone(),
                    pin: resolve_pin(netlist, &port)?,
                    port,
                    dir: IoDir::In,
                    net,
                });
            }
            CellKind::ClockBuf => {
                let in_net = pins
                    .net_on(&cell.name, "I")
                    .ok_or_else(|| format!("pack: CLKBUF {} has no I net", cell.name))?
                    .to_string();
                let out_net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: CLKBUF {} has no O net", cell.name))?
                    .to_string();
                out.clks.push(PackedClk {
                    name: cell.name.clone(),
                    in_net,
                    out_net,
                });
            }
            CellKind::Bram18 => {
                out.rams.push(pack_ram(&cell.name, &pins)?);
            }
            CellKind::BlackBox { module } if module.starts_with("SB_PLL40") => {
                let kind = if module.contains("PAD") {
                    PllKind::Pad
                } else {
                    PllKind::Core
                };
                let ref_net = pins
                    .net_on(&cell.name, "REFERENCECLK")
                    .or_else(|| pins.net_on(&cell.name, "I"))
                    .ok_or_else(|| format!("pack: PLL {} has no clock input", cell.name))?
                    .to_string();
                let out_net = pins
                    .net_on(&cell.name, "PLLOUTGLOBAL")
                    .or_else(|| pins.net_on(&cell.name, "O"))
                    .ok_or_else(|| format!("pack: PLL {} has no output", cell.name))?
                    .to_string();
                out.plls.push(PackedPll {
                    name: cell.name.clone(),
                    kind,
                    ref_net,
                    out_net,
                    out_freq_hz: 0,
                });
            }
            CellKind::Mac27 => {
                return Err(format!(
                    "pack: MAC27 {} unsupported: iCE40 HX8K has no DSP blocks",
                    cell.name
                ));
            }
            CellKind::Ila { .. } => {
                return Err(format!(
                    "pack: ILA {} unsupported on the iCE40 backend",
                    cell.name
                ));
            }
            other => {
                return Err(format!(
                    "pack: unsupported CellKind {other:?} for iCE40 (cell {})",
                    cell.name
                ));
            }
        }
    }

    // The ClockBuf is a logical global-clock marker. This backend routes
    // clocks as ordinary nets (via local tracks to lutff_global/clk), so
    // merge each ClockBuf's output net into its input net.
    for clk in &out.clks {
        for ff in out.ffs.iter_mut() {
            if ff.clk_net == clk.out_net {
                ff.clk_net = clk.in_net.clone();
            }
        }
    }

    // Pair each FF with its driver LUT when the driver is a packed LUT4 with
    // no other loads: they share one logic cell (LUT -> FF in the same LC).
    // (Pairing is recorded at place time; pack just keeps both lists.)

    Ok(out)
}

/// Resolve a top-level port to a CT256 package pin.
fn resolve_pin(netlist: &Design, port: &str) -> Result<String, String> {
    if let Some(def) = netlist.ports.iter().find(|p| p.name == port) {
        if let Some(loc) = def.attrs.get("LOC") {
            return Ok(loc.to_string());
        }
        // Breakout-board convention: clk -> J3, ledN -> LEDN pin.
        let map = crate::device::ct256_pin_map();
        if port == "clk" {
            return Ok("J3".to_string());
        }
        if let Some(led) = port.strip_prefix("led") {
            let names = ["B5", "B4", "A2", "A1", "C5", "C4", "B3", "C3"];
            if let Ok(n) = led.parse::<usize>() {
                if let Some(pin) = names.get(n) {
                    let _ = map;
                    return Ok(pin.to_string());
                }
            }
        }
        // Heuristic: port name matches a package pin directly.
        if map.contains_key(port) {
            return Ok(port.to_string());
        }
    }
    Err(format!(
        "pack: no package pin for port '{port}': add a LOC attribute or use the breakout-board naming (clk, led0..led7)"
    ))
}

/// Pack a Bram18 cell into SB_RAM40_4K shape.
fn pack_ram(
    name: &str,
    pins: &helion_ir::PinIndex,
) -> Result<PackedRam, String> {
    let bus = |prefix: &str, n: usize| -> Result<[String; 16], String> {
        let mut arr: [String; 16] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            if i < n {
                *slot = pins
                    .net_on(name, &format!("{prefix}{i}"))
                    .unwrap_or("gnd")
                    .to_string();
            } else {
                *slot = "gnd".to_string();
            }
        }
        Ok(arr)
    };
    let addr = |prefix: &str| -> Result<[String; 11], String> {
        let mut arr: [String; 11] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            *slot = pins
                .net_on(name, &format!("{prefix}{i}"))
                .unwrap_or("gnd")
                .to_string();
        }
        Ok(arr)
    };
    let need = |pin: &str| {
        pins.net_on(name, pin)
            .map(|s| s.to_string())
            .ok_or_else(|| format!("pack: RAM {name} has no {pin} net"))
    };
    Ok(PackedRam {
        name: name.to_string(),
        write_mode: 0,
        read_mode: 0,
        rdata_net: bus("DO", 16)?,
        wdata_net: bus("DI", 16)?,
        raddr_net: addr("RADDR")?,
        waddr_net: addr("WADDR")?,
        rclk_net: need("RCLK").unwrap_or_else(|_| "gnd".to_string()),
        wclk_net: need("WCLK").unwrap_or_else(|_| "gnd".to_string()),
        re_net: pins.net_on(name, "RE").map(|s| s.to_string()),
        we_net: pins.net_on(name, "WE").map(|s| s.to_string()),
    })
}

/// Map a LUT4 16-bit init to the 20 LC config bits.
///
/// Truth-table row `n` (n = in_3*8+in_2*4+in_1*2+in_0) lives at the LC bit
/// given by the IceStorm logic-tile documentation table.
pub fn lut4_init_to_lc_bits(init: u16) -> [bool; 20] {
    // row n -> LC bit index, from the docs table.
    const ROW_TO_LC: [usize; 16] = [4, 14, 15, 5, 6, 16, 17, 7, 3, 13, 12, 2, 1, 11, 10, 0];
    let mut bits = [false; 20];
    for n in 0..16 {
        bits[ROW_TO_LC[n]] = (init >> n) & 1 == 1;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut4_passthrough_decomposition() {
        // f = in_0 (buffer). init bit n = bit0 of n.
        let init: u64 = 0xAAAAAAAAAAAAAAAA;
        let inputs: [Option<String>; 6] = [
            Some("a".into()),
            None,
            None,
            None,
            None,
            None,
        ];
        let luts = decompose_lut6("u", init, &inputs);
        assert_eq!(luts.len(), 1);
        // LUT4(a) = a: init bit n = (n>>0)&1.
        assert_eq!(luts[0].init, 0xAAAA);
        assert_eq!(luts[0].input_nets[0], Some("a".to_string()));
    }

    #[test]
    fn lut6_xor_decomposition_is_correct() {
        // 6-input XOR: evaluate decomposed netlist in software.
        let mut init: u64 = 0;
        for a in 0..64u64 {
            if a.count_ones() % 2 == 1 {
                init |= 1 << a;
            }
        }
        let inputs: [Option<String>; 6] = [
            Some("i0".into()),
            Some("i1".into()),
            Some("i2".into()),
            Some("i3".into()),
            Some("i4".into()),
            Some("i5".into()),
        ];
        let luts = decompose_lut6("x", init, &inputs);
        // Evaluate: map net name -> value.
        for a in 0..64u64 {
            let mut vals: std::collections::HashMap<String, bool> = (0..6)
                .map(|i| (format!("i{i}"), (a >> i) & 1 == 1))
                .collect();
            for lut in &luts {
                let mut n = 0u16;
                for (k, net) in lut.input_nets.iter().enumerate() {
                    if let Some(net) = net {
                        if vals.get(net).copied().unwrap_or(false) {
                            n |= 1 << k;
                        }
                    }
                }
                vals.insert(lut.output_net.clone(), (lut.init >> n) & 1 == 1);
            }
            let got = vals["x$O"];
            let want = a.count_ones() % 2 == 1;
            assert_eq!(got, want, "mismatch at input {a:06b}");
        }
    }

    #[test]
    fn lc_bit_mapping_spot_check() {
        // init = 1 only for n=0 -> LC[4] set.
        let bits = lut4_init_to_lc_bits(0x0001);
        assert!(bits[4]);
        assert_eq!(bits.iter().filter(|&&b| b).count(), 1);
        // init = all ones -> 16 bits set.
        let bits = lut4_init_to_lc_bits(0xFFFF);
        assert_eq!(bits.iter().filter(|&&b| b).count(), 16);
    }
}
