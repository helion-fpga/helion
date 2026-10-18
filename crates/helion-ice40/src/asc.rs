//! ASC emission: [`RoutedIce`] → IceStorm `.asc` text bitstream.
//!
//! Sets routing bits from the routed edges, LC config bits (LUT truth
//! tables, DffEnable) from placement, and IO PINTYPE bits from the pin
//! directions. Global clocks program the per-tile
//! `glb_netwk -> lutff_global/clk` buffers, the `ColBufCtrl` column-buffer
//! bits, and the `padin_glb_netwk` extra bits. The `.asc` is converted to
//! binary with IceStorm's `icepack`.

use crate::device::{tile_db, tile_kind, TileKind, PINTYPE_INPUT, PINTYPE_OUTPUT};
use crate::gclk::colbuf_src_y;
use crate::pack::{lut4_init_to_lc_bits, HardIpKind, IoDir, OscKind, PackedPll};
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

/// Config bits selecting `glb_netwk_N` as a logic tile's clock
/// (`glb_netwk_N -> lutff_global/clk`).
fn glb_clk_buffer_bits(glb: u8) -> Result<Vec<(u8, u8)>, String> {
    let src = format!("glb_netwk_{glb}");
    let entry = tile_db(TileKind::Logic).iter().find(|e| {
        matches!(e.func, crate::db::DbFunc::Buffer)
            && e.src == src
            && e.dst == "lutff_global/clk"
    });
    match entry {
        Some(e) => Ok(e
            .bits
            .iter()
            .filter(|b| b.val)
            .map(|b| (b.row, b.bit))
            .collect()),
        None => Err(format!(
            "ice40: no buffer DB entry for glb_netwk_{glb} -> lutff_global/clk"
        )),
    }
}

/// Config bits selecting `glb_netwk_N` as a RAM tile's clock
/// (`glb_netwk_N -> ram/RCLK` in the RAMB tile, `-> ram/WCLK` in RAMT).
fn glb_ram_clk_buffer_bits(kind: TileKind, glb: u8, dst: &str) -> Result<Vec<(u8, u8)>, String> {
    let src = format!("glb_netwk_{glb}");
    let entry = tile_db(kind).iter().find(|e| {
        matches!(e.func, crate::db::DbFunc::Buffer) && e.src == src && e.dst == dst
    });
    match entry {
        Some(e) => Ok(e
            .bits
            .iter()
            .filter(|b| b.val)
            .map(|b| (b.row, b.bit))
            .collect()),
        None => Err(format!("ice40: no buffer DB entry for glb_netwk_{glb} -> {dst}")),
    }
}

/// The `ColBufCtrl` bit enabling `glb_netwk_N` in a column-buffer tile of
/// the given kind (bit positions differ per tile kind; read from the DB).
fn colbufctrl_bit(kind: TileKind, glb: u8) -> Result<(u8, u8), String> {
    let src = format!("glb_netwk_{glb}");
    tile_db(kind)
        .iter()
        .find(|e| {
            matches!(&e.func, crate::db::DbFunc::Other(f) if f == "ColBufCtrl")
                && e.src == src
        })
        .and_then(|e| e.bits.iter().find(|b| b.val).map(|b| (b.row, b.bit)))
        .ok_or_else(|| format!("ice40: no ColBufCtrl glb_netwk_{glb} DB entry"))
}

/// The `CarryInSet` bit (B1[50]).
fn carry_in_set_bit() -> Result<(u8, u8), String> {
    tile_db(TileKind::Logic)
        .iter()
        .find(|e| matches!(&e.func, crate::db::DbFunc::Other(f) if f == "CarryInSet"))
        .and_then(|e| e.bits.iter().find(|b| b.val).map(|b| (b.row, b.bit)))
        .ok_or_else(|| "ice40: no CarryInSet DB entry".to_string())
}

/// `(bank, bit)` for a `RamConfig <name>` entry (`PowerUp`, `CBIT_0`..)
/// in a RAM tile DB.
fn ram_config_bit(kind: TileKind, name: &str) -> Result<(u8, u8), String> {
    tile_db(kind)
        .iter()
        .find(|e| {
            matches!(&e.func, crate::db::DbFunc::Other(f) if f == "RamConfig") && e.src == name
        })
        .and_then(|e| e.bits.iter().find(|b| b.val).map(|b| (b.row, b.bit)))
        .ok_or_else(|| format!("ice40: no RamConfig {name} DB entry"))
}

