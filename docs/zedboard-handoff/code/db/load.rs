//! Typed JSON loaders for the Project X-Ray database files.
//!
//! Covers `tilegrid.json`, `tileconn.json`, `tile_type_*.json` and `part.json`.
//! All loaders return `Result<_, String>`; file IO errors and JSON errors are
//! mapped to human-readable strings.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Read a file to a string, mapping IO errors to `String`.
pub fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Parse JSON text into `T`, mapping errors to `String`.
pub fn parse_json<T: for<'de> Deserialize<'de>>(text: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|e| format!("cannot parse {what}: {e}"))
}

/// One configuration bus of a tile (from `tilegrid.json` `"bits"`).
#[derive(Clone, Debug, Deserialize)]
pub struct BusInfo {
    /// Hex string like `"0x00400000"`.
    pub baseaddr: String,
    pub frames: u32,
    pub offset: u32,
    pub words: u32,
}

impl BusInfo {
    /// Parse the hex base address.
    pub fn baseaddr_u32(&self) -> Result<u32, String> {
        let h = self
            .baseaddr
            .strip_prefix("0x")
            .or_else(|| self.baseaddr.strip_prefix("0X"))
            .ok_or_else(|| format!("bad baseaddr '{}'", self.baseaddr))?;
        u32::from_str_radix(h, 16).map_err(|_| format!("bad baseaddr '{}'", self.baseaddr))
    }
}

/// One tile entry of `tilegrid.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct TileGridEntry {
    pub grid_x: i32,
    pub grid_y: i32,
    /// Tile type name, e.g. `INT_L` (JSON field `"type"`).
    #[serde(rename = "type")]
    pub tile_type: String,
    /// Site instance name -> site type, e.g. `SLICE_X0Y0 -> SLICEL`.
    #[serde(default)]
    pub sites: HashMap<String, String>,
    /// Configuration bus name -> bus info.
    #[serde(default)]
    pub bits: HashMap<String, BusInfo>,
    #[serde(default)]
    pub pin_functions: HashMap<String, String>,
    #[serde(default)]
    pub prohibited_sites: Vec<String>,
}

/// Load `tilegrid.json`: tile name -> [`TileGridEntry`].
pub fn load_tilegrid(path: &Path) -> Result<HashMap<String, TileGridEntry>, String> {
    let text = read_text(path)?;
    parse_json(&text, &format!("tilegrid {}", path.display()))
}

/// One wire-stitching entry of `tileconn.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct TileConnEntry {
    /// `[dx, dy]`: neighbor offset relative to the `tile_types[0]` tile.
    pub grid_deltas: [i32; 2],
    /// `[type_a, type_b]`: `wire_pairs[i][0]` lives in a `type_a` tile,
    /// `wire_pairs[i][1]` in the neighboring `type_b` tile.
    pub tile_types: [String; 2],
    /// `[wire_in_a, wire_in_b]` pairs that are the same electrical node.
    pub wire_pairs: Vec<[String; 2]>,
}

/// Load `tileconn.json`.
pub fn load_tileconn(path: &Path) -> Result<Vec<TileConnEntry>, String> {
    let text = read_text(path)?;
    parse_json(&text, &format!("tileconn {}", path.display()))
}

/// One pip of a `tile_type_*.json` file.
#[derive(Clone, Debug, Deserialize)]
pub struct PipInfo {
    pub src_wire: String,
    pub dst_wire: String,
    /// `"1"`/`"0"` string in the DB; also seen as a JSON number.
    #[serde(default)]
    pub is_directional: serde_json::Value,
    #[serde(default)]
    pub is_pseudo: serde_json::Value,
    #[serde(default)]
    pub can_invert: serde_json::Value,
}

