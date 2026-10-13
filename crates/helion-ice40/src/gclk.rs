//! Global clock network planning for the iCE40 HX8K.
//!
//! A global clock travels: source -> global buffer -> `glb_netwk_N` ->
//! per-tile `glb_netwk_N -> lutff_global/clk`.
//!
//! Sources onto `glb_netwk_N` (Project IceStorm `icebox.py`, verified
//! against `/usr/bin/arachne-pnr` on clocked HX8K designs):
//! - An IO pin (or any fabric signal) routed to the `fabout` net of one of
//!   the 8 global-buffer IO tiles ([`GBUFIN_8K`], from
//!   `icebox.py::gbufin_db["8k"]`). The fabout->glb_netwk connection is
//!   physical and needs no config bits; arachne-pnr uses this path for
//!   pin-driven clocks.
//! - A PLL's `PLLOUTGLOBAL`, which lands on the `padin` wire of a fixed PIO
//!   and drives `glb_netwk_N` through the `padin_glb_netwk` extra bit
//!   ([`PADIN_EXTRA_BITS_8K`], from `icebox.py::extra_bits_db["8k"]` and
//!   `padin_pio_db["8k"]`). Bottom PLL -> glb net 6, top PLL -> glb net 7
//!   (matches arachne-pnr: `.extra_bit 0 870 271` = padin_glb_netwk 6).
//!
//! Distribution down each column is enabled by the `ColBufCtrl` bit for
//! `glb_netwk_N` in the column-buffer tile covering the consumer tile
//! (`icebox.py::colbuf_db["8k"]`: buffer rows 8, 9, 24, 25).

use crate::pack::PllKind;
use crate::place::PlacedIce;

/// Global-buffer IO tiles by glb net index: `(tile_x, tile_y)`.
/// From `icebox.py::gbufin_db["8k"]` (`(x, y, glb)` triples).
pub const GBUFIN_8K: [(u8, u8); 8] = [
    (17, 0),  // glb 0
    (17, 33), // glb 1
    (33, 17), // glb 2
    (0, 17),  // glb 3
    (16, 33), // glb 4
    (16, 0),  // glb 5
    (0, 16),  // glb 6
    (33, 16), // glb 7
];

/// `padin_glb_netwk` extra-bit `(bank, row, bit)` by glb net index.
/// From `icebox.py::extra_bits_db["8k"]`; emitted as
/// `.extra_bit <bank> <row> <bit>` at the end of the `.asc`.
pub const PADIN_EXTRA_BITS_8K: [(u8, u16, u16); 8] = [
    (0, 870, 270), // glb 0
    (0, 871, 270), // glb 1
    (1, 870, 271), // glb 2
    (1, 871, 271), // glb 3
    (1, 870, 270), // glb 4
    (1, 871, 270), // glb 5
    (0, 870, 271), // glb 6
    (0, 871, 271), // glb 7
];

/// Glb net driven by a PLL's `PLLOUTGLOBAL` via `padin_glb_netwk`.
/// The bottom PLL's `PLLOUT_A` is padin_1 of IO tile (16,0), which
/// `padin_pio_db["8k"]` maps to glb net 6; the top PLL's is (16,33,1),
/// mapped to glb net 7.
pub fn pll_glb_net(kind: PllKind) -> u8 {
    match kind {
        PllKind::Pad => 6,
        PllKind::Core => 7,
    }
}

/// Column-buffer tile row covering logic-tile row `y` on the HX8K.
/// Ports `icebox.py::colbuf_db["8k"]`: rows 0-8 <- 8, 9-16 <- 9,
/// 17-24 <- 24, 25-33 <- 25.
pub fn colbuf_src_y(y: u8) -> u8 {
    if y <= 8 {
        8
    } else if y <= 16 {
        9
    } else if y <= 24 {
        24
    } else {
        25
    }
}

/// How a planned global clock gets onto its `glb_netwk` wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockSource {
    /// Route the source net to `fabout` of this global-buffer IO tile
    /// (arachne-pnr's path for pin- and fabric-driven clocks).
    GlobalBuffer { gx: u8, gy: u8 },
    /// PLL `PLLOUTGLOBAL` via the `padin_glb_netwk` extra bit: no routing.
    Pll,
}

/// One assigned global clock.
#[derive(Clone, Debug)]
pub struct ClockPlan {
    /// Global net index 0..7.
    pub glb: u8,
    pub source: ClockSource,
    /// Net names carried on this global clock (the ClockBuf's `out_net`,
    /// plus the PLL's `out_net` when a ClockBuf follows a PLL).
    pub nets: Vec<String>,
    /// Source net to route toward the global buffer's `fabout`
    /// (the ClockBuf's `in_net`); `None` for PLL-driven clocks.
    pub source_net: Option<String>,
    /// The `padin_glb_netwk` extra bit for PLL-driven clocks.
    pub extra_bit: Option<(u8, u16, u16)>,
}

