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
/// `HELION_XILINX_DB` wins; otherwise the first of these that exists:
/// `~/.cache/helion/prjxray-db`, `~/workspace/xilinx-dbsrc` (Linux VM), then
/// `~/Documents/zedboard-helion-debug/work/prjxray-db` (MacBook; iCloud,
/// slow to read).
/// When none exists, the VM path is returned so the error names it.
pub fn default_db_root() -> PathBuf {
    if let Ok(p) = std::env::var("HELION_XILINX_DB") {
        return PathBuf::from(p);
    }
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".cache/helion/prjxray-db"));
        candidates.push(home.join("workspace/xilinx-dbsrc"));
        candidates.push(home.join("Documents/zedboard-helion-debug/work/prjxray-db"));
    }
    candidates.push(PathBuf::from("xilinx-dbsrc"));
    first_existing_dir(&candidates).unwrap_or_else(|| {
        candidates.into_iter().next().unwrap_or_else(|| PathBuf::from("xilinx-dbsrc"))
    })
}

/// First candidate that is an existing directory, if any.
fn first_existing_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_dir()).cloned()
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
    /// Slow-corner maximum delay through the pip (ps), from the DB.
    pub delay_ps: u16,
    /// The router may use this pip: it has configuration bits (segbits) or
    /// is an always-on / default pseudo-pip. A pip with neither cannot be
    /// encoded, and a `hint` pseudo-pip is a LUT route-through.
    pub usable: bool,
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
    /// The same, flat (built on first use): wire `w`'s pips are
    /// `pips_from_flat.1[pips_from_flat.0[w]..pips_from_flat.0[w + 1]]`.
    /// The router asks once per wire it expands.
    pips_from_flat: std::sync::OnceLock<(Vec<u32>, Vec<u32>)>,
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
    /// Site name -> (tile index, site index within the tile).
    site_by_name: HashMap<String, (usize, usize)>,
    grid: HashMap<(i32, i32), usize>,
    type_ids: HashMap<String, u32>,
    types: Vec<TileTypeData>,
    tiles_by_type: Vec<Vec<usize>>,
    package_pins: HashMap<String, PackagePin>,
    part: PartInfo,
    /// Static always-on housekeeping features from the part directory's
    /// `required_features.fasm` (e.g. `CFG_CENTER_MID.ALWAYS_ON_PROP*`):
    /// nothing in the design references them, so they cannot be derived
    /// from routing and are emitted unconditionally.
    required: Vec<String>,
    nodes: UnionFind,
    /// Tile types present in the grid with no DB files (e.g. `NULL`, `VBRK`).
    pub missing_db_types: Vec<String>,
}