/// Interpret the DB's loose `"1"`/`1`/`"0"`/`0` booleans.
pub fn loose_bool(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_u64().is_some_and(|x| x != 0),
        serde_json::Value::String(s) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// One site pin of a site template.
#[derive(Clone, Debug, Deserialize)]
pub struct SitePinInfo {
    /// Tile wire this site pin connects to.
    pub wire: String,
}

/// One site template of a `tile_type_*.json` file.
#[derive(Clone, Debug, Deserialize)]
pub struct SiteTemplate {
    pub name: String,
    pub prefix: String,
    /// Site type, e.g. `SLICEL` (JSON field `"type"`).
    #[serde(rename = "type")]
    pub site_type: String,
    #[serde(default)]
    pub x_coord: i32,
    #[serde(default)]
    pub y_coord: i32,
    /// Some pins are null in the JSON (unconnected); skipped during load.
    #[serde(default)]
    pub site_pins: HashMap<String, Option<SitePinInfo>>,
}

/// Raw `tile_type_*.json` contents.
#[derive(Clone, Debug, Deserialize)]
pub struct TileTypeFile {
    pub tile_type: String,
    /// Pip name (`TILE.SRC->DST`) -> pip info.
    #[serde(default)]
    pub pips: HashMap<String, PipInfo>,
    #[serde(default)]
    pub sites: Vec<SiteTemplate>,
    /// Wire name -> null (JSON object used as a set).
    #[serde(default)]
    pub wires: HashMap<String, Option<serde_json::Value>>,
}

/// Load one `tile_type_*.json` file.
pub fn load_tile_type(path: &Path) -> Result<TileTypeFile, String> {
    let text = read_text(path)?;
    parse_json(&text, &format!("tile_type {}", path.display()))
}

/// `part.json` (per part, e.g. `artix7/xc7a35tcsg324-1/part.json`).
#[derive(Clone, Debug, Deserialize)]
pub struct PartInfo {
    pub idcode: u32,
    #[serde(default)]
    pub global_clock_regions: serde_json::Value,
    #[serde(default)]
    pub iobanks: HashMap<String, String>,
}

impl PartInfo {
    /// Total configuration frame count, summed over the clock regions.
    ///
    /// For `xc7a35tcsg324-1` this must be 5408.
    pub fn frame_count(&self) -> Result<u32, String> {
        let regions = self
            .global_clock_regions
            .as_object()
            .ok_or_else(|| "part.json: global_clock_regions is not an object".to_string())?;
        let mut total: u32 = 0;
        for (_rname, region) in regions {
            let rows = region
                .get("rows")
                .and_then(|r| r.as_object())
                .ok_or_else(|| "part.json: region without rows".to_string())?;
            for (_row, row) in rows {
                let buses = row
                    .get("configuration_buses")
                    .and_then(|b| b.as_object())
                    .ok_or_else(|| "part.json: row without configuration_buses".to_string())?;
                for (_bus, bus) in buses {
                    let cols = bus
                        .get("configuration_columns")
                        .and_then(|c| c.as_object())
                        .ok_or_else(|| "part.json: bus without configuration_columns".to_string())?;
                    for (_col, col) in cols {
                        let fc = col
                            .get("frame_count")
                            .and_then(|f| f.as_u64())
                            .ok_or_else(|| "part.json: column without frame_count".to_string())?;
                        total = total
                            .checked_add(fc as u32)
                            .ok_or_else(|| "part.json: frame count overflow".to_string())?;
                    }
                }
            }
        }
        Ok(total)
    }
}

/// Load `part.json`.
pub fn load_part(path: &Path) -> Result<PartInfo, String> {
    let text = read_text(path)?;
    parse_json(&text, &format!("part {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn db_root() -> PathBuf {
        crate::device::default_db_root()
    }

    #[test]
    fn businfo_baseaddr() {
        let b = BusInfo {
            baseaddr: "0x00400000".into(),
            frames: 28,
            offset: 0,
            words: 2,
        };
        assert_eq!(b.baseaddr_u32().unwrap(), 0x0040_0000);
        let bad = BusInfo { baseaddr: "xyz".into(), frames: 0, offset: 0, words: 0 };
        assert!(bad.baseaddr_u32().is_err());
    }

    #[test]
    fn tilegrid_loads() {
        let tg = load_tilegrid(&db_root().join("artix7/xc7a50t/tilegrid.json")).unwrap();
        assert_eq!(tg.len(), 18055);
        let t = &tg["INT_L_X0Y0"];
        assert_eq!(t.tile_type, "INT_L");
        // NOTE: grid_x/grid_y are frame-grid coordinates, not the X0Y0 of the
        // tile name.
        assert_eq!((t.grid_x, t.grid_y), (4, 155));
        let bus = &t.bits["CLB_IO_CLK"];
        assert_eq!(bus.frames, 28);
        assert_eq!(bus.baseaddr_u32().unwrap(), 0x0040_0000);
    }

    #[test]
    fn tileconn_loads() {
        let tc = load_tileconn(&db_root().join("artix7/xc7a50t/tileconn.json")).unwrap();
        assert_eq!(tc.len(), 748);
        assert_eq!(tc[0].tile_types, ["BRAM_INT_INTERFACE_L".to_string(), "BRAM_L".to_string()]);
        assert_eq!(tc[0].grid_deltas, [-1, 0]);
        assert!(!tc[0].wire_pairs.is_empty());
    }

    #[test]
    fn tile_type_loads() {
        let tt = load_tile_type(&db_root().join("artix7/tile_type_INT_L.json")).unwrap();
        assert_eq!(tt.tile_type, "INT_L");
        assert_eq!(tt.pips.len(), 3737);
        assert_eq!(tt.wires.len(), 600);
        let p = &tt.pips["INT_L.BYP_ALT0->>BYP_BOUNCE0"];
        assert_eq!(p.src_wire, "BYP_ALT0");
        assert_eq!(p.dst_wire, "BYP_BOUNCE0");
        assert!(loose_bool(&p.is_directional));
        assert!(!loose_bool(&p.is_pseudo));

        let clb = load_tile_type(&db_root().join("artix7/tile_type_CLBLL_L.json")).unwrap();
        assert_eq!(clb.sites.len(), 2);
        assert_eq!(clb.sites[0].site_type, "SLICEL");
        assert!(clb.sites[0].site_pins.contains_key("A"));
    }

    #[test]
    fn part_loads() {
        let p = load_part(&db_root().join("artix7/xc7a35tcsg324-1/part.json")).unwrap();
        assert_eq!(p.idcode, 0x0362_D093);
        assert_eq!(p.frame_count().unwrap(), 5408);
        assert_eq!(p.iobanks["35"], "X113Y78");
    }

    #[test]
    fn loose_bool_forms() {
        assert!(loose_bool(&serde_json::json!("1")));
        assert!(loose_bool(&serde_json::json!(1)));
        assert!(!loose_bool(&serde_json::json!("0")));
        assert!(!loose_bool(&serde_json::json!(0)));
        assert!(!loose_bool(&serde_json::json!(null)));
    }
}