/// `(bank, bit)` for a `PLL PLLCONFIG_<n>` entry in an IO tile DB.
fn pll_config_bit(kind: TileKind, n: u8) -> Result<(u8, u8), String> {
    let want = format!("PLLCONFIG_{n}");
    tile_db(kind)
        .iter()
        .find(|e| {
            matches!(&e.func, crate::db::DbFunc::Other(f) if f == "PLL") && e.src == want
        })
        .and_then(|e| e.bits.iter().find(|b| b.val).map(|b| (b.row, b.bit)))
        .ok_or_else(|| format!("ice40: no PLL {want} DB entry"))
}

/// SB_PLL40 config bits as `(tile_x, PLLCONFIG_n, value)`.
///
/// Bit placement is IceStorm `icebox.py` `pllinfo_db` `"8k_0"` (bottom PLL
/// site, y=0) / `"8k_1"` (top PLL site, y=33): the y coordinate is supplied
/// by the caller, the x coordinates and PLLCONFIG numbers are fixed by the
/// table. Field values follow the iCE40 PLL usage:
/// PLLTYPE 3'b010 = SB_PLL40_PAD, 3'b011 = SB_PLL40_CORE;
/// FEEDBACK_PATH 3'b001 = SIMPLE; DIVR 4 bits; DIVF 7 bits; DIVQ 3 bits;
/// FILTER_RANGE 3 bits.
fn pll_config_bits(pll: &PackedPll) -> Vec<(u8, u8, bool)> {
    let mut bits: Vec<(u8, u8, bool)> = Vec::new();
    let mut push = |x: u8, cfg: u8, val: bool| bits.push((x, cfg, val));
    // PLLTYPE_0..2.
    let plltype: u8 = pll.kind.plltype_code();
    push(16, 5, plltype & 1 != 0);
    push(18, 1, plltype & 2 != 0);
    push(18, 3, plltype & 4 != 0);
    // FEEDBACK_PATH_0..2.
    let fb: u8 = pll.feedback_path.code();
    push(18, 5, fb & 1 != 0);
    push(15, 9, fb & 2 != 0);
    push(16, 1, fb & 4 != 0);
    for i in 0..4 {
        push(14, 1 + i, (pll.divr >> i) & 1 != 0);
    }
    // DIVF_0..4, DIVF_5..6.
    for i in 0..5 {
        push(14, 5 + i, (pll.divf >> i) & 1 != 0);
    }
    for i in 0..2 {
        push(15, 1 + i, (pll.divf >> (5 + i)) & 1 != 0);
    }
    // DIVQ_0..2.
    for i in 0..3 {
        push(15, 3 + i, (pll.divq >> i) & 1 != 0);
    }
    // FILTER_RANGE_0..2.
    for i in 0..3 {
        push(15, 6 + i, (pll.filter_range >> i) & 1 != 0);
    }
    bits
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

    // 2b2. Global clock network.
    // Each global clock programs, in every logic tile with FFs on it, the
    // `glb_netwk_N -> lutff_global/clk` buffer. The `ColBufCtrl` bit for
    // `glb_netwk_N` is set in the column-buffer tile covering each consumer
    // tile (IceStorm colbuf_db "8k": buffer rows 8, 9, 24, 25). Tiles where
    // a routed edge taps the global net (`glb_netwk_N -> glb2local_*` for a
    // non-clock load) need their column buffer as well.
    let mut colbuf_need: HashSet<(u8, u8, u8)> = HashSet::new(); // (x, y, glb)
    for gc in &routed.glb_clocks {
        let clk_bits = glb_clk_buffer_bits(gc.glb)?;
        for &(x, y) in &gc.tiles {
            if let Some(t) = tiles.get_mut(&(x, y)) {
                for &(bank, bit) in &clk_bits {
                    t.set(bank, bit);
                }
            }
            colbuf_need.insert((x, y, gc.glb));
        }
        for &(x, y, is_wclk) in &gc.ram_clks {
            let (kind, dst) = if is_wclk {
                (TileKind::Ramt, "ram/WCLK")
            } else {
                (TileKind::Ramb, "ram/RCLK")
            };
            let bits = glb_ram_clk_buffer_bits(kind, gc.glb, dst)?;
            if let Some(t) = tiles.get_mut(&(x, y)) {
                for &(bank, bit) in &bits {
                    t.set(bank, bit);
                }
            }
            colbuf_need.insert((x, y, gc.glb));
        }
    }
    for net in &routed.nets {
        for e in &net.edges {
            let src = router.rel_src(e.kind_idx, e.rel_idx);
            if let Some(glb) = src
                .strip_prefix("glb_netwk_")
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|&g| g < 8)
            {
                colbuf_need.insert((e.tile_x, e.tile_y, glb));
            }
        }
    }
    for (x, y, glb) in colbuf_need {
        let by = colbuf_src_y(y);
        let kind = tile_kind(x, by)
            .ok_or_else(|| format!("ice40: no tile at column-buffer ({x}, {by})"))?;
        let (bank, bit) = colbufctrl_bit(kind, glb)?;
        if let Some(t) = tiles.get_mut(&(x, by)) {
            t.set(bank, bit);
        }
    }

    // 2c. RAM config bits.
    // PowerUp lives in the RAMB tile; the CBITs in the RAMT tile
    // (WRITE_MODE[0]->CBIT_0, WRITE_MODE[1]->CBIT_1, READ_MODE[0]->CBIT_2,
    // READ_MODE[1]->CBIT_3 per the IceStorm RAM tile documentation).
    // Bit positions come from the tile DBs, not hardcoded.
    let mut ram_inits: Vec<((u8, u8), [u16; 256])> = Vec::new();
    for (ri, ram) in placed.packed.rams.iter().enumerate() {
        let pr = placed.rams.get(ri).ok_or_else(|| {
            format!("ice40: placed RAM {} missing for packed RAM {}", ri, ram.name)
        })?;
        let (bank, bit) = ram_config_bit(TileKind::Ramb, "PowerUp")?;
        if let Some(t) = tiles.get_mut(&(pr.x, pr.y)) {
            t.set(bank, bit);
            if ram.neg_clk_r {
                // B0[0] NegClk in RAMB tile
                t.set(0, 0);
            }
        }
        if ram.neg_clk_w {
            if let Some(t) = tiles.get_mut(&(pr.x, pr.y + 1)) {
                // B0[0] NegClk in RAMT tile
                t.set(0, 0);
            }
        }
        let modes = [
            (ram.write_mode, 0u8, 0u8),
            (ram.write_mode, 1, 1),
            (ram.read_mode, 0, 2),
            (ram.read_mode, 1, 3),
        ];
        for (mode, bit_idx, cbit) in modes {
            if (mode >> bit_idx) & 1 == 1 {
                let (bank, bit) =
                    ram_config_bit(TileKind::Ramt, &format!("CBIT_{cbit}"))?;
                if let Some(t) = tiles.get_mut(&(pr.x, pr.y + 1)) {
                    t.set(bank, bit);
                }
            }
        }
        ram_inits.push(((pr.x, pr.y), ram.init));
    }

    // 2d. PLL config bits.
    // The site row comes from the placed position (bottom y=0 / top y=33);
    // the bit layout is IceStorm icebox.py pllinfo_db "8k_0"/"8k_1".
    for pll in &placed.packed.plls {
        let (yb, io_kind) = if pll.kind.is_pad() {
            (0u8, TileKind::IoBottom)
        } else {
            (33u8, TileKind::IoTop)
        };
        for (x, cfg, val) in pll_config_bits(pll) {
            if !val {
                continue;
            }
            let (bank, bit) = pll_config_bit(io_kind, cfg)?;
            if let Some(t) = tiles.get_mut(&(x, yb)) {
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

    // 5. RAM init data, after the tiles (as arachne-pnr emits it).
    // `.ram_data <x> <y>` uses the RAMB tile coordinates; 16 lines of 64
    // hex chars hold flat bits 0..4095, line L holding bits L*256..L*256+255
    // with the leftmost char = the highest bit (verified against
    // arachne-pnr output and icepack's bit parser).
    for ((x, y), init) in &ram_inits {
        out.push_str(&format!(".ram_data {x} {y}\n"));
        let bit = |b: usize| (init[b / 16] >> (b % 16)) & 1;
        for line in 0..16usize {
            for ci in 0..64usize {
                let hi = line * 256 + 255 - 4 * ci;
                let d = (bit(hi) << 3) | (bit(hi - 1) << 2) | (bit(hi - 2) << 1) | bit(hi - 3);
                out.push(b"0123456789abcdef"[d as usize] as char);
            }
            out.push('\n');
        }
    }

    // 6. padin_glb_netwk extra bits (PLL-driven global clocks), after the
    // ram_data sections (matches IceStorm icebox.py write order).
    let mut extra_bits: Vec<(u8, u16, u16)> = routed
        .glb_clocks
        .iter()
        .filter_map(|gc| gc.extra_bit)
        .collect();
    extra_bits.sort();
    extra_bits.dedup();
    for (bank, row, bit) in extra_bits {
        out.push_str(&format!(".extra_bit {bank} {row} {bit}\n"));
    }

    // 7. Hard IP / DSP / SPRAM / Oscillator configuration blocks (.comment metadata
    // preserved alongside tile bitstream for SB_MAC16, SB_SPRAM256KA, SB_HFOSC/SB_LFOSC,
    // SB_I2C, SB_SPI, SB_RGBA_DRV, SB_LEDDA_IP).
    for (mi, mac) in placed.packed.mac16s.iter().enumerate() {
        let pmac = &placed.mac16s[mi];
        out.push_str(&format!(
            ".comment SB_MAC16 {} site=({},{}) IpConfig=0x{:07x} NegTrigger={} C_REG={} A_REG={} B_REG={} D_REG={} MODE_8x8={} A_SIGNED={} B_SIGNED={}\n",
            mac.name,
            pmac.x,
            pmac.y,
            mac.config_bits_25,
            u8::from(mac.neg_trigger),
            u8::from(mac.c_reg),
            u8::from(mac.a_reg),
            u8::from(mac.b_reg),
            u8::from(mac.d_reg),
            u8::from(mac.mode_8x8),
            u8::from(mac.a_signed),
            u8::from(mac.b_signed),
        ));
    }
    for (si, spram) in placed.packed.sprams.iter().enumerate() {
        let pspram = &placed.sprams[si];
        out.push_str(&format!(
            ".comment SB_SPRAM256KA {} site=({},{})\n",
            spram.name, pspram.x, pspram.y
        ));
    }
    for (oi, osc) in placed.packed.oscs.iter().enumerate() {
        let posc = &placed.oscs[oi];
        let kind_str = match osc.kind {
            OscKind::HfOsc => "SB_HFOSC",
            OscKind::LfOsc => "SB_LFOSC",
        };
        out.push_str(&format!(
            ".comment {} {} site=({},{}) CLKHF_DIV={}\n",
            kind_str, osc.name, posc.x, posc.y, osc.clk_hf_div
        ));
    }
    for (hi, ip) in placed.packed.hard_ips.iter().enumerate() {
        let pip = &placed.hard_ips[hi];
        match &ip.kind {
            HardIpKind::I2c { bus_addr74 } => {
                out.push_str(&format!(
                    ".comment SB_I2C {} site=({},{}) BUS_ADDR74=0b{:04b}\n",
                    ip.name, pip.x, pip.y, bus_addr74
                ));
            }
            HardIpKind::Spi { bus_addr74 } => {
                out.push_str(&format!(
                    ".comment SB_SPI {} site=({},{}) BUS_ADDR74=0b{:04b}\n",
                    ip.name, pip.x, pip.y, bus_addr74
                ));
            }
            HardIpKind::RgbaDrv {
                current_mode,
                rgb0_current,
                rgb1_current,
                rgb2_current,
            } => {
                out.push_str(&format!(
                    ".comment SB_RGBA_DRV {} site=({},{}) CURRENT_MODE={} RGB0_CURRENT=0b{:06b} RGB1_CURRENT=0b{:06b} RGB2_CURRENT=0b{:06b}\n",
                    ip.name,
                    pip.x,
                    pip.y,
                    current_mode,
                    rgb0_current,
                    rgb1_current,
                    rgb2_current
                ));
            }
            HardIpKind::LeddaIp => {
                out.push_str(&format!(
                    ".comment SB_LEDDA_IP {} site=({},{})\n",
                    ip.name, pip.x, pip.y
                ));
            }
        }
    }
    Ok(out)
}
