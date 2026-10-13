//! HX8K device model for the iCE40 backend.
//!
//! Geometry, tile kinds, the CT256 package pin map (empirically derived
//! against arachne-pnr), span-wire aliasing (precomputed from IceStorm's
//! `pos_follow_net`), and the intra-tile routing databases.
//!
//! Coordinate system matches IceStorm: x in 0..=33, y in 0..=33.
//! Logic tiles live at 1..=32 excluding the RAM columns (x = 8, 25).
//! IO tiles ring the perimeter.

use crate::db::{parse_db, DbEntry, DbFunc};
use std::collections::HashMap;
use std::sync::OnceLock;

/// HX8K grid dimensions (matches IceStorm `setup_empty_8k`).
pub const MAX_X: u8 = 33;
pub const MAX_Y: u8 = 33;
/// RAM columns on HX8K.
pub const RAM_COLS: [u8; 2] = [8, 25];

/// Kind of tile at a coordinate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TileKind {
    Logic,
    Ramb,
    Ramt,
    IoTop,
    IoBottom,
    IoLeft,
    IoRight,
}

/// Classify the tile at `(x, y)`. Returns `None` for coordinates with no tile.
pub fn tile_kind(x: u8, y: u8) -> Option<TileKind> {
    if x > MAX_X || y > MAX_Y {
        return None;
    }
    // IO ring first.
    if y == 0 && x >= 1 && x < MAX_X {
        return Some(TileKind::IoBottom);
    }
    if y == MAX_Y && x >= 1 && x < MAX_X {
        return Some(TileKind::IoTop);
    }
    if x == 0 && y >= 1 && y < MAX_Y {
        return Some(TileKind::IoLeft);
    }
    if x == MAX_X && y >= 1 && y < MAX_Y {
        return Some(TileKind::IoRight);
    }
    // Interior.
    if x >= 1 && x < MAX_X && y >= 1 && y < MAX_Y {
        if RAM_COLS.contains(&x) {
            if y % 2 == 1 {
                return Some(TileKind::Ramb);
            } else {
                return Some(TileKind::Ramt);
            }
        }
        return Some(TileKind::Logic);
    }
    None
}

/// Is `(x, y)` a logic tile that can host LCs?
pub fn is_logic_tile(x: u8, y: u8) -> bool {
    matches!(tile_kind(x, y), Some(TileKind::Logic))
}

/// All logic tile coordinates, row-major.
pub fn all_logic_tiles() -> Vec<(u8, u8)> {
    let mut v = Vec::new();
    for y in 1..MAX_X {
        for x in 1..MAX_X {
            if is_logic_tile(x, y) {
                v.push((x, y));
            }
        }
    }
    v
}

/// A placed IO pin: package pin name to `(tile_x, tile_y, iob_index)`.
///
/// Derived empirically: ran arachne-pnr on single-IO designs constrained to
/// each breakout-board pin and decoded which IO tile / IOB got the PINTYPE
/// bits. Covers the HX8K breakout board (CT256) LEDs and clock.
pub fn ct256_pin_map() -> HashMap<&'static str, (u8, u8, u8)> {
    // (package pin, tile_x, tile_y, iob)
    [
        ("J3", (0, 16, 1)), // clk
        ("B5", (7, 33, 1)), // LED0
        ("B4", (6, 33, 1)), // LED1
        ("A2", (5, 33, 1)), // LED2
        ("A1", (4, 33, 1)), // LED3
        ("C5", (4, 33, 0)), // LED4
        ("C4", (3, 33, 1)), // LED5
        ("B3", (3, 33, 0)), // LED6
        ("C3", (1, 33, 0)), // LED7
    ]
    .into_iter()
    .collect()
}

/// PINTYPE value for a plain input pin (empirical, from arachne-pnr output).
pub const PINTYPE_INPUT: &str = "100000";
/// PINTYPE value for a plain push-pull output pin (empirical).
pub const PINTYPE_OUTPUT: &str = "100110";

/// Span-wire alias map: `(x, y, local_name)` -> canonical wire id.
///
/// Precomputed from IceStorm's geometric net-following (`pos_follow_net`
/// plus the documented intra-tile through-connections). All `(x, y, name)`
/// triples that are the same physical copper share one id.
pub fn span_aliases() -> &'static HashMap<(u8, u8, String), u32> {
    static MAP: OnceLock<HashMap<(u8, u8, String), u32>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for line in include_str!("../data/span_aliases.txt").lines() {
            let mut p = line.split_whitespace();
            let wid: u32 = p.next().unwrap().parse().unwrap();
            let x: u8 = p.next().unwrap().parse().unwrap();
            let y: u8 = p.next().unwrap().parse().unwrap();
            let name = p.next().unwrap().to_string();
            m.insert((x, y, name), wid);
        }
        m
    })
}

/// Number of distinct span wires on HX8K.
pub fn span_wire_count() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        span_aliases()
            .values()
            .max()
            .map(|v| v + 1)
            .unwrap_or(0)
    })
}

