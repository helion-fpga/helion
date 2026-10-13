//! Parser for the Project Trellis device databases (ECP5).
//!
//! Three JSON files describe the LFE5U-85F (`data/tilegrid.json`,
//! `data/iodb.json`, `data/globals.json`; vendored from the Project Trellis
//! database, ISC licensed), plus one `bits.db` per tile type under
//! `data/tiledata/` (see [`crate::tiledata`]).
//!
//! `bits.db` format: `# Routing Mux Bits` sections list `.mux <dst>` headers
//! followed by `<src> <bit>...` lines, where each bit is `F<frame>B<bit>` or
//! `!F<frame>B<bit>` (set / clear the bit to connect `src` to `dst`). The
//! `# Non-Routing Configuration` section lists
//! `.config_enum <site>.<param> <default>` headers followed by
//! `<value> <bit>...` lines, and `.fixed_conn <src> <dst>` lines name
//! hardwired connections.

use serde::Deserialize;
use std::collections::HashMap;

/// One tile entry in `tilegrid.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct TileEntry {
    /// Config frames spanned by this tile (bitstream emission needs these).
    pub cols: u32,
    /// Config rows spanned by this tile.
    pub rows: u32,
    /// Primitive sites hosted by this tile.
    pub sites: Vec<SiteEntry>,
    /// First config bit of this tile.
    pub start_bit: u32,
    /// First config frame of this tile.
    pub start_frame: u32,
    /// Trellis tile type, e.g. `"PLC2"`, `"CIB"`, `"MIB_EBR0"`.
    #[serde(rename = "type")]
    pub tile_type: String,
}

/// One primitive site in a tile entry.
#[derive(Clone, Debug, Deserialize)]
pub struct SiteEntry {
    /// Site name, e.g. `"SLICE_A"`, `"PIOA"`, `"EBR0"`, `"MULT18_0"`.
    pub name: String,
    /// Bel column (Trellis bel coordinates, as decimal text in the DB).
    pub pos_col: String,
    /// Bel row (Trellis bel coordinates, as decimal text in the DB).
    pub pos_row: String,
}

/// The parsed `tilegrid.json`: tile key -> entry.
///
/// Tile keys look like `"R11C100:PLC2"` or `"MIB_R10C105:MIB_DSP0"`: an
/// optional region prefix, the grid position, and the tile type. Several
/// tile entries can share one grid position (stacked CIB/MIB/TAP tiles).
#[derive(Clone, Debug, Deserialize)]
pub struct TileGrid(pub HashMap<String, TileEntry>);

/// One package pin in `iodb.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct PackagePin {
    /// Bel column of the PIO.
    pub col: u32,
    /// Which PIO in the tile: `"A"`, `"B"`, `"C"`, or `"D"`.
    pub pio: String,
    /// Bel row of the PIO.
    pub row: u32,
}

/// One row of `iodb.json` `pio_metadata`.
#[derive(Clone, Debug, Deserialize)]
pub struct PioMetadata {
    /// IO bank number.
    pub bank: u32,
    /// Bel column.
    pub col: u32,
    /// DQS group name, if any.
    pub dqs: Option<String>,
    /// Special function (e.g. `"ULC_GPLL0T_IN"`), if any.
    pub function: Option<String>,
    /// Which PIO in the tile: `"A"`, `"B"`, `"C"`, or `"D"`.
    pub pio: String,
    /// Bel row.
    pub row: u32,
}

/// The parsed `iodb.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct IoDb {
    /// Package name -> pin name -> pin. Only packages in the DB exist.
    pub packages: HashMap<String, HashMap<String, PackagePin>>,
    /// Per-PIO metadata (bank, DQS group, special function).
    pub pio_metadata: Vec<PioMetadata>,
}

/// A clock quadrant in `globals.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Quadrant {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// A clock tap column range in `globals.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Tap {
    pub lx0: u32,
    pub lx1: u32,
    pub rx0: u32,
    pub rx1: u32,
}

/// A clock spine position in `globals.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Spine {
    pub x: u32,
    pub y: u32,
}

/// The parsed `globals.json`: clock quadrants, tap columns, spine positions.
#[derive(Clone, Debug, Deserialize)]
pub struct Globals {
    /// `"UL"`/`"UR"`/`"LL"`/`"LR"` -> quadrant bounds (grid coordinates).
    pub quadrants: HashMap<String, Quadrant>,
    /// Tap name (e.g. `"C13"`) -> left/right column ranges.
    pub taps: HashMap<String, Tap>,
    /// Spine name (e.g. `"UL13"`) -> position.
    pub spines: HashMap<String, Spine>,
}

/// Parse `tilegrid.json` text. Fails loudly on malformed JSON.
pub fn parse_tilegrid(text: &str) -> Result<TileGrid, String> {
    serde_json::from_str(text).map_err(|e| format!("tilegrid.json parse error: {e}"))
}

