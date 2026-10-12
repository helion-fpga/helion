//! In-memory device model for the Xilinx 7-series backend (`xc7a35t`).
//!
//! [`Device`] is built from the Project X-Ray database (open data only) by
//! [`Device::load_xc7a35t`]. Tiles are stored flat and indexed by name and by
//! `(grid_x, grid_y)`; per-tile-type data (wires, pips, site templates,
//! segbits) is shared across all tiles of that type. Electrical nodes spanning
//! tiles are built from `tileconn.json` stitching with a union-find.
//!
//! Frame addressing: for a tile configuration bus with `baseaddr`, `offset`
//! and a segbit `(frame_offset, bitpos, value)`:
//! `frame = baseaddr + frame_offset`, `word = offset + bitpos / 32`,
//! `bit = bitpos % 32`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::db::load::{
    BusInfo, PartInfo, TileConnEntry, TileGridEntry, TileTypeFile, load_part, load_tile_type,
    load_tileconn, load_tilegrid, loose_bool, read_text,
};
use crate::db::parse::{PackagePin, Ppip, SegBit, parse_mask_line, parse_package_pins, parse_ppips_line, parse_segbits_line};

/// Root of the raw X-Ray database.
///
/// `HELION_XILINX_DB` wins; otherwise `~/workspace/xilinx-dbsrc`.
pub fn default_db_root() -> PathBuf {
    if let Ok(p) = std::env::var("HELION_XILINX_DB") {
        return PathBuf::from(p);
    }
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join("workspace/xilinx-dbsrc"),
        Err(_) => PathBuf::from("xilinx-dbsrc"),
    }
}

/// Pip classification used by the router and bitstream emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipClass {
    /// General interconnect pip (all pips in `INT_*` tiles).
    TileRouting,
    /// Routing wire -> site input pin.
    SiteEntry,
    /// Site output pin -> routing wire.
    SiteExit,
    /// Site pin -> site pin inside one site (e.g. CLB input muxes).
    SiteInternal,
}

/// One routing pip within a tile type.
#[derive(Clone, Debug)]
pub struct Pip {
    pub src: u32,
    pub dst: u32,
    pub class: PipClass,
    pub is_pseudo: bool,
    pub can_invert: bool,
}

/// One site pin: (pin name, tile wire).
pub type SitePin = (String, String);

#[derive(Clone, Debug)]
struct SiteTemplateData {
    name: String,
    prefix: String,
    site_type: String,
    pins: Vec<SitePin>,
}

/// Shared data for one tile type.
#[derive(Clone, Debug, Default)]
struct TileTypeData {
    name: String,
    wires: Vec<String>,
    wire_idx: HashMap<String, u32>,
    pips: Vec<Pip>,
    /// src wire idx -> pip indices with that source.
    pips_from: HashMap<u32, Vec<u32>>,
    site_templates: Vec<SiteTemplateData>,
    segbits: HashMap<String, Vec<SegBit>>,
    mask: Vec<(u32, u32)>,
    ppips: Vec<Ppip>,
}

/// One site instance on a tile.
#[derive(Clone, Debug)]
pub struct SiteInst {
    pub name: String,
    pub site_type: String,
    /// Index into the tile type's site templates, if matched.
    template: Option<u32>,
}

/// One configuration bus of a tile instance.
#[derive(Clone, Debug)]
pub struct BusData {
    pub name: String,
    pub baseaddr: u32,
    pub frames: u32,
    pub offset: u32,
    pub words: u32,
}

/// One physical tile.
#[derive(Clone, Debug)]
pub struct Tile {
    pub name: String,
    pub type_id: u32,
    pub grid_x: i32,
    pub grid_y: i32,
    pub sites: Vec<SiteInst>,
    pub buses: Vec<BusData>,
    /// First global wire id belonging to this tile.
    wire_base: u32,
}

/// Union-find over global wire ids (tile, wire) pairs.
#[derive(Clone, Debug, Default)]
struct UnionFind {
    parent: Vec<u32>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Result<Self, String> {
        if n > u32::MAX as usize {
            return Err("device too large for union-find".to_string());
        }
        Ok(Self { parent: (0..n as u32).collect(), rank: vec![0; n] })
    }