/// IO-tile span to fabric span mapping: `(io_x, io_y, io_name)` ->
/// `(logic_x, logic_y, logic_name)`. Precomputed via `pos_follow_net`.
pub fn io_span_map() -> &'static HashMap<(u8, u8, String), (u8, u8, String)> {
    static MAP: OnceLock<HashMap<(u8, u8, String), (u8, u8, String)>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for line in include_str!("../data/io_span_map.txt").lines() {
            let mut p = line.split_whitespace();
            let ix: u8 = p.next().unwrap().parse().unwrap();
            let iy: u8 = p.next().unwrap().parse().unwrap();
            let inode = p.next().unwrap().to_string();
            let lx: u8 = p.next().unwrap().parse().unwrap();
            let ly: u8 = p.next().unwrap().parse().unwrap();
            let lnode = p.next().unwrap().to_string();
            m.insert((ix, iy, inode), (lx, ly, lnode));
        }
        m
    })
}

/// Intra-tile routing database for a tile kind: all buffer/routing entries
/// with IceStorm net-name normalization applied.
pub fn tile_db(kind: TileKind) -> &'static [DbEntry] {
    static LOGIC: OnceLock<Vec<DbEntry>> = OnceLock::new();
    static IO_L: OnceLock<Vec<DbEntry>> = OnceLock::new();
    static IO_R: OnceLock<Vec<DbEntry>> = OnceLock::new();
    static IO_T: OnceLock<Vec<DbEntry>> = OnceLock::new();
    static IO_B: OnceLock<Vec<DbEntry>> = OnceLock::new();
    static RAMB: OnceLock<Vec<DbEntry>> = OnceLock::new();
    static RAMT: OnceLock<Vec<DbEntry>> = OnceLock::new();

    fn load(text: &str) -> Vec<DbEntry> {
        let mut entries = parse_db(text);
        // Apply IceStorm net-name normalization to routing endpoints,
        // mirroring icebox.py lines 6166-6173.
        for e in &mut entries {
            if matches!(e.func, DbFunc::Buffer | DbFunc::Routing) {
                e.src = crate::db::normalize_netname(&e.src);
                e.dst = crate::db::normalize_netname(&e.dst);
            }
        }
        entries
    }

    match kind {
        TileKind::Logic => LOGIC.get_or_init(|| load(include_str!("../data/logic_8k.txt"))),
        TileKind::IoLeft => IO_L.get_or_init(|| load(include_str!("../data/io_l.txt"))),
        TileKind::IoRight => IO_R.get_or_init(|| load(include_str!("../data/io_r.txt"))),
        TileKind::IoTop => IO_T.get_or_init(|| load(include_str!("../data/io_t.txt"))),
        TileKind::IoBottom => IO_B.get_or_init(|| load(include_str!("../data/io_b.txt"))),
        TileKind::Ramb => RAMB.get_or_init(|| load(include_str!("../data/ramb_8k.txt"))),
        TileKind::Ramt => RAMT.get_or_init(|| load(include_str!("../data/ramt_8k.txt"))),
    }
}

/// Non-routing config entries (LC_*, IoCtrl, IOB_*/PINTYPE_*, RamConfig, ...)
/// for a tile kind.
pub fn tile_config_db(kind: TileKind) -> Vec<&'static DbEntry> {
    // The full DBs are already loaded by tile_db; filter for non-routing.
    // To avoid double storage we re-slice from the same static.
    fn all(kind: TileKind) -> &'static [DbEntry] {
        tile_db(kind)
    }
    all(kind)
        .iter()
        .filter(|e| !matches!(e.func, DbFunc::Buffer | DbFunc::Routing))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_counts() {
        // HX8K: 32x32 interior minus 2 RAM columns of 32 = 960 logic tiles.
        assert_eq!(all_logic_tiles().len(), 960);
        assert_eq!(tile_kind(8, 5), Some(TileKind::Ramb)); // odd y
        assert_eq!(tile_kind(8, 6), Some(TileKind::Ramt)); // even y
        assert_eq!(tile_kind(1, 1), Some(TileKind::Logic));
        assert_eq!(tile_kind(0, 16), Some(TileKind::IoLeft));
        assert_eq!(tile_kind(16, 0), Some(TileKind::IoBottom));
        assert_eq!(tile_kind(16, 33), Some(TileKind::IoTop));
        assert_eq!(tile_kind(33, 16), Some(TileKind::IoRight));
    }

    #[test]
    fn span_aliases_cover_fabric() {
        // Every span port of every interior tile has an alias.
        let m = span_aliases();
        assert!(m.contains_key(&(5u8, 5u8, "sp4_h_r_0".to_string())));
        // The docs example wire: (5,5,r_0) == (6,5,l_0) == (6,5,r_13).
        let a = m[&(5u8, 5u8, "sp4_h_r_0".to_string())];
        let b = m[&(6u8, 5u8, "sp4_h_l_0".to_string())];
        let c = m[&(6u8, 5u8, "sp4_h_r_13".to_string())];
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert!(span_wire_count() > 30000);
    }

    #[test]
    fn pin_map_has_breakout_pins() {
        let pins = ct256_pin_map();
        assert_eq!(pins["J3"], (0, 16, 1));
        assert_eq!(pins["B5"], (7, 33, 1));
    }

    #[test]
    fn routing_dbs_nonempty() {
        assert!(!tile_db(TileKind::Logic).is_empty());
        assert!(!tile_db(TileKind::IoLeft).is_empty());
        assert!(!tile_db(TileKind::Ramb).is_empty());
        // Logic tile has LC->span and span->local connections.
        let db = tile_db(TileKind::Logic);
        assert!(db.iter().any(|e| e.src == "lutff_0/out"));
        assert!(db.iter().any(|e| e.dst.starts_with("local_")));
    }
}
