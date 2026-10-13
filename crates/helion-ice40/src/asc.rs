//! ASC emission: [`RoutedIce`] → IceStorm `.asc` text bitstream.
//!
//! Sets routing bits from the routed edges, LC config bits (LUT truth
//! tables, DffEnable) from placement, and IO PINTYPE bits from the pin
//! directions. The `.asc` is converted to binary with IceStorm's `icepack`.

use crate::device::{tile_db, tile_kind, TileKind, PINTYPE_INPUT, PINTYPE_OUTPUT};
use crate::pack::{lut4_init_to_lc_bits, IoDir};
use crate::place::CarryMuxMode;
use crate::route::{RoutedIce, Router};
use std::collections::{HashMap, HashSet};

/// A tile's config bits: banks of bit vectors.
struct TileBits {
    kind: TileKind,
    banks: Vec<Vec<bool>>,
}

impl TileBits {
    fn new(kind: TileKind) -> Self {
        let (nbanks, width) = match kind {
            TileKind::Logic => (16, 54),
            TileKind::Ramb | TileKind::Ramt => (16, 42),
            TileKind::IoTop | TileKind::IoBottom | TileKind::IoLeft | TileKind::IoRight => (16, 18),
        };
        TileBits {
            kind,
            banks: vec![vec![false; width]; nbanks],
        }
    }

    fn set(&mut self, bank: u8, bit: u8) {
        if (bank as usize) < self.banks.len() && (bit as usize) < self.banks[bank as usize].len() {
            self.banks[bank as usize][bit as usize] = true;
        }
    }
}

/// IOB_N PINTYPE bit positions: `(iob, pintype_bit)` -> `(bank, bit)`.
fn iob_pintype_bits(kind: TileKind) -> HashMap<(u8, u8), (u8, u8)> {
    let mut m = HashMap::new();
    for e in tile_db(kind) {
        // Entries look like (["B3[17]"], Other("IOB_0"), src="PINTYPE_0").
        let (iob, pintype) = match &e.func {
            crate::db::DbFunc::Other(f) if f.starts_with("IOB_") => {
                let iob: u8 = f.strip_prefix("IOB_").and_then(|s| s.parse().ok()).unwrap_or(99);
                let pb: u8 = e
                    .src
                    .strip_prefix("PINTYPE_")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(99);
                (iob, pb)
            }
            _ => continue,
        };
        if iob > 1 || pintype > 5 {
            continue;
        }
        if let Some(b) = e.bits.first() {
            m.insert((iob, pintype), (b.row, b.bit));
        }
    }
    m
}

/// Config bits to set for a buffer `(src -> dst)` in a logic tile.
/// Returns an error if the DB has no such entry (a bug, not a silent skip).
fn buffer_set_bits(src: &str, dst: &str) -> Result<Vec<(u8, u8)>, String> {
    let entry = tile_db(TileKind::Logic).iter().find(|e| {
        matches!(e.func, crate::db::DbFunc::Buffer) && e.src == src && e.dst == dst
    });
    match entry {
        Some(e) => Ok(e
            .bits
            .iter()
            .filter(|b| b.val)
            .map(|b| (b.row, b.bit))
            .collect()),
        None => Err(format!(
            "ice40: no buffer DB entry for {src} -> {dst} (carry wiring)"
        )),
    }
}

/// The `CarryInSet` bit (B1[50]).
fn carry_in_set_bit() -> Result<(u8, u8), String> {
    tile_db(TileKind::Logic)
        .iter()
        .find(|e| matches!(&e.func, crate::db::DbFunc::Other(f) if f == "CarryInSet"))
        .and_then(|e| e.bits.iter().find(|b| b.val).map(|b| (b.row, b.bit)))
        .ok_or_else(|| "ice40: no CarryInSet DB entry".to_string())
}