    fn find(&mut self, mut x: u32) -> u32 {
        let mut root = x;
        while self.parent[root as usize] != root {
            root = self.parent[root as usize];
        }
        while self.parent[x as usize] != root {
            let next = self.parent[x as usize];
            self.parent[x as usize] = root;
            x = next;
        }
        root
    }

    fn union(&mut self, a: u32, b: u32) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        let (ra, rb) = (ra as usize, rb as usize);
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb as u32,
            std::cmp::Ordering::Greater => self.parent[rb] = ra as u32,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra as u32;
                self.rank[ra] = self.rank[ra].saturating_add(1);
            }
        }
    }
}

/// In-memory model of the `xc7a35t` device.
#[derive(Clone, Debug)]
pub struct Device {
    tiles: Vec<Tile>,
    tile_by_name: HashMap<String, usize>,
    grid: HashMap<(i32, i32), usize>,
    type_ids: HashMap<String, u32>,
    types: Vec<TileTypeData>,
    tiles_by_type: Vec<Vec<usize>>,
    package_pins: HashMap<String, PackagePin>,
    part: PartInfo,
    nodes: UnionFind,
    /// Tile types present in the grid with no DB files (e.g. `NULL`, `VBRK`).
    pub missing_db_types: Vec<String>,
}

impl Device {
    /// Load the full `xc7a35t` device model from the raw X-Ray DB.
    ///
    /// `db_root` is the directory holding `artix7/` (see [`default_db_root`]).
    /// This parses `tilegrid.json`, `tileconn.json`, `part.json`,
    /// `package_pins.csv` and every `tile_type_*.json` / `segbits_*.db` /
    /// `mask_*.db` / `ppips_*.db` present, then stitches cross-tile nodes.
    pub fn load_xc7a35t(db_root: &Path) -> Result<Self, String> {
        let artix7 = db_root.join("artix7");
        let fabric = artix7.join("xc7a50t");
        let partdir = artix7.join("xc7a35tcsg324-1");

        let grid = load_tilegrid(&fabric.join("tilegrid.json"))?;
        let conn = load_tileconn(&fabric.join("tileconn.json"))?;
        let part = load_part(&partdir.join("part.json"))?;
        let pins_text = read_text(&partdir.join("package_pins.csv"))?;
        let pins = parse_package_pins(&pins_text)?;

        let mut dev = Self {
            tiles: Vec::new(),
            tile_by_name: HashMap::new(),
            grid: HashMap::new(),
            type_ids: HashMap::new(),
            types: Vec::new(),
            tiles_by_type: Vec::new(),
            package_pins: HashMap::new(),
            part,
            nodes: UnionFind::default(),
            missing_db_types: Vec::new(),
        };

        for p in pins {
            if dev.package_pins.insert(p.pin.clone(), p).is_some() {
                return Err("package_pins.csv: duplicate pin".to_string());
            }
        }

        // Register every tile type found in the grid first so type ids are
        // dense and stable.
        let mut type_names: Vec<String> =
            grid.values().map(|e| e.tile_type.clone()).collect();
        type_names.sort();
        type_names.dedup();
        for name in &type_names {
            let id = dev.types.len() as u32;
            dev.type_ids.insert(name.clone(), id);
            dev.types.push(TileTypeData { name: name.clone(), ..Default::default() });
            dev.tiles_by_type.push(Vec::new());
        }

        // Load per-type DB files.
        for (id, name) in type_names.iter().enumerate() {
            Self::load_tile_type_db(&artix7, name, &mut dev.types[id], &mut dev.missing_db_types)?;
        }

        // Instantiate tiles.
        let mut names: Vec<&String> = grid.keys().collect();
        names.sort();
        let mut wire_total: u64 = 0;
        for tname in names {
            let e: &TileGridEntry = &grid[tname];
            let type_id = *dev
                .type_ids
                .get(&e.tile_type)
                .ok_or_else(|| format!("tile {} has unregistered type", tname))?;
            let wire_base = u32::try_from(wire_total)
                .map_err(|_| "device too large: wire id overflow".to_string())?;
            let nwires = dev.types[type_id as usize].wires.len() as u64;
            wire_total = wire_total
                .checked_add(nwires)
                .ok_or_else(|| "device too large: wire count overflow".to_string())?;

            let sites = Self::instantiate_sites(&dev.types[type_id as usize], &e.sites)?;
            let mut buses = Vec::new();
            for (bname, b) in &e.bits {
                buses.push(BusData {
                    name: bname.clone(),
                    baseaddr: b.baseaddr_u32()?,
                    frames: b.frames,
                    offset: b.offset,
                    words: b.words,
                });
            }
            let idx = dev.tiles.len();
            dev.grid.insert((e.grid_x, e.grid_y), idx);
            dev.tile_by_name.insert(tname.clone(), idx);
            dev.tiles_by_type[type_id as usize].push(idx);
            dev.tiles.push(Tile {
                name: tname.clone(),
                type_id,
                grid_x: e.grid_x,
                grid_y: e.grid_y,
                sites,
                buses,
                wire_base,
            });
        }

        dev.nodes = UnionFind::new(
            usize::try_from(wire_total).map_err(|_| "wire count overflow".to_string())?,
        )?;
        dev.stitch_nodes(&conn)?;

        Ok(dev)
    }