/// Assign each clock to a global network.
///
/// `PackedClk` markers and PLL outputs each get one of the 8 global nets.
/// Fails loud when a clock source has no driver or when more distinct
/// clock nets exist than the 8 global buffers.
pub fn plan(placed: &PlacedIce) -> Result<Vec<ClockPlan>, String> {
    let mut plans: Vec<ClockPlan> = Vec::new();
    let mut used = [false; 8];
    // Net name -> plan index, covering aliases (PLL out_net feeding a ClockBuf).
    let mut net_plan: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // PLLs first: their glb nets are fixed by the padin wiring.
    for pll in &placed.packed.plls {
        let glb = pll_glb_net(pll.kind);
        if used[glb as usize] {
            return Err(format!(
                "gclk: glb_netwk_{glb} already used by another clock (PLL {})",
                pll.name
            ));
        }
        used[glb as usize] = true;
        net_plan.insert(pll.out_net.clone(), plans.len());
        plans.push(ClockPlan {
            glb,
            source: ClockSource::Pll,
            nets: vec![pll.out_net.clone()],
            source_net: None,
            extra_bit: Some(PADIN_EXTRA_BITS_8K[glb as usize]),
        });
    }

    // ClockBuf markers.
    for clk in &placed.packed.clks {
        // A ClockBuf on a PLL output shares the PLL's global net.
        if let Some(&pi) = net_plan.get(&clk.in_net) {
            if plans[pi].source != ClockSource::Pll {
                return Err(format!(
                    "gclk: ClockBuf {} input net '{}' is ambiguous",
                    clk.name, clk.in_net
                ));
            }
            net_plan.insert(clk.out_net.clone(), pi);
            plans[pi].nets.push(clk.out_net.clone());
            continue;
        }
        // Find the source driver position for global-buffer selection.
        let driver_pos = driver_position(placed, &clk.in_net).ok_or_else(|| {
            format!(
                "gclk: ClockBuf {} input net '{}' has no driver",
                clk.name, clk.in_net
            )
        })?;
        // Prefer the global-buffer tile co-located with the driver;
        // otherwise take the nearest free one.
        let mut best: Option<(u32, u8)> = None;
        for (glb, &(gx, gy)) in GBUFIN_8K.iter().enumerate() {
            if used[glb] {
                continue;
            }
            let dist = (gx as i32 - driver_pos.0 as i32).unsigned_abs()
                + (gy as i32 - driver_pos.1 as i32).unsigned_abs();
            // Co-located (dist 0) always wins; otherwise nearest.
            let better = match best {
                None => true,
                Some((bd, _)) => dist < bd,
            };
            if better {
                best = Some((dist, glb as u8));
            }
        }
        let (_, glb) = best.ok_or_else(|| {
            format!(
                "gclk: more distinct clock nets than the 8 global buffers \
                 (ClockBuf {} net '{}' has nowhere to go)",
                clk.name, clk.out_net
            )
        })?;
        used[glb as usize] = true;
        let (gx, gy) = GBUFIN_8K[glb as usize];
        net_plan.insert(clk.out_net.clone(), plans.len());
        plans.push(ClockPlan {
            glb,
            source: ClockSource::GlobalBuffer { gx, gy },
            nets: vec![clk.out_net.clone()],
            source_net: Some(clk.in_net.clone()),
            extra_bit: None,
        });
    }

    Ok(plans)
}

/// Position of the driver of `net`: IO pin tile, or the LC tile of a
/// LUT/FF output. `None` when the net has no fabric driver.
fn driver_position(placed: &PlacedIce, net: &str) -> Option<(u8, u8)> {
    for io in &placed.ios {
        if io.net == net && io.dir == crate::pack::IoDir::In {
            return Some((io.x, io.y));
        }
    }
    for lut in &placed.luts {
        if lut.output_net == net {
            return Some((lut.x, lut.y));
        }
    }
    for ff in &placed.ffs {
        if ff.q_net == net {
            return Some((ff.x, ff.y));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colbuf_rows_match_icebox() {
        // icebox.py colbuf_db["8k"]: 0-8 <- 8, 9-16 <- 9, 17-24 <- 24, 25-33 <- 25.
        for y in 0..=8 {
            assert_eq!(colbuf_src_y(y), 8);
        }
        for y in 9..=16 {
            assert_eq!(colbuf_src_y(y), 9);
        }
        for y in 17..=24 {
            assert_eq!(colbuf_src_y(y), 24);
        }
        for y in 25..=33 {
            assert_eq!(colbuf_src_y(y), 25);
        }
    }

    #[test]
    fn pll_glb_nets_match_padin_pio_db() {
        // padin_pio_db["8k"]: glb 6 -> (16,0,1), glb 7 -> (16,33,1):
        // the bottom/top PLL PLLOUT_A PIOs.
        assert_eq!(pll_glb_net(PllKind::Pad), 6);
        assert_eq!(pll_glb_net(PllKind::Core), 7);
        assert_eq!(PADIN_EXTRA_BITS_8K[6], (0, 870, 271));
        assert_eq!(PADIN_EXTRA_BITS_8K[7], (0, 871, 271));
    }
}