/// Emit the `.asc` text for a routed design.
pub fn emit_asc(router: &Router, routed: &RoutedIce) -> Result<String, String> {
    let placed = &routed.placed;

    // Allocate all tiles.
    let mut tiles: HashMap<(u8, u8), TileBits> = HashMap::new();
    for y in 0..=33u8 {
        for x in 0..=33u8 {
            if let Some(k) = tile_kind(x, y) {
                tiles.insert((x, y), TileBits::new(k));
            }
        }
    }

    // 1. Routing bits from routed edges.
    for net in &routed.nets {
        for e in &net.edges {
            let bits = router.edge_bits(e.kind_idx, e.rel_idx);
            if let Some(t) = tiles.get_mut(&(e.tile_x, e.tile_y)) {
                for &(bank, bit) in bits {
                    t.set(bank, bit);
                }
            }
        }
    }

    // 2. LC config bits.
    // Map (x, y, lc) -> (lut init, has_ff).
    let mut lc_lut: HashMap<(u8, u8, u8), u16> = HashMap::new();
    let mut lc_ff: HashSet<(u8, u8, u8)> = HashSet::new();
    let mut lc_carry: HashSet<(u8, u8, u8)> = HashSet::new();
    for lut in &placed.luts {
        lc_lut.insert((lut.x, lut.y, lut.lc), lut.init);
        if lut.carry {
            lc_carry.insert((lut.x, lut.y, lut.lc));
        }
    }
    for ff in &placed.ffs {
        lc_ff.insert((ff.x, ff.y, ff.lc));
    }
    for (&(x, y, lc), &init) in &lc_lut {
        let mut bits = lut4_init_to_lc_bits(init);
        // Bit 9: DffEnable. Bit 8: CarryEnable.
        bits[9] = lc_ff.contains(&(x, y, lc));
        bits[8] = lc_carry.contains(&(x, y, lc));
        // Bits 18, 19: set/reset config (v1: synchronous reset, off).
        bits[18] = false;
        bits[19] = false;
        if let Some(t) = tiles.get_mut(&(x, y)) {
            // LC_i[j] lives at bank 2i bit (36+j) for j<10, bank 2i+1 bit
            // (36+j-10) for j>=10 (IceStorm logic-tile documentation table).
            for (j, &b) in bits.iter().enumerate().take(10) {
                if b {
                    t.set(2 * lc, 36 + j as u8);
                }
            }
            for (j, &b) in bits.iter().enumerate().skip(10) {
                if b {
                    t.set(2 * lc + 1, 36 + (j - 10) as u8);
                }
            }
        }
    }

    // 2b. Carry wiring bits.
    // in_3 of each carry LC selects the carry: carry_in_mux for LC0,
    // the previous LC's cout otherwise (dedicated carry needs no bits).
    for &(x, y, lc) in &lc_carry {
        let (src, dst) = if lc == 0 {
            ("carry_in_mux".to_string(), "lutff_0/in_3".to_string())
        } else {
            (
                format!("lutff_{}/cout", lc - 1),
                format!("lutff_{lc}/in_3"),
            )
        };
        let set_bits = buffer_set_bits(&src, &dst)?;
        if let Some(t) = tiles.get_mut(&(x, y)) {
            for &(bank, bit) in &set_bits {
                t.set(bank, bit);
            }
        }
    }
    // carry_in_mux source per tile.
    for mux in &placed.carry_mux {
        let set_bits = match mux.mode {
            CarryMuxMode::One => vec![carry_in_set_bit()?],
            CarryMuxMode::FromBelow => buffer_set_bits("carry_in", "carry_in_mux")?,
        };
        if let Some(t) = tiles.get_mut(&(mux.x, mux.y)) {
            for &(bank, bit) in &set_bits {
                t.set(bank, bit);
            }
        }
    }

    // 3. IO PINTYPE bits.
    // Group IOs by tile to get the kind.
    let mut io_by_tile: HashMap<(u8, u8), Vec<&crate::place::PlacedIo>> = HashMap::new();
    for io in &placed.ios {
        io_by_tile.entry((io.x, io.y)).or_default().push(io);
    }
    for ((x, y), ios) in &io_by_tile {
        let kind = tile_kind(*x, *y)
            .ok_or_else(|| format!("ice40: IO placed at ({x}, {y}) is not a known tile"))?;
        let pintype_bits = iob_pintype_bits(kind);
        if let Some(t) = tiles.get_mut(&(*x, *y)) {
            for io in ios {
                let pintype = match io.dir {
                    IoDir::In => PINTYPE_INPUT,
                    IoDir::Out => PINTYPE_OUTPUT,
                };
                for (i, ch) in pintype.chars().enumerate() {
                    if ch == '1' {
                        if let Some(&(bank, bit)) = pintype_bits.get(&(io.iob, i as u8)) {
                            t.set(bank, bit);
                        }
                    }
                }
            }
        }
    }

    // 4. Render .asc. Tile order: io, logic, ramb, ramt (matches arachne-pnr).
    let mut out = String::new();
    out.push_str(".device 8k\n");
    let mut io_tiles: Vec<(u8, u8)> = Vec::new();
    let mut logic_tiles: Vec<(u8, u8)> = Vec::new();
    let mut ramb_tiles: Vec<(u8, u8)> = Vec::new();
    let mut ramt_tiles: Vec<(u8, u8)> = Vec::new();
    for (&(x, y), t) in &tiles {
        match t.kind {
            TileKind::IoTop | TileKind::IoBottom | TileKind::IoLeft | TileKind::IoRight => {
                io_tiles.push((x, y))
            }
            TileKind::Logic => logic_tiles.push((x, y)),
            TileKind::Ramb => ramb_tiles.push((x, y)),
            TileKind::Ramt => ramt_tiles.push((x, y)),
        }
    }
    io_tiles.sort();
    logic_tiles.sort();
    ramb_tiles.sort();
    ramt_tiles.sort();
    let render = |out: &mut String, tiles: &HashMap<(u8, u8), TileBits>, list: &[(u8, u8)], name: &str| {
        for &(x, y) in list {
            out.push_str(&format!(".{name} {x} {y}\n"));
            if let Some(t) = tiles.get(&(x, y)) {
                for bank in &t.banks {
                    for &b in bank {
                        out.push(if b { '1' } else { '0' });
                    }
                    out.push('\n');
                }
            }
        }
    };
    render(&mut out, &tiles, &io_tiles, "io_tile");
    render(&mut out, &tiles, &logic_tiles, "logic_tile");
    render(&mut out, &tiles, &ramb_tiles, "ramb_tile");
    render(&mut out, &tiles, &ramt_tiles, "ramt_tile");
    Ok(out)
}