    /// Match tilegrid site instances against the tile type's site templates.
    fn instantiate_sites(
        tdata: &TileTypeData,
        sites: &HashMap<String, String>,
    ) -> Result<Vec<SiteInst>, String> {
        let mut names: Vec<&String> = sites.keys().collect();
        names.sort();
        // Group template indices by site_type, preserving template order.
        // Template types may be more specific than tilegrid types
        // (e.g. `IOB33S`/`IOB33M` vs `IOB33`); match by prefix.
        let mut tmpl_by_type: HashMap<String, Vec<u32>> = HashMap::new();
        for (i, t) in tdata.site_templates.iter().enumerate() {
            tmpl_by_type
                .entry(t.site_type.clone())
                .or_default()
                .push(i as u32);
        }
        // Positional index of each site among sites of the same type.
        let mut type_counts: HashMap<&str, usize> = HashMap::new();
        let mut out = Vec::with_capacity(names.len());
        for sname in names {
            let stype = &sites[sname];
            let mut template = Self::match_template(&tdata.site_templates, sname, stype);
            // Positional fallback: Nth site of this type -> Nth template whose
            // type matches (by prefix). Covers IOB pairs and BUFGCTRL banks
            // where names carry die-level coordinates.
            if template.is_none() {
                let mut candidates: Vec<u32> = Vec::new();
                for (ttype, idxs) in &tmpl_by_type {
                    if ttype == stype || ttype.starts_with(stype.as_str()) {
                        candidates.extend(idxs);
                    }
                }
                candidates.sort();
                let pos = type_counts.get(stype.as_str()).copied().unwrap_or(0);
                if pos < candidates.len() {
                    template = Some(candidates[pos]);
                }
            }
            *type_counts.entry(stype.as_str()).or_insert(0) += 1;
            out.push(SiteInst { name: (*sname).clone(), site_type: stype.clone(), template });
        }
        Ok(out)
    }

    /// Match a site instance name/type to a template: first by
    /// `<prefix>_<template_name>` coordinate equality, then by unique site
    /// type. Returns `None` when ambiguous (e.g. BUFGCTRL banks, where site
    /// names carry die-level coordinates).
    fn match_template(
        templates: &[SiteTemplateData],
        inst_name: &str,
        site_type: &str,
    ) -> Option<u32> {
        for (i, t) in templates.iter().enumerate() {
            let full = format!("{}_{}", t.prefix, t.name);
            if full == inst_name && t.site_type == site_type {
                return Some(i as u32);
            }
        }
        let mut cand = None;
        for (i, t) in templates.iter().enumerate() {
            if t.site_type == site_type {
                if cand.is_some() {
                    return None;
                }
                cand = Some(i as u32);
            }
        }
        cand
    }