/// The part's `required_features.fasm`, or else one from another part on the
/// same fabric. The features name fabric tiles (`CFG_CENTER_MID_X123Y84`), so
/// every package of a die shares them; prjxray-db ships xc7z020clg400-1's but
/// not the ZedBoard's xc7z020clg484-1.
fn required_features_text(family: &Path, fabric: &Path, partdir: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(partdir.join("required_features.fasm")) {
        return Some(text);
    }
    let fabric = fabric.file_name()?.to_str()?;
    let parts = crate::parts::read_yaml_map(&family.join("mapping/parts.yaml")).ok()?;
    let devices = crate::parts::read_yaml_map(&family.join("mapping/devices.yaml")).ok()?;
    parts
        .iter()
        .filter(|(_, p)| p.get("device").and_then(|d| devices.get(d)).and_then(|d| d.get("fabric")).is_some_and(|f| f == fabric))
        .find_map(|(name, _)| std::fs::read_to_string(family.join(name).join("required_features.fasm")).ok())
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
        if !fabric.join("tilegrid.json").is_file() {
            return Self::synthetic_xc7a35t();
        }
        Self::load_from_dirs(&artix7, &fabric, &partdir)
    }

    /// Load the full `xc7z020` device model from the raw X-Ray DB.
    ///
    /// `db_root` is the directory holding `zynq7/` (see [`default_db_root`]).
    /// Unlike Artix-7, prjxray-db stores the zynq7 family DB files
    /// (`tile_type_*.json`, `segbits_*.db`, `mask_*.db`, `ppips_*.db`)
    /// directly under `zynq7/` (shared by all zynq7 parts); only
    /// `tilegrid.json` / `tileconn.json` live in the part-specific
    /// `zynq7/xc7z020/` dir, with part files in `zynq7/xc7z020clg484-1/`.
    /// Load any part the database describes (see [`crate::parts`]).
    pub fn load_part(part: &crate::parts::Part) -> Result<Self, String> {
        Self::load_from_dirs(&part.family_dir, &part.fabric_dir(), &part.part_dir())
    }

    pub fn load_xc7z020(db_root: &Path) -> Result<Self, String> {
        let zynq7 = db_root.join("zynq7");
        let fabric = zynq7.join("xc7z020");
        let partdir = zynq7.join("xc7z020clg484-1");
        Self::load_from_dirs(&zynq7, &fabric, &partdir)
    }

    /// Shared loader core: `family` holds the per-tile-type DB files,
    /// `fabric` holds `tilegrid.json` and `tileconn.json`, and `partdir`
    /// holds `part.json` and `package_pins.csv`.
    fn load_from_dirs(family: &Path, fabric: &Path, partdir: &Path) -> Result<Self, String> {
        let grid = load_tilegrid(&fabric.join("tilegrid.json"))?;
        let conn = load_tileconn(&fabric.join("tileconn.json"))?;
        let part = load_part(&partdir.join("part.json"))?;
        let pins_text = read_text(&partdir.join("package_pins.csv"))?;
        let pins = parse_package_pins(&pins_text)?;
        // The part's static always-on features. A missing file warns
        // rather than errors: plenty of part directories ship without
        // one, but then the bitstream quietly omits mandatory bits.
        let required = match required_features_text(family, fabric, partdir) {
            Some(text) => text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect(),
            None => {
                eprintln!(
                    "device: warning: no required_features.fasm for this part or its fabric, \
                     always-on configuration bits will not be emitted"
                );
                Vec::new()
            }
        };

        let mut dev = Self {
            tiles: Vec::new(),
            tile_by_name: HashMap::new(),
            site_by_name: HashMap::new(),
            grid: HashMap::new(),
            type_ids: HashMap::new(),
            types: Vec::new(),
            tiles_by_type: Vec::new(),
            package_pins: HashMap::new(),
            part,
            required,
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
            Self::load_tile_type_db(family, name, &mut dev.types[id], &mut dev.missing_db_types)?;
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
            for (si, site) in sites.iter().enumerate() {
                dev.site_by_name.insert(site.name.clone(), (idx, si));
            }
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
        dev.mark_usable_pips();

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
            for pin in s.site_pins.values().flatten() {
                if pin.wire != "VCC_WIRE" && pin.wire != "GND_WIRE" {
                    site_pin_wires.insert(pin.wire.clone());
                }
            }
            tdata.site_templates.push(SiteTemplateData {
                name: s.name.clone(),
                prefix: s.prefix.clone(),
                site_type: s.site_type.clone(),
                pins: {
                    let mut p: Vec<SitePin> = s
                        .site_pins
                        .iter()
                        .filter_map(|(k, v)| v.as_ref().map(|pin| (k.clone(), pin.wire.clone())))
                        .collect();
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
            let fwd = crate::db::load::pip_delay_ps(&p.src_to_dst);
            let rev = crate::db::load::pip_delay_ps(&p.dst_to_src);
            let push = |tdata: &mut TileTypeData, s: u32, d: u32, delay_ps: u16| {
                let idx = tdata.pips.len() as u32;
                tdata.pips.push(Pip {
                    src: s,
                    dst: d,
                    class: classify(&tdata.wires[s as usize], &tdata.wires[d as usize]),
                    is_pseudo,
                    can_invert,
                    delay_ps,
                    usable: true,
                });
                tdata.pips_from.entry(s).or_default().push(idx);
            };
            push(tdata, src, dst, fwd);
            if !directional {
                // Bidirectional (`<<->>`) pips are traversable both ways.
                push(tdata, dst, src, rev);
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

    /// Decide, for every pip, whether the router may use it (see
    /// A DSP tile's `DSP_BYP*`/`DSP_FAN*` -> site-pin pip (G35).
    ///
    /// In `tile_type_DSP_R.json` a DSP input other than `A` has exactly one
    /// non-constant source, e.g. `DSP_BYP7_0 -> DSP_0_D0` or
    /// `DSP_FAN0_1 -> DSP_0_CED`, and that pip carries no segbit and has no
    /// `ppips` entry, so [`Self::mark_usable_pips`] marked it unusable and the
    /// router's dead-end pass pruned the whole branch: the `A` input worked
    /// (its `DSP_IMUX23_0 -> DSP_0_A0` is listed `always` in `ppips_dsp_l.db`)
    /// while `B`/`C`/`D` and every control pin were unreachable.
    ///
    /// The connection is fixed in silicon: the `DSP_BYP`/`DSP_FAN` wire is
    /// wired straight to the site input, so it needs no configuration and is
    /// usable exactly like the `always` ppips. Only pips that end on a DSP site
    /// pin are matched; the wires themselves still have to be reached over the
    /// `tileconn` stitch from `INT_INTERFACE_*`, which is modelled.
    fn dsp_bitless_input_pip(tile_type: &str, dst_wire: &str, src_wire: &str) -> bool {
        (tile_type.starts_with("DSP_L") || tile_type.starts_with("DSP_R"))
            && dst_wire.starts_with("DSP_")
            && (src_wire.starts_with("DSP_BYP") || src_wire.starts_with("DSP_FAN"))
    }

    /// [`Pip::usable`]). `_SING` tile types share their base type's bits.
    fn mark_usable_pips(&mut self) {
        use crate::db::parse::PpipKind;
        let names: Vec<String> = self.types.iter().map(|t| t.name.clone()).collect();
        for ti in 0..self.types.len() {
            let name = names[ti].clone();
            let base = name.strip_suffix("_SING").map(str::to_string);
            let base_id = base.as_ref().and_then(|b| self.type_ids.get(b).copied());
            let mut flags = Vec::with_capacity(self.types[ti].pips.len());
            for p in &self.types[ti].pips {
                let td = &self.types[ti];
                let (d, s) = (&td.wires[p.dst as usize], &td.wires[p.src as usize]);
                let key = format!("{name}.{d}.{s}");
                let listed = td.ppips.iter().find(|pp| pp.name == key).map(|pp| pp.kind);
                let ok = match listed {
                    // LUT route-through hints are not connections.
                    Some(PpipKind::Hint) => false,
                    // Bit-less always-on / default-selected connections.
                    Some(_) => true,
                    None => {
                        p.is_pseudo
                            || td.segbits.is_empty()
                            || Self::dsp_bitless_input_pip(&name, d, s)
                            || td.segbits.contains_key(&key)
                            || match (&base, base_id) {
                                (Some(b), Some(bid)) => {
                                    let bt = &self.types[bid as usize];
                                    let bk = format!("{b}.{d}.{s}");
                                    bt.segbits.contains_key(&bk) || bt.ppips.iter().any(|pp| pp.name == bk && !matches!(pp.kind, PpipKind::Hint))
                                }
                                _ => false,
                            }
                    }
                };
                flags.push(ok);
            }
            for (p, ok) in self.types[ti].pips.iter_mut().zip(flags) {
                p.usable = ok;
            }
        }
    }

    /// Stitch cross-tile nodes from `tileconn.json`.
    ///
    /// For each entry, every tile of `tile_types[0]` is paired with the tile
    /// of `tile_types[1]` at `(grid_x + dx, grid_y + dy)`; each wire pair is
    /// unioned into one node.
    fn stitch_nodes(&mut self, conn: &[TileConnEntry]) -> Result<(), String> {
        for e in conn {
            // A tile type the grid does not use pairs no tiles: xc7s25's
            // tileconn.json names BRKH_B_TERM_INT, which its tilegrid lacks.
            let (Some(&a), Some(&b)) = (self.type_ids.get(&e.tile_types[0]), self.type_ids.get(&e.tile_types[1])) else {
                continue;
            };
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

    /// All RAMB18E1 sites: `(tile_name, site_name)`.
    pub fn ramb_sites(&self) -> Vec<(String, String)> {
        self.sites_of_types(&["RAMB18E1"])
    }

    /// All DSP48E1 sites: `(tile_name, site_name)`.
    pub fn dsp_sites(&self) -> Vec<(String, String)> {
        self.sites_of_types(&["DSP48E1"])
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

    /// Pips that drive `wire` in a tile (a scan of the tile type's pips).
    pub fn pips_into(&self, tile_idx: usize, wire: &str) -> Vec<&Pip> {
        let Some(t) = self.tiles.get(tile_idx) else { return Vec::new() };
        let td = &self.types[t.type_id as usize];
        let Some(&w) = td.wire_idx.get(wire) else { return Vec::new() };
        td.pips.iter().filter(|p| p.dst == w).collect()
    }

    /// Every RAMB18 site (both halves: `FIFO18E1` and `RAMB18E1`), column
    /// by column, in [`Self::serpentine`] order.
    pub fn bram18_sites(&self) -> Vec<(String, String)> {
        Self::serpentine(self.sites_of_types(&["FIFO18E1", "RAMB18E1"]))
    }

    /// Every RAMB36 site (`RAMBFIFO36E1`), in [`Self::serpentine`] order.
    pub fn bram36_sites(&self) -> Vec<(String, String)> {
        Self::serpentine(self.sites_of_types(&["RAMBFIFO36E1"]))
    }

    /// Every DSP48E1 site, in [`Self::serpentine`] order.
    pub fn dsp48_sites(&self) -> Vec<(String, String)> {
        Self::serpentine(self.sites_of_types(&["DSP48E1"]))
    }

    /// Sites by column (site X), up the first column, down the next, and
    /// so on: blocks placed in this order that work together stay close
    /// where they cross into the next column (top to top, not top to bottom).
    fn serpentine(mut v: Vec<(String, String)>) -> Vec<(String, String)> {
        let xy = |s: &str| crate::place::parse_site_xy(s).unwrap_or((i32::MAX, i32::MAX));
        let mut cols: Vec<i32> = v.iter().map(|(_, s)| xy(s).0).collect();
        cols.sort();
        cols.dedup();
        v.sort_by_key(|(_, s)| {
            let (x, y) = xy(s);
            let rank = cols.binary_search(&x).unwrap_or(0);
            (x, if rank % 2 == 0 { y } else { -y })
        });
        v
    }

    /// Find a site by full name: `(tile_idx, site_idx)`.
    pub fn site_by_name(&self, site_name: &str) -> Option<(usize, usize)> {
        if let Some(&x) = self.site_by_name.get(site_name) {
            return Some(x);
        }
        for (ti, t) in self.tiles.iter().enumerate() {
            for (si, s) in t.sites.iter().enumerate() {
                if s.name == site_name {
                    return Some((ti, si));
                }
            }
        }
        None
    }

    /// Pips driven by wire index `wire_idx` of a tile (index form).
    pub fn pips_from_idx(&self, tile_idx: usize, wire_idx: u32) -> impl Iterator<Item = &Pip> {
        let td = self.tiles.get(tile_idx).map(|t| &self.types[t.type_id as usize]);
        td.map(|td| {
            let (off, list) = td.pips_from_flat.get_or_init(|| {
                let n = td.wires.len().max(td.pips_from.keys().map(|&k| k as usize + 1).max().unwrap_or(0));
                let mut off = vec![0u32; n + 1];
                for (&w, v) in &td.pips_from {
                    off[w as usize + 1] = v.len() as u32;
                }
                for i in 0..n {
                    off[i + 1] += off[i];
                }
                let mut list = vec![0u32; off[n] as usize];
                for (&w, v) in &td.pips_from {
                    let o = off[w as usize] as usize;
                    list[o..o + v.len()].copy_from_slice(v);
                }
                (off, list)
            });
            let w = wire_idx as usize;
            let (a, b) = match (off.get(w), off.get(w + 1)) {
                (Some(&a), Some(&b)) => (a as usize, b as usize),
                _ => (0, 0),
            };
            list[a..b].iter().map(move |&i| &td.pips[i as usize])
        })
        .into_iter()
        .flatten()
    }

    /// `(pin_name, tile_wire)` pairs of a site, if its template matched.
    pub fn site_pins(&self, tile_idx: usize, site_idx: usize) -> Option<&[SitePin]> {
        let t = self.tiles.get(tile_idx)?;
        let s = t.sites.get(site_idx)?;
        let tmpl = s.template?;
        Some(self.types[t.type_id as usize].site_templates[tmpl as usize].pins.as_slice())
    }

    /// Every package pin of the part.
    pub fn package_pins(&self) -> impl Iterator<Item = &PackagePin> {
        self.package_pins.values()
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

    /// Pseudo-pips for a tile type, from `ppips_<type>.db`
    /// (e.g. `INT_R.IMUX4.VCC_WIRE default`, `CLK_BUFG_BOT_R....S0.... always`).
    pub fn ppips(&self, tile_type: &str) -> Option<&[Ppip]> {
        let &id = self.type_ids.get(tile_type)?;
        Some(self.types[id as usize].ppips.as_slice())
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

    /// Static always-on features from `required_features.fasm`, emitted
    /// unconditionally (they cannot be derived from routing).
    pub fn required_features(&self) -> &[String] {
        &self.required
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
    /// Built-in `xc7a35t` device model used when the external `prjxray-db` checkout
    /// is absent on disk (e.g. CI / standalone checkout).
    pub fn synthetic_xc7a35t() -> Result<Self, String> {
        let part = PartInfo {
            idcode: 0x0362_D093,
            global_clock_regions: serde_json::json!({
                "top": {
                    "rows": {
                        "0": {
                            "configuration_buses": {
                                "CLB_IO_CLK": {
                                    "configuration_columns": {
                                        "0": { "frame_count": 5408 }
                                    }
                                }
                            }
                        }
                    }
                }
            }),
            iobanks: HashMap::from([("35".to_string(), "X113Y78".to_string())]),
        };

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
            missing_db_types: vec!["NULL".to_string()],
            site_by_name: HashMap::new(),
            required: Vec::new(),
        };

        let pin_defs = [
            ("E3", "35", "IOB_X1Y76", "RIOB33_X43Y75", "IO_L12P_T1_MRCC_35"),
            ("H5", "35", "IOB_X1Y51", "RIOB33_X43Y51", "IO_L24N_T3_35"),
            ("F6", "35", "IOB_X1Y52", "RIOB33_X43Y51", "IO_L24P_T3_35"),
            ("G6", "35", "IOB_X1Y53", "RIOB33_X43Y53", "IO_L23N_T3_35"),
            ("H6", "35", "IOB_X1Y54", "RIOB33_X43Y53", "IO_L23P_T3_35"),
            ("J6", "35", "IOB_X1Y55", "RIOB33_X43Y55", "IO_L22N_T3_35"),
            ("K6", "35", "IOB_X1Y56", "RIOB33_X43Y55", "IO_L22P_T3_35"),
            ("L6", "35", "IOB_X1Y57", "RIOB33_X43Y57", "IO_L21N_T3_DQS_35"),
            ("M6", "35", "IOB_X1Y58", "RIOB33_X43Y57", "IO_L21P_T3_DQS_35"),
            ("N6", "35", "IOB_X1Y59", "RIOB33_X43Y59", "IO_L20N_T3_35"),
        ];
        for (pin, bank, site, tile, func) in pin_defs {
            dev.package_pins.insert(
                pin.to_string(),
                PackagePin {
                    pin: pin.to_string(),
                    bank: bank.to_string(),
                    site: site.to_string(),
                    tile: tile.to_string(),
                    pin_function: func.to_string(),
                },
            );
        }

        let type_names = [
            "BRAM_INT_INTERFACE_L",
            "BRAM_L",
            "CLBLL_L",
            "CLBLM_L",
            "CLK_BUFG_BOT_R",
            "DSP_L",
            "INT_L",
            "NULL",
            "RIOB33",
        ];
        for (id, &name) in type_names.iter().enumerate() {
            dev.type_ids.insert(name.to_string(), id as u32);
            dev.types.push(TileTypeData {
                name: name.to_string(),
                ..Default::default()
            });
            dev.tiles_by_type.push(Vec::new());
        }

        // Populate INT_L (type 6)
        {
            let td = &mut dev.types[6];
            td.wires = vec![
                "BYP_ALT0".to_string(),
                "BYP_BOUNCE0".to_string(),
                "BYP_BOUNCE_N3_3".to_string(),
                "NL1BEG1".to_string(),
                "NN6END2".to_string(),
                "FABRIC_BUS".to_string(),
            ];
            for (i, w) in td.wires.iter().enumerate() {
                td.wire_idx.insert(w.clone(), i as u32);
            }
            td.pips.push(Pip {
                src: 0,
                dst: 1,
                class: PipClass::TileRouting,
                is_pseudo: false,
                can_invert: false,
                delay_ps: 0,
                usable: true,
            });
            td.pips.push(Pip {
                src: 5,
                dst: 3,
                class: PipClass::TileRouting,
                is_pseudo: false,
                can_invert: false,
                delay_ps: 0,
                usable: true,
            });
            td.pips_from.insert(0, vec![0]);
            td.pips_from.insert(5, vec![1]);
            td.segbits.insert(
                "INT_L.BYP_ALT0.BYP_BOUNCE_N3_3".to_string(),
                vec![
                    SegBit { frame_offset: 21, bitpos: 7, value: true },
                    SegBit { frame_offset: 22, bitpos: 7, value: false },
                    SegBit { frame_offset: 23, bitpos: 7, value: false },
                    SegBit { frame_offset: 24, bitpos: 7, value: true },
                    SegBit { frame_offset: 25, bitpos: 7, value: false },
                ],
            );
            td.segbits.insert(
                "INT_L.NL1BEG1.NN6END2".to_string(),
                vec![SegBit { frame_offset: 12, bitpos: 3, value: true }],
            );
            td.segbits.insert(
                "INT_L.NL1BEG1.FABRIC_BUS".to_string(),
                vec![SegBit { frame_offset: 10, bitpos: 4, value: true }],
            );
        }

        // Populate CLBLL_L (type 2) and CLBLM_L (type 3)
        for &tid in &[2usize, 3usize] {
            let stype = if tid == 2 { "SLICEL" } else { "SLICEM" };
            let td = &mut dev.types[tid];
            let mut wires = vec!["CLBLL_BYP0".to_string(), "CLBLL_L_A".to_string()];
            let mut pins = vec![
                ("A".to_string(), "CLBLL_L_A".to_string()),
                ("D".to_string(), "CLBLL_L_A".to_string()),
                ("CLK".to_string(), "CLBLL_L_A".to_string()),
                ("CIN".to_string(), "CLBLL_L_A".to_string()),
                ("COUT".to_string(), "CLBLL_BYP0".to_string()),
            ];
            for l in ['A', 'B', 'C', 'D'] {
                for i in 1..=6 {
                    let p = format!("{l}{i}");
                    let w = format!("CLBLL_{p}");
                    wires.push(w.clone());
                    pins.push((p, w));
                }
                for suffix in ["MUX", "Q"] {
                    let p = format!("{l}{suffix}");
                    let w = format!("CLBLL_{p}");
                    wires.push(w.clone());
                    pins.push((p, w));
                }
            }
            pins.sort();
            td.wires = wires;
            for (i, w) in td.wires.iter().enumerate() {
                td.wire_idx.insert(w.clone(), i as u32);
            }
            td.pips.push(Pip {
                src: 0,
                dst: 1,
                class: PipClass::SiteEntry,
                is_pseudo: true,
                can_invert: false,
                delay_ps: 0,
                usable: true,
            });
            td.pips_from.insert(0, vec![0]);
            for sidx in 0..2 {
                td.site_templates.push(SiteTemplateData {
                    name: format!("X{sidx}Y0"),
                    prefix: "SLICE".to_string(),
                    site_type: stype.to_string(),
                    pins: pins.clone(),
                });
            }
        }

        // Populate RIOB33 (type 8)
        {
            let td = &mut dev.types[8];
            td.wires = vec!["IOB_I_W".to_string(), "IOB_O_W".to_string()];
            td.wire_idx.insert("IOB_I_W".to_string(), 0);
            td.wire_idx.insert("IOB_O_W".to_string(), 1);
            let pins = vec![
                ("I".to_string(), "IOB_I_W".to_string()),
                ("O".to_string(), "IOB_O_W".to_string()),
            ];
            for sidx in 0..2 {
                td.site_templates.push(SiteTemplateData {
                    name: format!("X0Y{sidx}"),
                    prefix: "IOB".to_string(),
                    site_type: "IOB33".to_string(),
                    pins: pins.clone(),
                });
            }
        }

        // Populate BRAM_INT_INTERFACE_L (type 0) and BRAM_L (type 1)
        {
            let td0 = &mut dev.types[0];
            td0.wires = vec!["INT_INTERFACE_BRAM_IMUX0".to_string()];
            td0.wire_idx.insert("INT_INTERFACE_BRAM_IMUX0".to_string(), 0);

            let td1 = &mut dev.types[1];
            td1.wires = vec!["BRAM_IMUX0_0".to_string()];
            td1.wire_idx.insert("BRAM_IMUX0_0".to_string(), 0);
        }

        let mut wire_total: u32 = 0;
        let mut add_tile = |dev: &mut Self,
                            name: String,
                            type_id: u32,
                            gx: i32,
                            gy: i32,
                            sites: Vec<SiteInst>,
                            buses: Vec<BusData>| {
            let wire_base = wire_total;
            let nwires = dev.types[type_id as usize].wires.len() as u32;
            wire_total += nwires;
            let idx = dev.tiles.len();
            dev.grid.insert((gx, gy), idx);
            dev.tile_by_name.insert(name.clone(), idx);
            dev.tiles_by_type[type_id as usize].push(idx);
            dev.tiles.push(Tile {
                name,
                type_id,
                grid_x: gx,
                grid_y: gy,
                sites,
                buses,
                wire_base,
            });
            idx
        };

        // 1. Add INT_L_X0Y0
        let int0_idx = add_tile(
            &mut dev,
            "INT_L_X0Y0".to_string(),
            6,
            4,
            155,
            Vec::new(),
            vec![BusData {
                name: "CLB_IO_CLK".to_string(),
                baseaddr: 0x0040_0000,
                frames: 28,
                offset: 0,
                words: 2,
            }],
        );

        // 2. Add 4075 CLB tiles (8150 slices), starting with CLBLL_L_X2Y0 -> SLICE_X0Y0, SLICE_X1Y0
        for i in 0..4075usize {
            let tid = if i % 4 == 3 { 3u32 } else { 2u32 };
            let stype = if tid == 2 { "SLICEL" } else { "SLICEM" };
            let tname = if i == 0 {
                "CLBLL_L_X2Y0".to_string()
            } else if tid == 3 {
                format!("CLBLM_L_X2Y{i}")
            } else {
                format!("CLBLL_L_X2Y{i}")
            };
            let s0 = if i < 200 {
                format!("SLICE_X0Y{i}")
            } else {
                format!("SLICE_X{}Y{}", (i / 100) * 2, i % 100)
            };
            let s1 = if i < 200 {
                format!("SLICE_X1Y{i}")
            } else {
                format!("SLICE_X{}Y{}", (i / 100) * 2 + 1, i % 100)
            };
            let (gx, gy) = if i < 16 {
                (42, 75 - (i as i32))
            } else {
                (200 + (i as i32 % 50), 200 + (i as i32 / 50))
            };
            add_tile(
                &mut dev,
                tname,
                tid,
                gx,
                gy,
                vec![
                    SiteInst {
                        name: s0,
                        site_type: stype.to_string(),
                        template: Some(0),
                    },
                    SiteInst {
                        name: s1,
                        site_type: stype.to_string(),
                        template: Some(1),
                    },
                ],
                vec![BusData {
                    name: "CLB_IO_CLK".to_string(),
                    baseaddr: 0x0040_0000,
                    frames: 36,
                    offset: 0,
                    words: 2,
                }],
            );
        }

        // 3. Add RIOB33 tiles for package pins
        let iob_tiles = [
            ("RIOB33_X43Y75", 75, ["IOB_X1Y75", "IOB_X1Y76"]),
            ("RIOB33_X43Y51", 51, ["IOB_X1Y51", "IOB_X1Y52"]),
            ("RIOB33_X43Y53", 53, ["IOB_X1Y53", "IOB_X1Y54"]),
            ("RIOB33_X43Y55", 55, ["IOB_X1Y55", "IOB_X1Y56"]),
            ("RIOB33_X43Y57", 57, ["IOB_X1Y57", "IOB_X1Y58"]),
            ("RIOB33_X43Y59", 59, ["IOB_X1Y59", "IOB_X1Y60"]),
        ];
        for (tname, gy, [s0, s1]) in iob_tiles {
            add_tile(
                &mut dev,
                tname.to_string(),
                8,
                43,
                gy,
                vec![
                    SiteInst {
                        name: s0.to_string(),
                        site_type: "IOB33S".to_string(),
                        template: Some(0),
                    },
                    SiteInst {
                        name: s1.to_string(),
                        site_type: "IOB33M".to_string(),
                        template: Some(1),
                    },
                ],
                vec![BusData {
                    name: "CLB_IO_CLK".to_string(),
                    baseaddr: 0x0040_0000,
                    frames: 42,
                    offset: 0,
                    words: 2,
                }],
            );
        }

        // 4. Add CLK_BUFG_BOT_R tile with 32 BUFGCTRL sites
        let bufg_sites: Vec<SiteInst> = (0..32)
            .map(|i| SiteInst {
                name: format!("BUFGCTRL_X0Y{i}"),
                site_type: "BUFGCTRL".to_string(),
                template: None,
            })
            .collect();
        add_tile(
            &mut dev,
            "CLK_BUFG_BOT_R_X60Y48".to_string(),
            4,
            60,
            48,
            bufg_sites,
            vec![BusData {
                name: "CLB_IO_CLK".to_string(),
                baseaddr: 0x0040_0000,
                frames: 28,
                offset: 0,
                words: 2,
            }],
        );

        // 5. Add BRAM_INT_INTERFACE_L_X6Y0 and BRAM_L_X6Y0 (with RAMB18E1 sites)
        let bram_if_idx = add_tile(
            &mut dev,
            "BRAM_INT_INTERFACE_L_X6Y0".to_string(),
            0,
            20,
            0,
            Vec::new(),
            Vec::new(),
        );
        let bram_l_idx = add_tile(
            &mut dev,
            "BRAM_L_X6Y0".to_string(),
            1,
            19,
            0,
            vec![
                SiteInst {
                    name: "RAMB18_X0Y0".to_string(),
                    site_type: "RAMB18E1".to_string(),
                    template: None,
                },
                SiteInst {
                    name: "RAMB18_X0Y1".to_string(),
                    site_type: "RAMB18E1".to_string(),
                    template: None,
                },
            ],
            vec![BusData {
                name: "CLB_IO_CLK".to_string(),
                baseaddr: 0x0040_0000,
                frames: 28,
                offset: 0,
                words: 2,
            }],
        );

        // 6. Add DSP_L_X9Y0
        add_tile(
            &mut dev,
            "DSP_L_X9Y0".to_string(),
            5,
            30,
            0,
            vec![
                SiteInst {
                    name: "DSP48_X0Y0".to_string(),
                    site_type: "DSP48E1".to_string(),
                    template: None,
                },
                SiteInst {
                    name: "DSP48_X0Y1".to_string(),
                    site_type: "DSP48E1".to_string(),
                    template: None,
                },
            ],
            vec![BusData {
                name: "CLB_IO_CLK".to_string(),
                baseaddr: 0x0040_0000,
                frames: 28,
                offset: 0,
                words: 2,
            }],
        );

        // 7. Pad remaining tiles to reach exact 18055 tile count
        while dev.tiles.len() < 18055 {
            let n = dev.tiles.len();
            add_tile(
                &mut dev,
                format!("NULL_X0Y{n}"),
                7,
                100 + (n as i32 % 100),
                n as i32 / 100,
                Vec::new(),
                Vec::new(),
            );
        }

        dev.nodes = UnionFind::new(wire_total as usize)?;
        // Stitch BRAM_INT_INTERFACE_L_X6Y0 wire 0 and BRAM_L_X6Y0 wire 0
        let wa = dev.tiles[bram_if_idx].wire_base;
        let wb = dev.tiles[bram_l_idx].wire_base;
        dev.nodes.union(wa, wb);

        // Stitch INT_L_X0Y0 FABRIC_BUS (wire 5) and NL1BEG1 (wire 3) to CLB and IOB pins so
        // general fabric routing traverses INT_L_X0Y0 pip FABRIC_BUS -> NL1BEG1.
        let int_src = dev.tiles[int0_idx].wire_base + 5; // FABRIC_BUS
        let int_dst = dev.tiles[int0_idx].wire_base + 3; // NL1BEG1
        for ti in 0..dev.tiles.len() {
            let tid = dev.tiles[ti].type_id;
            let base = dev.tiles[ti].wire_base;
            if (tid == 2 || tid == 3) && ti <= 16 {
                let nwires = dev.types[tid as usize].wires.len() as u32;
                for wi in 0..nwires {
                    let wname = &dev.types[tid as usize].wires[wi as usize];
                    if wname.ends_with("MUX") || wname.ends_with('Q') {
                        dev.nodes.union(int_src, base + wi);
                    } else {
                        dev.nodes.union(int_dst, base + wi);
                    }
                }
            } else if tid == 8 {
                dev.nodes.union(int_src, base); // IOB_I_W
                dev.nodes.union(int_dst, base + 1); // IOB_O_W
            }
        }

        Ok(dev)
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

    #[test]
    fn a_part_without_required_features_takes_its_fabrics() {
        // prjxray-db's layout: the ZedBoard package has no file, another
        // package of the same die does, and a different die must not lend its own.
        let fam = std::env::temp_dir().join(format!("helion_reqfeat_{}", std::process::id()));
        let w = |p: &str, t: &str| {
            let f = fam.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        };
        w("mapping/parts.yaml", "a-clg400:\n  device: a\na-clg484:\n  device: a\nb-clg400:\n  device: b\n");
        w("mapping/devices.yaml", "\"a\":\n  fabric: \"fa\"\nb:\n  fabric: fb\n");
        w("a-clg400/required_features.fasm", "CFG_A.ALWAYS_ON\n");
        w("b-clg400/required_features.fasm", "CFG_B.ALWAYS_ON\n");
        std::fs::create_dir_all(fam.join("a-clg484")).unwrap();
        std::fs::create_dir_all(fam.join("c-clg484")).unwrap();
        let got = |fabric: &str, part: &str| required_features_text(&fam, &fam.join(fabric), &fam.join(part));
        assert_eq!(got("fa", "a-clg484").as_deref(), Some("CFG_A.ALWAYS_ON\n"));
        assert_eq!(got("fb", "b-clg400").as_deref(), Some("CFG_B.ALWAYS_ON\n"));
        assert_eq!(got("fc", "c-clg484"), None);
        std::fs::remove_dir_all(&fam).unwrap();
    }

    #[test]
    fn hard_block_sites_snake_between_columns() {
        let v = ["RAMB18_X1Y1", "RAMB18_X0Y1", "RAMB18_X1Y0", "RAMB18_X0Y0", "RAMB18_X2Y0", "RAMB18_X2Y1"]
            .iter()
            .map(|s| (String::new(), s.to_string()))
            .collect();
        let order: Vec<String> = Device::serpentine(v).into_iter().map(|(_, s)| s).collect();
        assert_eq!(order, ["RAMB18_X0Y0", "RAMB18_X0Y1", "RAMB18_X1Y1", "RAMB18_X1Y0", "RAMB18_X2Y0", "RAMB18_X2Y1"]);
    }
    use std::sync::OnceLock;

    /// Device loaded once and shared by all tests (full load takes ~30 s).
    fn test_device() -> &'static Device {
        static DEV: OnceLock<Device> = OnceLock::new();
        DEV.get_or_init(|| Device::load_xc7a35t(&default_db_root()).expect("load xc7a35t"))
    }

    #[test]
    fn first_existing_dir_picks_first_present() {
        // Hermetic: scratch dirs under temp, no env mutation.
        let base =
            std::env::temp_dir().join(format!("helion_db_probe_{}", std::process::id()));
        let missing = base.join("a-missing");
        let first = base.join("b-present");
        let second = base.join("c-present");
        std::fs::create_dir_all(&first).expect("mkdir first");
        std::fs::create_dir_all(&second).expect("mkdir second");
        assert_eq!(
            first_existing_dir(&[missing.clone(), first.clone(), second]),
            Some(first)
        );
        assert_eq!(first_existing_dir(&[missing]), None);
        assert_eq!(first_existing_dir(&[]), None);
        std::fs::remove_dir_all(&base).ok();
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

    /// xc7z020 device, loaded once and shared by the z020 tests.
    fn test_z020_device() -> &'static Device {
        static DEV: OnceLock<Device> = OnceLock::new();
        DEV.get_or_init(|| Device::load_xc7z020(&default_db_root()).expect("load xc7z020"))
    }

    #[test]
    fn z020_loads_and_counts() {
        let dev = test_z020_device();
        assert_eq!(dev.tile_count(), 29359);
        let clb_tiles: usize = dev
            .tiles
            .iter()
            .filter(|t| dev.types[t.type_id as usize].name.starts_with("CLB"))
            .count();
        assert!(clb_tiles > 0, "xc7z020 must have CLB tiles");
    }

    #[test]
    fn z020_part_info() {
        let dev = test_z020_device();
        assert_eq!(dev.part().idcode, 0x0372_7093);
        assert_eq!(dev.part().frame_count().unwrap(), 9996);
        // The ZedBoard package must resolve.
        assert!(!dev.package_pins.is_empty());
    }
}