/// Parse `iodb.json` text. Fails loudly on malformed JSON.
pub fn parse_iodb(text: &str) -> Result<IoDb, String> {
    serde_json::from_str(text).map_err(|e| format!("iodb.json parse error: {e}"))
}

/// Parse `globals.json` text. Fails loudly on malformed JSON.
pub fn parse_globals(text: &str) -> Result<Globals, String> {
    serde_json::from_str(text).map_err(|e| format!("globals.json parse error: {e}"))
}

/// A single configuration bit requirement: frame `frame`, bit `bit`, must be
/// `val` (set) or cleared (`!` prefix in the DB).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct BitReq {
    pub frame: u32,
    pub bit: u32,
    pub val: bool,
}

/// One routing mux input: setting `bits` connects `src` to the mux output.
#[derive(Clone, Debug)]
pub struct MuxInput {
    pub src: String,
    pub bits: Vec<BitReq>,
}

/// A routing mux: one output wire `dst`, selectable inputs.
#[derive(Clone, Debug)]
pub struct Mux {
    pub dst: String,
    pub inputs: Vec<MuxInput>,
}

/// One `.config_enum` option: choosing `value` for the site parameter sets
/// (and clears) `bits`.
#[derive(Clone, Debug)]
pub struct ConfigOption {
    pub value: String,
    pub bits: Vec<BitReq>,
}

/// A `.config_enum <site>.<param>` block with its default and options.
#[derive(Clone, Debug)]
pub struct ConfigEnum {
    /// Site-qualified parameter, e.g. `"SLICEA.MODE"`, `"PIOA.BASE_TYPE"`.
    pub name: String,
    /// Default option name from the header line.
    pub default: String,
    pub options: Vec<ConfigOption>,
}

/// The parsed `bits.db` for one tile type.
#[derive(Clone, Debug, Default)]
pub struct TileBits {
    /// Routing muxes (`.mux` sections).
    pub muxes: Vec<Mux>,
    /// Non-routing configuration enums (`.config_enum` sections).
    pub config_enums: Vec<ConfigEnum>,
    /// Hardwired connections (`.fixed_conn` lines).
    pub fixed_conns: Vec<(String, String)>,
}

/// Parse one `F<frame>B<bit>` / `!F<frame>B<bit>` token.
fn parse_bitreq(tok: &str) -> Option<BitReq> {
    let tok = tok.trim();
    let (tok, val) = match tok.strip_prefix('!') {
        Some(rest) => (rest, false),
        None => (tok, true),
    };
    let tok = tok.strip_prefix('F')?;
    let (frame, bit) = tok.split_once('B')?;
    Some(BitReq {
        frame: frame.parse().ok()?,
        bit: bit.parse().ok()?,
        val,
    })
}

/// Parse the bit list at the end of a mux input / config option line.
fn parse_bits(toks: &[&str]) -> Option<Vec<BitReq>> {
    toks.iter().map(|t| parse_bitreq(t)).collect()
}