    /// Load `tile_type_*.json`, `segbits_*.db`, `mask_*.db`, `ppips_*.db` for
    /// one tile type. Missing files are tolerated (types like `NULL` have no
    /// DB files); the type name is recorded in `missing`.
    fn load_tile_type_db(
        artix7: &Path,
        name: &str,
        tdata: &mut TileTypeData,
        missing: &mut Vec<String>,
    ) -> Result<(), String> {
        let tt_path = artix7.join(format!("tile_type_{name}.json"));
        let tt: TileTypeFile = match load_tile_type(&tt_path) {
            Ok(tt) => tt,
            Err(_) => {
                missing.push(name.to_string());
                return Ok(());
            }
        };
        if tt.tile_type != name {
            return Err(format!(
                "tile_type file {} names type '{}', expected '{name}'",
                tt_path.display(),
                tt.tile_type
            ));
        }

        let mut wires: Vec<String> = tt.wires.keys().cloned().collect();
        wires.sort();
        tdata.wire_idx = wires.iter().enumerate().map(|(i, w)| (w.clone(), i as u32)).collect();
        tdata.wires = wires;

        // Site-pin wire set for pip classification (skip power rails: INT
        // tiles carry TIEOFF sites whose HARD pins hit VCC/GND wires).
        let mut site_pin_wires = std::collections::HashSet::new();
        for s in &tt.sites {
            for pin in s.site_pins.values() {
                if pin.wire != "VCC_WIRE" && pin.wire != "GND_WIRE" {
                    site_pin_wires.insert(pin.wire.clone());
                }
            }
            tdata.site_templates.push(SiteTemplateData {
                name: s.name.clone(),
                prefix: s.prefix.clone(),
                site_type: s.site_type.clone(),
                pins: {
                    let mut p: Vec<SitePin> =
                        s.site_pins.iter().map(|(k, v)| (k.clone(), v.wire.clone())).collect();
                    p.sort();
                    p
                },
            });
        }

        let is_int = name.starts_with("INT");
        let classify = |src_name: &str, dst_name: &str| -> PipClass {
            if is_int {
                PipClass::TileRouting
            } else {
                match (
                    site_pin_wires.contains(src_name),
                    site_pin_wires.contains(dst_name),
                ) {
                    (true, true) => PipClass::SiteInternal,
                    (false, true) => PipClass::SiteEntry,
                    (true, false) => PipClass::SiteExit,
                    (false, false) => PipClass::TileRouting,
                }
            }
        };
        let mut pip_names: Vec<&String> = tt.pips.keys().collect();
        pip_names.sort();
        for pname in pip_names {
            let p = &tt.pips[pname];
            // Endpoints come from the authoritative src_wire/dst_wire fields,
            // not the pip name: bidirectional pips are named `A<<->>B`.
            let (src_name, dst_name) = (p.src_wire.as_str(), p.dst_wire.as_str());
            let src = *tdata.wire_idx.get(src_name).ok_or_else(|| {
                format!("pip {pname}: unknown src wire '{src_name}'")
            })?;
            let dst = *tdata.wire_idx.get(dst_name).ok_or_else(|| {
                format!("pip {pname}: unknown dst wire '{dst_name}'")
            })?;
            let is_pseudo = loose_bool(&p.is_pseudo);
            let can_invert = loose_bool(&p.can_invert);
            let directional = loose_bool(&p.is_directional);
            let push = |tdata: &mut TileTypeData, s: u32, d: u32| {
                let idx = tdata.pips.len() as u32;
                tdata.pips.push(Pip {
                    src: s,
                    dst: d,
                    class: classify(&tdata.wires[s as usize], &tdata.wires[d as usize]),
                    is_pseudo,
                    can_invert,
                });
                tdata.pips_from.entry(s).or_default().push(idx);
            };
            push(tdata, src, dst);
            if !directional {
                // Bidirectional (`<<->>`) pips are traversable both ways.
                push(tdata, dst, src);
            }
        }

        let low = name.to_lowercase();
        let seg_path = artix7.join(format!("segbits_{low}.db"));
        if let Ok(text) = read_text(&seg_path) {
            for (i, line) in text.lines().enumerate() {
                if let Some((feat, bits)) = parse_segbits_line(line)
                    .map_err(|e| format!("{}:{}: {e}", seg_path.display(), i + 1))?
                {
                    tdata.segbits.insert(feat, bits);
                }
            }
        }
        let mask_path = artix7.join(format!("mask_{low}.db"));
        if let Ok(text) = read_text(&mask_path) {
            for (i, line) in text.lines().enumerate() {
                if let Some(bit) = parse_mask_line(line)
                    .map_err(|e| format!("{}:{}: {e}", mask_path.display(), i + 1))?
                {
                    tdata.mask.push(bit);
                }
            }
        }
        let pp_path = artix7.join(format!("ppips_{low}.db"));
        if let Ok(text) = read_text(&pp_path) {
            for (i, line) in text.lines().enumerate() {
                if let Some(pp) = parse_ppips_line(line)
                    .map_err(|e| format!("{}:{}: {e}", pp_path.display(), i + 1))?
                {
                    tdata.ppips.push(pp);
                }
            }
        }
        Ok(())
    }

    /// Stitch cross-tile nodes from `tileconn.json`.
    ///
    /// For each entry, every tile of `tile_types[0]` is paired with the tile
    /// of `tile_types[1]` at `(grid_x + dx, grid_y + dy)`; each wire pair is
    /// unioned into one node.
    fn stitch_nodes(&mut self, conn: &[TileConnEntry]) -> Result<(), String> {
        for e in conn {
            let a = *self
                .type_ids
                .get(&e.tile_types[0])
                .ok_or_else(|| format!("tileconn: unknown tile type '{}'", e.tile_types[0]))?;
            let b = *self
                .type_ids
                .get(&e.tile_types[1])
                .ok_or_else(|| format!("tileconn: unknown tile type '{}'", e.tile_types[1]))?;
            let tiles_a = self.tiles_by_type[a as usize].clone();
            for &ta in &tiles_a {
                let (gx, gy) = (self.tiles[ta].grid_x, self.tiles[ta].grid_y);
                let Some(&tb) = self.grid.get(&(gx + e.grid_deltas[0], gy + e.grid_deltas[1])) else {
                    continue;
                };
                if self.tiles[tb].type_id != b {
                    continue;
                }
                for pair in &e.wire_pairs {
                    let (wa, wb) = (&pair[0], &pair[1]);
                    let ia = self.types[a as usize].wire_idx.get(wa);
                    let ib = self.types[b as usize].wire_idx.get(wb);
                    let (Some(&ia), Some(&ib)) = (ia, ib) else { continue };
                    let ga = self.tiles[ta].wire_base + ia;
                    let gb = self.tiles[tb].wire_base + ib;
                    self.nodes.union(ga, gb);
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Query API
    // ------------------------------------------------------------------

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn tile(&self, idx: usize) -> Option<&Tile> {
        self.tiles.get(idx)
    }

    pub fn tile_by_name(&self, name: &str) -> Option<usize> {
        self.tile_by_name.get(name).copied()
    }

    pub fn tile_at(&self, grid_x: i32, grid_y: i32) -> Option<usize> {
        self.grid.get(&(grid_x, grid_y)).copied()
    }

    /// First global wire id of a tile (wires are `wire_base + wire_idx`).
    pub fn tile_wire_base(&self, tile_idx: usize) -> Option<u32> {
        self.tiles.get(tile_idx).map(|t| t.wire_base)
    }

    pub fn tile_type_name(&self, tile_idx: usize) -> Option<&str> {
        self.tiles
            .get(tile_idx)
            .map(|t| self.types[t.type_id as usize].name.as_str())
    }

    pub fn part(&self) -> &PartInfo {
        &self.part
    }

    /// All SLICEL/SLICEM sites: `(tile_name, site_name)`.
    pub fn slices(&self) -> Vec<(String, String)> {
        self.sites_of_types(&["SLICEL", "SLICEM"])
    }

    /// All IOB33 sites: `(tile_name, site_name)`.
    pub fn iob_sites(&self) -> Vec<(String, String)> {
        self.sites_of_types(&["IOB33", "IOB33S", "IOB33M"])
    }

    /// All BUFGCTRL sites: `(tile_name, site_name)`.
    pub fn bufg_sites(&self) -> Vec<(String, String)> {
        self.sites_of_types(&["BUFGCTRL"])
    }

    fn sites_of_types(&self, types: &[&str]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for t in &self.tiles {
            for s in &t.sites {
                if types.contains(&s.site_type.as_str()) {
                    out.push((t.name.clone(), s.name.clone()));
                }
            }
        }
        out.sort();
        out
    }

    /// Wire names of a tile.
    pub fn wires(&self, tile_idx: usize) -> Option<&[String]> {
        self.tiles
            .get(tile_idx)
            .map(|t| self.types[t.type_id as usize].wires.as_slice())
    }

    /// Index of a wire within its tile, if the tile type has it.
    pub fn wire_index(&self, tile_idx: usize, wire: &str) -> Option<u32> {
        self.tiles
            .get(tile_idx)
            .and_then(|t| self.types[t.type_id as usize].wire_idx.get(wire).copied())
    }

    /// Pips driven by `wire` in a tile.
    pub fn pips_from(&self, tile_idx: usize, wire: &str) -> Vec<&Pip> {
        let Some(t) = self.tiles.get(tile_idx) else { return Vec::new() };
        let td = &self.types[t.type_id as usize];
        let Some(&w) = td.wire_idx.get(wire) else { return Vec::new() };
        td.pips_from.get(&w).map(|v| v.iter().map(|&i| &td.pips[i as usize]).collect()).unwrap_or_default()
    }

    /// Find a site by full name: `(tile_idx, site_idx)`.
    pub fn site_by_name(&self, site_name: &str) -> Option<(usize, usize)> {
        for (ti, t) in self.tiles.iter().enumerate() {
            for (si, s) in t.sites.iter().enumerate() {
                if s.name == site_name {
                    return Some((ti, si));
                }
            }
        }
        None
    }

    /// `(pin_name, tile_wire)` pairs of a site, if its template matched.
    pub fn site_pins(&self, tile_idx: usize, site_idx: usize) -> Option<&[SitePin]> {
        let t = self.tiles.get(tile_idx)?;
        let s = t.sites.get(site_idx)?;
        let tmpl = s.template?;
        Some(self.types[t.type_id as usize].site_templates[tmpl as usize].pins.as_slice())
    }

    /// Look up a package pin, e.g. `package_pin("E3")`.
    pub fn package_pin(&self, pin: &str) -> Option<&PackagePin> {
        self.package_pins.get(pin)
    }

    /// Segbits for `feature` in a tile type, e.g. `segbits("INT_L", "INT_L.NL1BEG1.NN6END2")`.
    pub fn segbits(&self, tile_type: &str, feature: &str) -> Option<&[SegBit]> {
        let &id = self.type_ids.get(tile_type)?;
        self.types[id as usize].segbits.get(feature).map(Vec::as_slice)
    }

    /// Global wire id for `(tile, wire index)`.
    pub fn wire_id(&self, tile_idx: usize, wire_idx: u32) -> Option<u32> {
        self.tiles.get(tile_idx).map(|t| t.wire_base + wire_idx)
    }

    /// Node representative for a global wire id (path-compressed).
    pub fn node_of(&mut self, wire_id: u32) -> Option<u32> {
        if (wire_id as usize) < self.nodes.parent.len() {
            Some(self.nodes.find(wire_id))
        } else {
            None
        }
    }

    /// Number of distinct electrical nodes after stitching.
    pub fn node_count(&mut self) -> usize {
        let n = self.nodes.parent.len();
        let mut roots = std::collections::HashSet::with_capacity(n / 2);
        for i in 0..n as u32 {
            roots.insert(self.nodes.find(i));
        }
        roots.len()
    }

    /// Absolute frame address math for one segbit.
    ///
    /// Returns `(frame, word, bit)`.
    pub fn frame_bit(baseaddr: u32, offset: u32, bit: &SegBit) -> (u32, u32, u32) {
        (baseaddr + bit.frame_offset, offset + bit.bitpos / 32, bit.bitpos % 32)
    }

    /// Resolve a FASM feature on a tile to absolute `(frame, word, bit, value)` tuples.
    pub fn tile_feature_bits(
        &self,
        tile_idx: usize,
        bus_name: &str,
        feature: &str,
    ) -> Result<Vec<(u32, u32, u32, bool)>, String> {
        let t = self
            .tiles
            .get(tile_idx)
            .ok_or_else(|| format!("no tile {tile_idx}"))?;
        let bus = t
            .buses
            .iter()
            .find(|b| b.name == bus_name)
            .ok_or_else(|| format!("tile {} has no bus '{bus_name}'", t.name))?;
        let bits = self
            .segbits(&self.types[t.type_id as usize].name, feature)
            .ok_or_else(|| {
                format!("no segbits for feature '{feature}' in tile type '{}'", self.types[t.type_id as usize].name)
            })?;
        Ok(bits
            .iter()
            .map(|b| {
                let (f, w, bit) = Self::frame_bit(bus.baseaddr, bus.offset, b);
                (f, w, bit, b.value)
            })
            .collect())
    }
}

// Silence dead-code warnings for struct fields kept for future stages
// (emitter, router) while the flow logic lands.
#[allow(dead_code)]
fn _use_bus_info_fields(b: &BusInfo) {
    let _ = (b.frames, b.words);
}

#[allow(dead_code)]
fn _use_tile_grid_entry_fields(e: &TileGridEntry) {
    let _ = (&e.pin_functions, &e.prohibited_sites);
}

#[allow(dead_code)]
fn _use_tile_conn_fields(e: &TileConnEntry) {
    let _ = e;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    /// Device loaded once and shared by all tests (full load takes ~30 s).
    fn test_device() -> &'static Device {
        static DEV: OnceLock<Device> = OnceLock::new();
        DEV.get_or_init(|| Device::load_xc7a35t(&default_db_root()).expect("load xc7a35t"))
    }

    #[test]
    fn device_loads_and_counts() {
        let dev = test_device();
        assert_eq!(dev.tile_count(), 18055);
        // CLB tiles x2 slices. Note: the DB maps xc7a35t onto the xc7a50t die
        // fabric, so this is the 50T slice count (8150), not the 35T's 5200.
        let clb_tiles: usize = dev
            .tiles
            .iter()
            .filter(|t| dev.types[t.type_id as usize].name.starts_with("CLB"))
            .count();
        assert_eq!(clb_tiles, 4075);
        let slices = dev.slices();
        assert_eq!(slices.len(), clb_tiles * 2);
        assert!(slices.contains(&("CLBLL_L_X2Y0".to_string(), "SLICE_X0Y0".to_string())));
        let iobs = dev.iob_sites();
        assert!(!iobs.is_empty());
        let bufgs = dev.bufg_sites();
        assert_eq!(bufgs.len(), 32);
    }

    #[test]
    fn package_pin_e3_h5() {
        let dev = test_device();
        let e3 = dev.package_pin("E3").expect("E3");
        assert_eq!(e3.site, "IOB_X1Y76");
        assert_eq!(e3.tile, "RIOB33_X43Y75");
        assert_eq!(e3.bank, "35");
        let h5 = dev.package_pin("H5").expect("H5");
        assert_eq!(h5.site, "IOB_X1Y51");
        assert_eq!(h5.tile, "RIOB33_X43Y51");
        assert!(dev.package_pin("ZZ99").is_none());
    }

    #[test]
    fn frame_math_spot_check() {
        // INT_L_X0Y0 CLB_IO_CLK bus: baseaddr 0x00400000, offset 0.
        let (f, w, b) = Device::frame_bit(
            0x0040_0000,
            0,
            &SegBit { frame_offset: 21, bitpos: 7, value: true },
        );
        assert_eq!((f, w, b), (0x0040_0015, 0, 7));
        // bitpos >= 32 spills into the next word.
        let (f, w, b) = Device::frame_bit(
            0x0040_0000,
            2,
            &SegBit { frame_offset: 7, bitpos: 32, value: true },
        );
        assert_eq!((f, w, b), (0x0040_0007, 3, 0));
    }

    #[test]
    fn tile_feature_bits_resolve() {
        let dev = test_device();
        let tile = dev.tile_by_name("INT_L_X0Y0").unwrap();
        let bits = dev
            .tile_feature_bits(tile, "CLB_IO_CLK", "INT_L.BYP_ALT0.BYP_BOUNCE_N3_3")
            .unwrap();
        assert_eq!(bits.len(), 5);
        assert_eq!(bits[0], (0x0040_0015, 0, 7, true));
        assert_eq!(bits[1], (0x0040_0016, 0, 7, false));
        assert!(dev.tile_feature_bits(tile, "CLB_IO_CLK", "INT_L.NOPE").is_err());
        assert!(dev.tile_feature_bits(tile, "NO_BUS", "INT_L.BYP_ALT0.BYP_BOUNCE_N3_3").is_err());
    }

    #[test]
    fn pips_from_int_tile() {
        let dev = test_device();
        let tile = dev.tile_by_name("INT_L_X0Y0").unwrap();
        let pips = dev.pips_from(tile, "BYP_ALT0");
        assert!(!pips.is_empty());
        assert!(pips.iter().all(|p| p.class == PipClass::TileRouting));
        let wires = dev.wires(tile).unwrap();
        assert!(wires.contains(&"BYP_ALT0".to_string()));
        assert!(dev.pips_from(tile, "NO_SUCH_WIRE").is_empty());
    }

    #[test]
    fn clb_pip_classes() {
        let dev = test_device();
        let tile = dev.tile_by_name("CLBLL_L_X2Y0").unwrap();
        // CLBLL_L_A is a SLICE site pin wire -> pips driving it are SiteEntry.
        let pips = dev.pips_from(tile, "CLBLL_BYP0");
        assert!(!pips.is_empty());
        let classes: Vec<PipClass> = pips.iter().map(|p| p.class).collect();
        assert!(classes.contains(&PipClass::SiteEntry), "expected SiteEntry in {classes:?}");
    }

    #[test]
    fn site_pins_slice() {
        let dev = test_device();
        let (ti, si) = dev.site_by_name("SLICE_X0Y0").unwrap();
        let pins = dev.site_pins(ti, si).expect("slice pins");
        assert!(pins.iter().any(|(p, _)| p == "A"));
        let a_wire = pins.iter().find(|(p, _)| p == "A").unwrap().1.clone();
        assert!(a_wire.starts_with("CLBLL_"));
    }

    #[test]
    fn nodes_stitched() {
        let mut dev = test_device().clone();
        let total_wires: usize = dev.tiles.iter().map(|t| dev.types[t.type_id as usize].wires.len()).sum();
        let nodes = dev.node_count();
        assert!(nodes < total_wires, "stitching should merge wires: {nodes} vs {total_wires}");
        assert!(nodes > 0);
        // A stitched pair shares a node: BRAM_INT_INTERFACE_L IMUX wires.
        let tile = dev.tile_by_name("BRAM_INT_INTERFACE_L_X6Y0").unwrap();
        let t = &dev.tiles[tile];
        let w = dev.wire_index(tile, "INT_INTERFACE_BRAM_IMUX0").unwrap();
        let id_a = dev.wire_id(tile, w).unwrap();
        let nb = dev.tile_at(t.grid_x - 1, t.grid_y).unwrap();
        assert_eq!(dev.tile_type_name(nb), Some("BRAM_L"));
        let w2 = dev.wire_index(nb, "BRAM_IMUX0_0").unwrap();
        let id_b = dev.wire_id(nb, w2).unwrap();
        assert_eq!(dev.node_of(id_a), dev.node_of(id_b));
    }

    #[test]
    fn segbits_lookup() {
        let dev = test_device();
        let bits = dev.segbits("INT_L", "INT_L.NL1BEG1.NN6END2").expect("pip segbits");
        assert!(!bits.is_empty());
        assert!(dev.segbits("INT_L", "INT_L.NOPE").is_none());
        assert!(dev.segbits("NULL", "X").is_none());
    }

    #[test]
    fn part_info() {
        let dev = test_device();
        assert_eq!(dev.part().idcode, 0x0362_D093);
        assert_eq!(dev.part().frame_count().unwrap(), 5408);
    }
}