/// Parse one tile type's `bits.db` text. Malformed lines are skipped; a file
/// that yields zero sections is reported by the caller, not here.
pub fn parse_bits_db(text: &str) -> TileBits {
    let mut out = TileBits::default();
    let mut cur_mux: Option<Mux> = None;
    let mut cur_enum: Option<ConfigEnum> = None;

    let flush_mux = |out: &mut TileBits, m: &mut Option<Mux>| {
        if let Some(m) = m.take() {
            out.muxes.push(m);
        }
    };
    let flush_enum = |out: &mut TileBits, e: &mut Option<ConfigEnum>| {
        if let Some(e) = e.take() {
            out.config_enums.push(e);
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(dst) = line.strip_prefix(".mux ") {
            flush_mux(&mut out, &mut cur_mux);
            flush_enum(&mut out, &mut cur_enum);
            cur_mux = Some(Mux {
                dst: dst.trim().to_string(),
                inputs: Vec::new(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix(".config_enum ") {
            flush_mux(&mut out, &mut cur_mux);
            flush_enum(&mut out, &mut cur_enum);
            // Header is `.config_enum <site>.<param> [<default>]`; the
            // default is absent in some Trellis files (e.g. PIOA.CLAMP).
            let mut p = rest.split_whitespace();
            if let Some(name) = p.next() {
                cur_enum = Some(ConfigEnum {
                    name: name.to_string(),
                    default: p.next().unwrap_or("").to_string(),
                    options: Vec::new(),
                });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(".fixed_conn ") {
            flush_mux(&mut out, &mut cur_mux);
            flush_enum(&mut out, &mut cur_enum);
            let mut p = rest.split_whitespace();
            if let (Some(src), Some(dst)) = (p.next(), p.next()) {
                out.fixed_conns.push((src.to_string(), dst.to_string()));
            }
            continue;
        }
        // Data line inside a .mux or .config_enum section.
        let mut toks = line.split_whitespace();
        let name = match toks.next() {
            Some(n) => n,
            None => continue,
        };
        let rest: Vec<&str> = toks.collect();
        if let Some(m) = cur_mux.as_mut() {
            if let Some(bits) = parse_bits(&rest) {
                m.inputs.push(MuxInput {
                    src: name.to_string(),
                    bits,
                });
            }
        } else if let Some(e) = cur_enum.as_mut() {
            if let Some(bits) = parse_bits(&rest) {
                e.options.push(ConfigOption {
                    value: name.to_string(),
                    bits,
                });
            }
        }
    }
    if let Some(m) = cur_mux.take() {
        out.muxes.push(m);
    }
    if let Some(e) = cur_enum.take() {
        out.config_enums.push(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bitreq_tokens() {
        assert_eq!(
            parse_bitreq("F1B2"),
            Some(BitReq {
                frame: 1,
                bit: 2,
                val: true
            })
        );
        assert_eq!(
            parse_bitreq("!F92B1"),
            Some(BitReq {
                frame: 92,
                bit: 1,
                val: false
            })
        );
        assert_eq!(parse_bitreq("B2"), None);
        assert_eq!(parse_bitreq("F1"), None);
    }

    #[test]
    fn parses_mux_section() {
        let t = parse_bits_db(".mux A0\nE1_H01E0001 F1B2 F6B3\nF5 F1B3 F4B2\n");
        assert_eq!(t.muxes.len(), 1);
        assert_eq!(t.muxes[0].dst, "A0");
        assert_eq!(t.muxes[0].inputs.len(), 2);
        assert_eq!(t.muxes[0].inputs[0].src, "E1_H01E0001");
        assert_eq!(t.muxes[0].inputs[0].bits.len(), 2);
        assert!(t.config_enums.is_empty());
    }

    #[test]
    fn parses_config_enum_section() {
        let t = parse_bits_db(".config_enum PIOA.CLAMP\nOFF !F9B0\nON F9B0\n");
        assert_eq!(t.config_enums.len(), 1);
        let e = &t.config_enums[0];
        assert_eq!(e.name, "PIOA.CLAMP");
        assert_eq!(e.options.len(), 2);
        assert_eq!(e.options[0].value, "OFF");
        assert!(!e.options[0].bits[0].val);
    }

    #[test]
    fn parses_fixed_conn() {
        let t = parse_bits_db(".fixed_conn DIA_SIOLOGIC JDIA\n");
        assert_eq!(t.fixed_conns, vec![("DIA_SIOLOGIC".to_string(), "JDIA".to_string())]);
    }

    #[test]
    fn all_vendored_json_parses() {
        let tg = parse_tilegrid(include_str!("../data/tilegrid.json")).unwrap();
        assert!(tg.0.len() > 14000);
        let io = parse_iodb(include_str!("../data/iodb.json")).unwrap();
        assert!(io.packages.contains_key("CABGA381"));
        let gl = parse_globals(include_str!("../data/globals.json")).unwrap();
        assert_eq!(gl.quadrants.len(), 4);
        assert_eq!(gl.taps.len(), 6);
    }

    #[test]
    fn all_vendored_bits_db_parse() {
        // Every vendored tile type must parse; dummy/special tiles
        // legitimately have empty files (see device.rs test for the list).
        for tt in crate::tiledata::TILE_TYPES {
            let text = crate::tiledata::bits_db(tt)
                .unwrap_or_else(|| panic!("no vendored bits.db for {tt}"));
            let _ = parse_bits_db(text);
        }
        // Spot-check: non-dummy tiles must have real content.
        for tt in ["PLC2", "CIB", "PIOT0", "MIB_EBR0", "MIB_DSP0", "PLL0_UL"] {
            let t = parse_bits_db(crate::tiledata::bits_db(tt).unwrap());
            let sections = t.muxes.len() + t.config_enums.len() + t.fixed_conns.len();
            assert!(sections > 0, "{tt} parsed to zero sections");
        }
    }

    #[test]
    fn plc2_muxes_have_expected_shape() {
        // Cross-check against the Trellis DB: the PFU tile drives each slice
        // input (A0..D7) and the span-wire muxes from named sources.
        let t = parse_bits_db(crate::tiledata::bits_db("PLC2").unwrap());
        assert!(t.muxes.iter().any(|m| m.dst == "A0"));
        assert!(t.muxes.iter().any(|m| m.dst == "D7"));
        assert!(t.muxes.iter().any(|m| m.dst.starts_with("E1_H")));
        assert!(t.config_enums.iter().any(|e| e.name == "SLICEA.MODE"));
    }
}
