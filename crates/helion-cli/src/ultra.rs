//! UltraScale+ `helion impl` flow (xcku3p / xcvu3p / xczu3eg).
//!
//! Synth (helion-sv) -> pack -> place -> legality -> preliminary STA ->
//! route -> route-to-PIP -> bitgen. Every stage that is not implemented
//! for UltraScale+ fails loud with the stage name; there is no silent
//! fallback to another part and no empty bitstream file.
//!
//! Current honest state (US-09, see COORD.md gate):
//! - SYNTH, PACK, PLACE, LEGALITY, STA (preliminary) run.
//! - ROUTE runs on the real INT-tile routing graph (vendored INT tile
//!   JSON, one INT tile per placed slice, PROVISIONAL tile naming
//!   INT_X{x}Y{y} until the xcku3p tilegrid lands). Without a tileconn
//!   DB it fails loud with NoPath naming the first BEL pin with no
//!   wire. With `HELION_TILECONN_DB_ROOT` set to a prjuray-db family
//!   root, the graph is a `TileconnWireDb` instead: BEL pins resolve
//!   through the public `tileconn.json` map (US-01 wiredb lane) and
//!   the INT tiles are the real tilegrid tiles. xczu3eg still stops
//!   at preliminary STA before ROUTE (no xczu3eg STA DB, GAP-F5).
//! - ROUTE-TO-PIP translates routed nets to INT PIPs via the WireDb
//!   naming seam (CTO ruling 2026-11-07, option A); unnamed wires and
//!   non-edges fail loud. Multi-sink nets route as a flattened union
//!   of per-sink paths, which is not an ordered path, so translation
//!   fails loud on those until the router emits trees.
//! - BITGEN is wired (emit on the real xcku3p device file plus the
//!   vendored segbits DBs, with a parse-back set-bit check), but is
//!   unreachable until a design routes. CARRY8 designs fail loud at
//!   BITGEN: the flow has no CARRY8 FASM generator yet.
//! - FF FASM (complete, INIT/SRVAL default V0), FFMUX FASM (D6 for a
//!   same-slice same-letter LUT-O to FF-D edge, BYP otherwise), FF
//!   control FASM, and LUT INIT FASM are generated after legality.
//! - Clock nets are phase 2 for the router and IOBs have no placed
//!   site in this flow yet; nets touching them are skipped loudly at
//!   ROUTE, not routed.

use helion_ir::{CellKind, Design, intern, resolve};
use helion_ultrascale::bitgen::{
    BitgenError, emit_bitstream, emit_frames, parse_bitstream,
};
use helion_ultrascale::device_file::{DeviceFile, IntColumnTable, IntfColumnTable};
use helion_ultrascale::{
    best_seed_line, check_legality, dedup_results, emit_ff_control_fasm, emit_ff_fasm,
    emit_ffmux_fasm, emit_lut_fasm, escape_json_value, map_source_cell, pack_ultra,
    parse_results_text, place_ultra, route_ultra, seed_range, seeds_to_run, sta_db_for_part,
    sta_routed, summarize_multiseed, translated_to_fasm, ControlSetLimits, FabricWithPlaceholders,
    FileSegbitsDb, FfMuxSel, IntTileWireDb, RouteToPipError, RoutableNet, SeedResult, SourceCell,
    SourceKind, StaNetlist, TimingReport, TileconnWireDb, TranslatedRoute, UltraPlaceOpts,
    UltraRouteOpts, UltraScaleDevice, UltraScaleFabric, WireDb,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// True for the UltraScale+ parts this flow owns.
pub fn is_ultra_part(part: &str) -> bool {
    matches!(part, "xcku3p" | "xcvu3p" | "xczu3eg")
}

/// Vendored UltraScale+ data directory (segbits DBs, device files,
/// INT tile JSON). Compile-time repo path, overridable at runtime via
/// HELION_ULTRASCALE_DATA_DIR (tests point it at the crate data dir).
pub fn ultrascale_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("HELION_ULTRASCALE_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../helion-ultrascale/data")
}

/// Segbits DB files the emitter loads, in load order. Mirrors
/// helion-bitgen (crates/helion-ultrascale/src/bin/helion-bitgen.rs):
/// the CLB DBs carry LUT and FF encodings, the rest carry INT PIP,
/// IO, clock, and HDIO.
const SEGBITS_FILES: [&str; 9] = [
    "segbits_int.db",
    "segbits_hpio_right.db",
    "segbits_hdio_top_right.db",
    "segbits_hdio_bot_right.db",
    "segbits_cmt_right.db",
    "segbits_clel_l.db",
    "segbits_clel_r.db",
    "segbits_clem.db",
    "segbits_clem_r.db",
];

/// Load and merge the vendored segbits DBs, then synthesize the
/// single-feature LUT INIT lists. Fails loud on any missing or
/// malformed file: a silently partial DB would emit guessed bits.
pub fn load_ultra_segbits(data_dir: &Path) -> Result<FileSegbitsDb, String> {
    let paths: Vec<PathBuf> = SEGBITS_FILES
        .iter()
        .map(|name| data_dir.join(name))
        .collect();
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut db = FileSegbitsDb::from_files(&refs).map_err(|e| {
        format!(
            "ULTRA BITGEN: cannot load vendored segbits DBs from {}: {e}",
            data_dir.display()
        )
    })?;
    db.synthesize_lut_init()
        .map_err(|e| format!("ULTRA BITGEN: cannot synthesize LUT INIT entries: {e}"))?;
    Ok(db)
}

/// Device for a part: the real `DeviceFile` implementation of
/// `BitgenDevice` (SLICE base FARs from the file's own CLB column
/// majors, INT base FARs from the tilegrid table on xczu3eg and the
/// provisional rule on xcku3p, RAMB18 from the BRAM content columns,
/// full config order). `xcku3p` and `xczu3eg`
/// are wired: `xcvu3p.json` is provisional (computed frame total
/// 73,035 vs UG570 Table 1's 71,820) until that gap is resolved by
/// a CTO decision; any other part fails loud here. The flow emits
/// no HPIO lines (no IO placement), so no HPIO bridge is needed
/// here. xczu3eg: IDCODE 0x04a42093 is the PL configuration IDCODE
/// read from the file, confirmed by the public Vivado bitstream
/// P-13 (BNN-PYNQ cnvW1A1-ultra96.bit, md5
/// a56a86cf137c719c09f891a8b2472280, header part
/// xczu3eg-sbva484-1-i, big-endian bytes at offset 291); the AMD
/// forum figure 0x14710093 is the PS TAP IDCODE, not this one.
/// Total frames 14,952 (UG1085 Table 1 lists 14,964 for ZU3; the
/// 12-frame gap is 4 padding frames per FAR row x 3 rows, which
/// real Vivado streams carry and the decoder skips, so the
/// emitter writes one FDRI frame per real FAR, 14,952, and the
/// decoder does not require the padding). INT tiles resolve per the
/// tilegrid-derived INT column table attached here (INT_X2+ included),
/// and INT_INTF/HDIO tiles resolve per the INTF column table attached
/// here; both tables match the helion-bitgen binary device.
pub fn load_ultra_device(part: &str, data_dir: &Path) -> Result<DeviceFile, String> {
    match part {
        "xcku3p" => DeviceFile::from_path(&data_dir.join("xcku3p.json")).map_err(|e| {
            format!(
                "ULTRA BITGEN: cannot load device file {}: {e}",
                data_dir.join("xcku3p.json").display()
            )
        }),
        "xczu3eg" => {
            let mut file = DeviceFile::from_path(&data_dir.join("xczu3eg.json")).map_err(|e| {
                format!(
                    "ULTRA BITGEN: cannot load device file {}: {e}",
                    data_dir.join("xczu3eg.json").display()
                )
            })?;
            // Tilegrid-derived INT column table: without it the
            // provisional rule mispredicts INT columns for INT_X2
            // and above on this part (see bitgen_device.rs).
            let table =
                IntColumnTable::from_path(&data_dir.join("xczu3eg_int_columns.json")).map_err(
                    |e| {
                        format!(
                            "cannot load INT column table {}: {e}",
                            data_dir.join("xczu3eg_int_columns.json").display()
                        )
                    },
                )?;
            file.set_int_columns(table).map_err(|e| {
                format!("INT column table does not match xczu3eg.json: {e}")
            })?;
            // Tilegrid-derived INT_INTF / HDIO column table (B2):
            // without it INT_INTF and HDIO tiles do not resolve and
            // any interface line fails loud.
            let intf_table = IntfColumnTable::from_path(
                &data_dir.join("xczu3eg_intf_columns.json"),
            )
            .map_err(|e| {
                format!(
                    "cannot load INTF column table {}: {e}",
                    data_dir.join("xczu3eg_intf_columns.json").display()
                )
            })?;
            file.set_intf_columns(intf_table).map_err(|e| {
                format!("INTF column table does not match xczu3eg.json: {e}")
            })?;
            Ok(file)
        }
        other => Err(format!(
            "ULTRA BITGEN: part {other} has no emitter device yet: only xcku3p and xczu3eg are wired; \
            xcvu3p.json is provisional until its frame-total gap is resolved"
        )),
    }
}

/// Emit a bitstream for FASM text on a part. The FASM must already be
/// in canonical spelling (`.V0`/`.V1` FF lines, `[63:0]` LUT lines):
/// the flow generates it that way, so no valued-FF translation runs
/// here.
pub fn emit_ultra_bitstream(fasm: &str, part: &str, data_dir: &Path) -> Result<Vec<u8>, String> {
    let device = load_ultra_device(part, data_dir)?;
    let segbits = load_ultra_segbits(data_dir)?;
    emit_bitstream(fasm, &device, &segbits)
        .map_err(|e: BitgenError| format!("ULTRA BITGEN: emit failed: {e}"))
}

/// Parse a bitstream back to FASM on a part.
pub fn parse_ultra_bitstream(bytes: &[u8], part: &str, data_dir: &Path) -> Result<String, String> {
    let device = load_ultra_device(part, data_dir)?;
    let segbits = load_ultra_segbits(data_dir)?;
    parse_bitstream(bytes, &device, &segbits)
        .map_err(|e: BitgenError| format!("ULTRA BITGEN: parse-back failed: {e}"))
}

/// Set-bit equality of two FASM texts on a part: emit both frame maps
/// and compare the set bits. Empty frames carry no configuration, so
/// they are not a difference (the parse-back renders complement lines
/// for BELs the design never mentions, whose re-emit legitimately
/// carries empty frames the first emit never touched).
pub fn fasm_set_bits_equal(a: &str, b: &str, part: &str, data_dir: &Path) -> Result<bool, String> {
    let device = load_ultra_device(part, data_dir)?;
    let segbits = load_ultra_segbits(data_dir)?;
    let set_bits = |fasm: &str| -> Result<BTreeSet<(u32, u16)>, String> {
        let frames = emit_frames(fasm, &device, &segbits)
            .map_err(|e: BitgenError| format!("ULTRA BITGEN: emit failed: {e}"))?;
        let mut out = BTreeSet::new();
        for (far, words) in &frames {
            for (wi, word) in words.iter().enumerate() {
                let mut w = *word;
                while w != 0 {
                    let bit = w.trailing_zeros() as usize;
                    out.insert((*far, (wi * 32 + bit) as u16));
                    w &= w - 1;
                }
            }
        }
        Ok(out)
    };
    Ok(set_bits(a)? == set_bits(b)?)
}

/// Packer inputs extracted from a synthesized [`Design`].
///
/// Same extraction logic as the pack-place acceptance test
/// (`crates/helion-ultrascale/tests/corpus_legality.rs`); the two copies
/// are kept in sync by that test. The CLI copy lives here because
/// helion-ultrascale does not depend on helion-ir.
pub struct ExtractedCells {
    pub luts: Vec<(String, u64, bool)>,
    pub ffs: Vec<(String, String, Option<String>, Option<String>)>,
    pub carry_chains: Vec<Vec<String>>,
}

pub fn extract_cells(design: &Design) -> Result<ExtractedCells, String> {
    let mut luts = Vec::new();
    let mut ffs = Vec::new();
    let mut carry_cells: Vec<String> = Vec::new();
    // Carry4 cell -> CI net name, if any.
    let mut carry_ci: HashMap<String, Option<String>> = HashMap::new();
    // Net name -> Carry4 cell driving it through CO3/CO[3].
    let mut co3_driver: HashMap<String, String> = HashMap::new();

    let mut pin_to_net: HashMap<(helion_ir::Spur, helion_ir::Spur), helion_ir::Spur> =
        HashMap::new();
    for net in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.cell, ep.pin), net.name);
        }
    }
    let net_of = |cell: helion_ir::Spur, pin: &str| -> Option<String> {
        pin_to_net
            .get(&(cell, intern(pin)))
            .map(|s| resolve(*s).to_string())
    };

    for cell in &design.cells {
        let name = resolve(cell.name).to_string();
        // Every cell passes through the synthesis-mapping table first
        // (helion-ultrascale::synthmap): a cell with no UltraScale
        // mapping fails loud here with the missing primitive and its
        // unblocker, never silently dropped. A LUT6 carrying SRL or
        // distributed-RAM attributes fails the same way: it is not a
        // plain LUT6 and packing it as one would miscompile.
        let source_kind = match &cell.kind {
            CellKind::Lut6 { init } => SourceKind::Lut6 {
                init: *init,
                is_memory: cell.attrs.get("MEMORY").map(|s| s == "1").unwrap_or(false),
                is_srl: cell.attrs.map.contains_key("SRL_TYPE")
                    || cell.attrs.map.contains_key("SHREG_EXTRACT"),
            },
            CellKind::Hff => SourceKind::Ff {
                has_ce: net_of(cell.name, "CE").is_some(),
                has_sr: net_of(cell.name, "SR").is_some(),
            },
            CellKind::Carry4 => SourceKind::Carry4,
            CellKind::IobIn => SourceKind::IobIn,
            CellKind::IobOut => SourceKind::IobOut,
            CellKind::ClockBuf => SourceKind::ClockBuf,
            CellKind::Mac27 => SourceKind::Mac27,
            CellKind::Dsp48e1 { opmode, alumode } => SourceKind::Dsp48e1 {
                opmode: *opmode,
                alumode: *alumode,
            },
            CellKind::Ila { .. } => SourceKind::Ila,
            CellKind::Bram18 => SourceKind::Bram18,
            CellKind::Bram18e1 {
                read_width_a,
                read_width_b,
                write_width_a,
                write_width_b,
            } => SourceKind::Bram18e1 {
                read_width_a: *read_width_a,
                read_width_b: *read_width_b,
                write_width_a: *write_width_a,
                write_width_b: *write_width_b,
            },
            CellKind::BlackBox { module } => SourceKind::BlackBox {
                module: resolve(*module).to_string(),
            },
        };
        map_source_cell(&SourceCell {
            name: name.clone(),
            kind: source_kind,
        })
        .map_err(|e| format!("ULTRA PACK: {e}"))?;
        match cell.kind {
            CellKind::Lut6 { init } => {
                let is_mem = cell.attrs.get("MEMORY").map(|s| s == "1").unwrap_or(false);
                luts.push((name, init, is_mem));
            }
            CellKind::Hff => {
                let clk = net_of(cell.name, "CLK")
                    .ok_or_else(|| format!("ULTRA PACK: FF {name} has no CLK pin net"))?;
                let ce = net_of(cell.name, "CE");
                let sr = net_of(cell.name, "SR");
                ffs.push((name, clk, ce, sr));
            }
            CellKind::Carry4 => {
                carry_cells.push(name.clone());
                carry_ci.insert(name.clone(), net_of(cell.name, "CI"));
                for pin in ["CO3", "CO[3]"] {
                    if let Some(net) = net_of(cell.name, pin) {
                        co3_driver.insert(net, name.clone());
                    }
                }
            }
            CellKind::IobIn | CellKind::IobOut | CellKind::ClockBuf => {}
            // Every other kind was rejected by map_source_cell above.
            // This arm stays so a future CellKind variant fails loud
            // here instead of silently dropping.
            _ => {
                return Err(format!(
                    "ULTRA PACK: cell {name} of kind {:?} reached pack without a mapping",
                    cell.kind
                ));
            }
        }
    }

    // Build CARRY8 chains from Carry4 CI/CO3 links. Chain heads are cells
    // whose CI net is not driven by another Carry4's CO3 (CYINIT or a
    // constant). One Carry4 occupies one CARRY8 slice in this flow (the
    // upper 4 bits unused); the 2:1 pairing (synthmap::pair_carry4_chains:
    // chained pairs to SINGLE_CY8, independent pairs to DUAL_CY4) is not
    // wired into pack yet because PackedUltraSlice carries one carry cell
    // name per slice, so pairing needs a pack struct change first.
    let mut carry_chains: Vec<Vec<String>> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    for cell in &carry_cells {
        let is_head = match carry_ci.get(cell) {
            Some(Some(ci_net)) => !co3_driver.contains_key(ci_net),
            _ => true,
        };
        if !is_head || visited.contains(cell) {
            continue;
        }
        let mut chain = vec![cell.clone()];
        visited.insert(cell.clone());
        let mut cur = cell.clone();
        loop {
            let mut next = None;
            for cand in &carry_cells {
                if visited.contains(cand) {
                    continue;
                }
                if let Some(Some(ci_net)) = carry_ci.get(cand) {
                    if co3_driver.get(ci_net) == Some(&cur) {
                        next = Some(cand.clone());
                        break;
                    }
                }
            }
            match next {
                Some(n) => {
                    chain.push(n.clone());
                    visited.insert(n.clone());
                    cur = n;
                }
                None => break,
            }
        }
        carry_chains.push(chain);
    }
    // Any Carry4 not visited is part of a cycle or a broken link.
    for cell in &carry_cells {
        if !visited.contains(cell) {
            return Err(format!(
                "ULTRA PACK: Carry4 {cell} is not reachable from a chain head (cycle or broken CI link)"
            ));
        }
    }

    Ok(ExtractedCells {
        luts,
        ffs,
        carry_chains,
    })
}

/// Placed/routed netlist view for preliminary STA.
struct UltraStaNetlist<'a> {
    design: &'a Design,
    /// cell name -> (x, y) for LUTs and FFs.
    sites: HashMap<String, (i32, i32)>,
    /// net name -> (driver cell, x, y). Driver cell is "ff:<name>" for FF Q.
    drivers: HashMap<String, (String, i32, i32)>,
    /// cell name -> input net names.
    fanin: HashMap<String, Vec<String>>,
    ff_q: HashMap<String, String>,
    ff_d: HashMap<String, String>,
    /// Carry-chain cells (CARRY8), charged the carry8 s_to_o delay.
    carry: HashSet<String>,
}

impl<'a> StaNetlist for UltraStaNetlist<'a> {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        self.design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .filter_map(|c| {
                let name = resolve(c.name).to_string();
                self.sites.get(&name).map(|(x, y)| (name, *x, *y))
            })
            .collect()
    }

    fn luts(&self) -> Vec<(String, i32, i32)> {
        self.design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .filter_map(|c| {
                let name = resolve(c.name).to_string();
                self.sites.get(&name).map(|(x, y)| (name, *x, *y))
            })
            .collect()
    }

    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        self.drivers.clone()
    }

    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        self.fanin.clone()
    }

    fn ff_q_net(&self, ff: &str) -> Option<String> {
        self.ff_q.get(ff).cloned()
    }

    fn ff_d_net(&self, ff: &str) -> Option<String> {
        self.ff_d.get(ff).cloned()
    }

    fn carry_cells(&self) -> HashSet<String> {
        self.carry.clone()
    }
}

/// The routing graph the impl flow routes on: either the plain
/// INT-tile graph (no BEL-pin map; route fails loud naming the first
/// pin with no wire) or the tileconn graph (BEL pins resolved from
/// the public `tileconn.json` map). The two graphs share the
/// `WireDb` trait for routing but have different concrete
/// `translate_route` methods, so the enum carries both.
enum RouteDb {
    Int(Arc<IntTileWireDb>),
    Tileconn(Arc<TileconnWireDb>),
}

impl RouteDb {
    fn wires(&self) -> Arc<dyn WireDb> {
        match self {
            RouteDb::Int(db) => db.clone(),
            RouteDb::Tileconn(db) => db.clone(),
        }
    }

    fn translate_route(
        &self,
        route: &[helion_ultrascale::WireId],
        design: &str,
    ) -> Result<TranslatedRoute, RouteToPipError> {
        match self {
            RouteDb::Int(db) => db.translate_route(route, design),
            RouteDb::Tileconn(db) => db.translate_route(route, design),
        }
    }

    fn describe(&self) -> String {
        match self {
            RouteDb::Int(db) => format!(
                "tiles={} wires={} tileconn=none",
                db.tile_count(),
                db.wire_count()
            ),
            RouteDb::Tileconn(db) => format!(
                "tiles={} wires={} tileconn={} entries={} pairs={}",
                db.tile_count(),
                db.wire_count(),
                db.tileconn_db().part_dir.display(),
                db.tileconn_db().tileconn_entries,
                db.tileconn_db().tileconn_pairs
            ),
        }
    }
}

/// Build the routing graph for the placed slices. Without
/// `HELION_TILECONN_DB_ROOT` this is the plain INT-tile graph (the
/// US-09 behavior: no BEL-pin map). With it set to a prjuray-db
/// family root (the same directory the `helion-bitgen` decoder takes
/// as `--db-root`, e.g. `~/workspace/prjuray-db-ref/zynqusp`), the
/// graph is a `TileconnWireDb` loaded from that root for `part`.
/// `HELION_TILECONN_PART` optionally names the full part directory
/// (e.g. `xczu3eg-sbva484-1-e`); without it, the part directory is
/// resolved from `part` and must match exactly one directory, else
/// the load fails loud naming the candidates. The 84 MB
/// `tileconn.json` is never vendored: it is read from the DB root at
/// runtime (US-01 wiredb lane decision).
fn load_route_db(
    tile_json: &str,
    placed: &helion_ultrascale::UltraPlaced,
    part: &str,
) -> Result<RouteDb, String> {
    let Some(root) = std::env::var("HELION_TILECONN_DB_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        let db = IntTileWireDb::from_placed_slices(tile_json, placed)
            .map_err(|e| format!("ULTRA ROUTE: {e}"))?;
        return Ok(RouteDb::Int(Arc::new(db)));
    };
    let part_arg = std::env::var("HELION_TILECONN_PART")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| part.to_string());
    let db = TileconnWireDb::from_placed_slices(
        tile_json,
        Path::new(&root),
        &part_arg,
        placed,
    )
    .map_err(|e| format!("ULTRA ROUTE: tileconn load failed: {e}"))?;
    Ok(RouteDb::Tileconn(Arc::new(db)))
}

/// Run `helion impl` for an UltraScale+ part. Prints hang_diag lines and
/// returns Err naming the first blocking stage. `out_bit` / `out_fasm`
/// are the bitstream and FASM output paths; when None they default to
/// `{design}.{part}.bit` / `{design}.{part}.fasm` in the working
/// directory. No output file is written when the flow stops before
/// BITGEN.
pub fn run_impl(
    path: &str,
    part: &str,
    seed: u64,
    out_bit: Option<&str>,
    out_fasm: Option<&str>,
) -> Result<(), String> {
    let design = super::synth_any(path)?;
    let name = resolve(design.name).to_string();
    let device = match part {
        "xcku3p" => UltraScaleDevice::xcku3p(),
        "xcvu3p" => UltraScaleDevice::xcvu3p(),
        "xczu3eg" => UltraScaleDevice::xczu3eg(),
        _ => return Err(format!("ULTRA: unsupported part {part}")),
    };
    let fabric = match part {
        "xcku3p" => FabricWithPlaceholders::xcku3p(),
        "xcvu3p" => FabricWithPlaceholders::xcvu3p(),
        "xczu3eg" => FabricWithPlaceholders::xczu3eg(),
        _ => return Err(format!("ULTRA: unsupported part {part}")),
    };

    // SYNTH is done (synth_any above); extract packer inputs.
    let cells = extract_cells(&design)?;
    println!(
        "hang_diag ultra synth {} part={} luts={} ffs={} carry_chains={}",
        name,
        part,
        cells.luts.len(),
        cells.ffs.len(),
        cells.carry_chains.len()
    );

    // PACK.
    let limits = ControlSetLimits::default();
    let mut packed = pack_ultra(&cells.luts, &cells.ffs, &cells.carry_chains, &limits)
        .map_err(|e| format!("ULTRA PACK: {e}"))?;
    // X-pin repair: the packer cannot see D nets, so two FFs with
    // different BYP D nets can share one letter (one X pin). Move
    // the second one before place; see repair_ff_xpin_conflicts.
    repair_ff_xpin_conflicts(&design, &mut packed)?;
    println!(
        "hang_diag ultra pack slices={} slicel={} slicem={} luts={} ffs={} carry8={}",
        packed.slices.len(),
        packed.stats.slicel_count,
        packed.stats.slicem_count,
        packed.stats.lut_count,
        packed.stats.ff_count,
        packed.stats.carry8_count
    );

    // PLACE.
    let placed = place_ultra(
        &packed.slices,
        &device,
        &UltraPlaceOpts {
            seed,
            timing_weight: 0.0,
        },
    )
    .map_err(|e| format!("ULTRA PLACE: {e}"))?;
    println!(
        "hang_diag ultra place slices={} seed={}",
        placed.slices.len(),
        placed.seed
    );

    // LEGALITY.
    let violations = check_legality(&packed.slices, &placed.slices, &device, &limits);
    if !violations.is_empty() {
        let mut msg = String::from("ULTRA LEGALITY: placement illegal:\n");
        for v in &violations {
            msg.push_str(&format!("  {v}\n"));
        }
        return Err(msg);
    }
    println!("hang_diag ultra legality ok violations=0");

    // FF FASM (complete): every placed FF gets an INIT line and an SRVAL
    // line, default V0 when unspecified (CTO ruling 2026-11-07, complete
    // FASM; Xilinx default INIT/SRVAL 0). helion-ir Hff carries no
    // INIT/SRVAL params, so the attribute map is empty today and every
    // FF defaults to V0. Generated here, consumed by the BITGEN stage
    // once ROUTE lands; it does not depend on routing.
    let ff_fasm = emit_ff_fasm(&packed.slices, &placed, &HashMap::new())
        .map_err(|e| format!("ULTRA FASM: {e}"))?;
    println!("hang_diag ultra fasm ff_lines={}", ff_fasm.len());

    // FF control FASM (CEUSED/SRUSED/CLKINV/SRINV/SYNC per group,
    // synthmap): fails loud if two FFs in one group have different CE
    // or SR nets. Like
    // the INIT/SRVAL lines above, these are generated here and consumed
    // by the BITGEN stage once ROUTE lands.
    let ff_control_fasm =
        emit_ff_control_fasm(&packed.slices, &placed).map_err(|e| format!("ULTRA FASM: {e}"))?;
    println!(
        "hang_diag ultra fasm ff_control_lines={}",
        ff_control_fasm.len()
    );

    // FFMUX FASM (D6/BYP per FF, decided from the design nets): a FF
    // whose D is driven by the same-slice, same-letter LUT O takes D6
    // (intra-slice, no fabric route for that edge); any other driver
    // takes BYP (D arrives through the AX site pin, routed as today).
    // Generated here with the other FF lines, consumed at BITGEN.
    let ffmux_fasm = emit_ffmux_fasm(&packed.slices, &placed, &ffmux_selections(&design, &packed))
        .map_err(|e| format!("ULTRA FASM: {e}"))?;
    println!("hang_diag ultra fasm ffmux_lines={}", ffmux_fasm.len());

    // Preliminary STA on the placed netlist (Manhattan estimates, ballpark
    // delays; NOT signoff timing). The DB is selected by part (F3 fix:
    // xcku3p no longer silently runs on the xcvu3p DB) and the report is
    // labeled estimate=1 until a real delay DB exists.
    let timing = sta_on_placed(&design, &packed, &placed, part)?;
    println!("{}", timing.hang_diag_line(0));

    // ROUTE: build the real INT-tile routing graph for the placed
    // slices (vendored INT tile JSON). Without HELION_TILECONN_DB_ROOT
    // the graph has no BEL-pin to INT-wire map (US-01 data gap), so
    // route_ultra fails loud naming the first pin with no wire. With
    // the variable set, the graph is a TileconnWireDb loaded from the
    // public tileconn DB (US-01 wiredb lane) and the flow keeps its
    // own handle for the translator's bidirectional-PIP resolver.
    let data_dir = ultrascale_data_dir();
    let tile_json = std::fs::read_to_string(data_dir.join("tile_type_INT.json"))
        .map_err(|e| format!("ULTRA ROUTE: cannot read INT tile JSON: {e}"))?;
    let wire_db = load_route_db(&tile_json, &placed, part)?;
    println!("hang_diag ultra wiredb {}", wire_db.describe());
    let fabric_wires: Arc<dyn WireDb> = wire_db.wires();
    let fabric = fabric.with_wire_db(fabric_wires);
    let nets = build_routable_nets(&design, &packed, &placed)?;
    // Route budget has one home: UltraRouteOpts::default() (max_iters
    // 50). This call site takes the default rather than restating the
    // number, so the multiseed path and the impl path can never drift
    // apart on it once routing lands for multiseed.
    let routed = route_ultra(
        &nets,
        fabric.wires(),
        UltraRouteOpts {
            seed,
            ..Default::default()
        },
    )
    .map_err(|e| format!("ULTRA ROUTE: {e}"))?;
    println!(
        "hang_diag ultra route nets={} iters={} overused={}",
        routed.routes.len(),
        routed.iters_used,
        routed.overused_nodes
    );

    // ROUTE-TO-PIP: translate every routed net to INT PIPs through
    // the wire_info naming seam (INTERFACES.md). The concrete graph
    // resolves bidirectional PIPs from its PIP table; any unnamed
    // wire or non-edge fails loud here, never silently skipped.
    // Multi-sink nets route as a flattened union of per-sink paths;
    // the translator derives the PIP tree from that wire set (each
    // wire's parent is its latest preceding driver), so unions
    // translate instead of failing on the leg junctions.
    let mut translated: Vec<TranslatedRoute> = Vec::with_capacity(routed.routes.len());
    for (net, route) in nets.iter().zip(routed.routes.iter()) {
        translated.push(
            wire_db
                .translate_route(route, &net.name)
                .map_err(|e| format!("ULTRA ROUTE-TO-PIP: {e}"))?,
        );
    }
    let pip_fasm = translated_to_fasm(&translated);
    let n_pips: usize = translated.iter().map(|t| t.pips.len()).sum();
    println!("hang_diag ultra route-to-pip pips={n_pips}");

    // BITGEN: assemble complete FASM (LUT INIT, FF INIT/SRVAL, FF
    // control, INT PIPs) and emit the bitstream on the real xcku3p
    // device file. CARRY8 designs fail loud: the flow has no CARRY8
    // FASM generator yet, and emitting without the carry lines would
    // silently misconfigure the design.
    if packed.slices.iter().any(|s| s.carry8.is_some()) {
        return Err(
            "ULTRA BITGEN: design uses CARRY8; the flow has no CARRY8 FASM generator yet \
             (US-05 carry encoding exists in bitgen-core but the flow does not emit CARRY8 lines)"
                .to_string(),
        );
    }
    let lut_fasm =
        emit_lut_fasm(&packed.slices, &placed).map_err(|e| format!("ULTRA FASM: {e}"))?;
    println!("hang_diag ultra fasm lut_lines={}", lut_fasm.len());
    let mut fasm_lines: Vec<String> = Vec::new();
    fasm_lines.extend(lut_fasm);
    fasm_lines.extend(ff_fasm);
    fasm_lines.extend(ffmux_fasm);
    fasm_lines.extend(ff_control_fasm);
    fasm_lines.extend(pip_fasm.lines().map(|l| l.to_string()));
    let fasm_text = fasm_lines.join("\n") + "\n";
    let out_bit = out_bit
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{name}.{part}.bit"));
    let out_fasm = out_fasm
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{name}.{part}.fasm"));
    std::fs::write(&out_fasm, &fasm_text)
        .map_err(|e| format!("ULTRA BITGEN: cannot write FASM {out_fasm}: {e}"))?;
    let bytes = emit_ultra_bitstream(&fasm_text, part, &data_dir)?;
    std::fs::write(&out_bit, &bytes)
        .map_err(|e| format!("ULTRA BITGEN: cannot write bitstream {out_bit}: {e}"))?;
    // Parse-back: the emitted bitstream must decode to FASM that
    // re-emits identical set bits. A mismatch fails loud instead of
    // shipping an unverified bitstream.
    let back = parse_ultra_bitstream(&bytes, part, &data_dir)?;
    if !fasm_set_bits_equal(&fasm_text, &back, part, &data_dir)? {
        return Err(format!(
            "ULTRA BITGEN: parse-back of {out_bit} re-emits different set bits than the flow FASM"
        ));
    }
    println!(
        "hang_diag ultra bitgen bit={out_bit} fasm={out_fasm} bytes={} pips={n_pips} parse_back_lines={}",
        bytes.len(),
        back.lines().count()
    );
    Ok(())
}

/// Preliminary STA over the placed design.
fn sta_on_placed(
    design: &Design,
    packed: &helion_ultrascale::UltraPacked,
    placed: &helion_ultrascale::UltraPlaced,
    part: &str,
) -> Result<TimingReport, String> {
    // Map cell name -> placed (x, y) via its packed slice. Carry8 cells
    // live in `slice.carry8`, not in the LUT/FF lists.
    let mut sites: HashMap<String, (i32, i32)> = HashMap::new();
    for p in &placed.slices {
        let Some(slice) = packed.slices.get(p.packed_idx) else {
            continue;
        };
        for (_, cell) in slice.luts.iter().chain(slice.ffs.iter()) {
            sites.insert(cell.clone(), (p.x, p.y));
        }
        if let Some(cell) = &slice.carry8 {
            sites.insert(cell.clone(), (p.x, p.y));
        }
    }
    // Pin -> net map.
    let mut pin_to_net: HashMap<(helion_ir::Spur, helion_ir::Spur), helion_ir::Spur> =
        HashMap::new();
    for net in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.cell, ep.pin), net.name);
        }
    }
    let net_of = |cell: helion_ir::Spur, pin: &str| -> Option<String> {
        pin_to_net
            .get(&(cell, intern(pin)))
            .map(|s| resolve(*s).to_string())
    };

    let mut drivers: HashMap<String, (String, i32, i32)> = HashMap::new();
    let mut fanin: HashMap<String, Vec<String>> = HashMap::new();
    let mut ff_q: HashMap<String, String> = HashMap::new();
    let mut ff_d: HashMap<String, String> = HashMap::new();
    let mut carry: HashSet<String> = HashSet::new();
    for cell in &design.cells {
        let cname = resolve(cell.name).to_string();
        let Some(&(x, y)) = sites.get(&cname) else {
            continue;
        };
        match cell.kind {
            CellKind::Lut6 { .. } => {
                if let Some(o) = net_of(cell.name, "O") {
                    drivers.insert(o, (cname.clone(), x, y));
                }
                let mut ins = Vec::new();
                for pin in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                    if let Some(n) = net_of(cell.name, pin) {
                        ins.push(n);
                    }
                }
                fanin.insert(cname, ins);
            }
            CellKind::Hff => {
                if let Some(q) = net_of(cell.name, "Q") {
                    drivers.insert(q.clone(), (format!("ff:{cname}"), x, y));
                    ff_q.insert(cname.clone(), q);
                }
                if let Some(d) = net_of(cell.name, "D") {
                    ff_d.insert(cname.clone(), d);
                }
            }
            CellKind::Carry4 => {
                // Sum outputs O0..O3 drive FF D pins; carry outputs
                // CO0..CO3 (plus the CO[3] spelling) feed the next
                // chunk's CI. Fanin is the S/DI propagate/generate
                // inputs plus CI/CYINIT. Every output is charged the
                // carry8 s_to_o delay by sta_routed (see carry_cells).
                carry.insert(cname.clone());
                for lane in 0..4 {
                    for pin in [format!("O{lane}"), format!("CO{lane}")] {
                        if let Some(o) = net_of(cell.name, &pin) {
                            drivers.insert(o, (cname.clone(), x, y));
                        }
                    }
                }
                if let Some(o) = net_of(cell.name, "CO[3]") {
                    drivers.insert(o, (cname.clone(), x, y));
                }
                let mut ins = Vec::new();
                for lane in 0..4 {
                    for pin in [format!("S{lane}"), format!("DI{lane}")] {
                        if let Some(n) = net_of(cell.name, &pin) {
                            ins.push(n);
                        }
                    }
                }
                for pin in ["CI", "CYINIT"] {
                    if let Some(n) = net_of(cell.name, pin) {
                        ins.push(n);
                    }
                }
                fanin.insert(cname, ins);
            }
            _ => {}
        }
    }
    // Nets driven by top-level input ports (e.g. clk) and by constants
    // (e.g. CYINIT, the +1 operand of a counter's propagate LUTs) have
    // no cell driver; they arrive at time 0. Drop them from fanin lists
    // so STA traverses only driven nets instead of failing loud on a
    // missing driver for a primary input or constant.
    for ins in fanin.values_mut() {
        ins.retain(|n| drivers.contains_key(n));
    }
    // Nets driven by top-level input ports (e.g. clk) have no cell driver;
    // STA only traverses LUT/FF/carry fanin cones, so they are not needed here.
    let nl = UltraStaNetlist {
        design,
        sites,
        drivers,
        fanin,
        ff_q,
        ff_d,
        carry,
    };
    let db = sta_db_for_part(part).map_err(|e| format!("ULTRA STA: {e}"))?;
    sta_routed(&nl, &db, 10_000).map_err(|e| format!("ULTRA STA: {e}"))
}

/// Decide the FFMUX selection for every packed FF, from the design
/// nets. A FF whose D net is driven by a LUT O packed into the same
/// slice with the same BEL letter takes D6 (the LUT O feeds D directly
/// inside the slice); every other D source takes BYP: another LUT in
/// the same slice but a different letter, a LUT in another slice, an
/// FF Q, a carry output, or a port. A FF with no D net at all also
/// takes BYP (nothing drives its fabric input either). The decision is
/// per FF cell, keyed by cell name, and covers every packed FF by
/// construction; `emit_ffmux_fasm` fails loud on a missing entry.
/// The D net is recovered from `design.nets` here (the same pin_to_net
/// pattern as `sta_on_placed`): `PackedUltraSlice` carries no `ff_d`
/// field and `extract_cells` drops the D pin, so the packer cannot
/// supply it.
fn ffmux_selections(
    design: &Design,
    packed: &helion_ultrascale::UltraPacked,
) -> HashMap<String, FfMuxSel> {
    // Pin -> net, by resolved names.
    let mut pin_to_net: HashMap<(String, String), String> = HashMap::new();
    for net in &design.nets {
        let net_name = resolve(net.name).to_string();
        for ep in &net.endpoints {
            pin_to_net.insert(
                (resolve(ep.cell).to_string(), resolve(ep.pin).to_string()),
                net_name.clone(),
            );
        }
    }
    // Net -> driving LUT cell, for nets driven by a LUT O. Only a LUT
    // driver can select D6; any other driver means BYP.
    let mut lut_driver: HashMap<String, String> = HashMap::new();
    for cell in &design.cells {
        if !matches!(cell.kind, CellKind::Lut6 { .. }) {
            continue;
        }
        let cell_name = resolve(cell.name).to_string();
        if let Some(net) = pin_to_net.get(&(cell_name.clone(), "O".to_string())) {
            lut_driver.insert(net.clone(), cell_name);
        }
    }
    // Packed placement per cell: (packed slice index, BEL letter).
    // LUT letter is `bel`, FF letter is `bel / 2` (same mapping as
    // build_routable_nets).
    let mut lut_place: HashMap<String, (usize, char)> = HashMap::new();
    let mut ff_place: HashMap<String, (usize, char)> = HashMap::new();
    for (idx, slice) in packed.slices.iter().enumerate() {
        for (bel, cell) in &slice.luts {
            lut_place.insert(cell.clone(), (idx, (b'A' + *bel) as char));
        }
        for (bel, cell) in &slice.ffs {
            ff_place.insert(cell.clone(), (idx, (b'A' + (bel / 2)) as char));
        }
    }
    let mut sels = HashMap::new();
    for (ff_cell, (ff_slice, ff_letter)) in &ff_place {
        let sel = pin_to_net
            .get(&(ff_cell.clone(), "D".to_string()))
            .and_then(|d_net| lut_driver.get(d_net))
            .and_then(|lut_cell| lut_place.get(lut_cell))
            .map(|(lut_slice, lut_letter)| {
                if lut_slice == ff_slice && lut_letter == ff_letter {
                    FfMuxSel::D6
                } else {
                    FfMuxSel::Byp
                }
            })
            .unwrap_or(FfMuxSel::Byp);
        sels.insert(ff_cell.clone(), sel);
    }
    sels
}

/// Repair shared-X-pin conflicts after packing.
///
/// Two FFs in one slice letter share the one X pin (`{L}X`), so when
/// both select BYP (their D nets come from a LUT of another letter,
/// another slice, an FF Q, or a port) they must share one D net: the
/// X pin is a single physical input. The packer assigns FF BELs
/// without D-net knowledge (see `ffmux_selections`: `PackedUltraSlice`
/// carries no `ff_d` field), so it can pack two FFs with different BYP
/// D nets into one letter. That packing is unroutable: both D nets
/// must terminate at the same INT wire, and the router can only fail
/// with OveruseTimeout after 50 iters (counter: BQ/BQ2 share BX, so
/// cnt_1_d and cnt_2_d both sink at BYPASS_W1; the forced sharing
/// makes zero overuse impossible, see the counter lane report).
///
/// This repair moves the second BYP FF of a conflicted letter to the
/// first free letter of the same slice that can take it, preferring
/// its driver LUT's letter (the move then selects D6 and needs no
/// fabric route at all). A target letter can take the FF when it has
/// a free BEL and its resident BYP users (if any) share the moved
/// FF's D net; resident D6 users never consume the X pin. Moving
/// within the slice cannot change control sets. When no letter in the
/// slice can take the FF, the repair fails loud naming the FF, its D
/// net, and the slice, instead of emitting a packing the router
/// cannot configure. Runs after pack, before place, in `run_impl`
/// (and the multiseed pack-once site, so both flows pack alike).
/// Each move strictly reduces the conflict count, so the loop
/// terminates; the pass bound only guards against a logic error.
fn repair_ff_xpin_conflicts(
    design: &Design,
    packed: &mut helion_ultrascale::UltraPacked,
) -> Result<(), String> {
    // Pin -> net, and net -> driving LUT cell (same pattern as
    // ffmux_selections).
    let mut pin_to_net: HashMap<(String, String), String> = HashMap::new();
    for net in &design.nets {
        let net_name = resolve(net.name).to_string();
        for ep in &net.endpoints {
            pin_to_net.insert(
                (resolve(ep.cell).to_string(), resolve(ep.pin).to_string()),
                net_name.clone(),
            );
        }
    }
    let mut lut_driver: HashMap<String, String> = HashMap::new();
    for cell in &design.cells {
        if !matches!(cell.kind, CellKind::Lut6 { .. }) {
            continue;
        }
        let cell_name = resolve(cell.name).to_string();
        if let Some(net) = pin_to_net.get(&(cell_name.clone(), "O".to_string())) {
            lut_driver.insert(net.clone(), cell_name);
        }
    }
    for _ in 0..64 {
        // Current placements: LUT cell -> (slice, letter), FF cell ->
        // (slice, bel).
        let mut lut_place: HashMap<String, (usize, char)> = HashMap::new();
        let mut ff_place: HashMap<String, (usize, u8)> = HashMap::new();
        for (idx, slice) in packed.slices.iter().enumerate() {
            for (bel, cell) in &slice.luts {
                lut_place.insert(cell.clone(), (idx, (b'A' + *bel) as char));
            }
            for (bel, cell) in &slice.ffs {
                ff_place.insert(cell.clone(), (idx, *bel));
            }
        }
        // Selection and D net per FF (same D6 rule as
        // ffmux_selections).
        let mut ff_info: HashMap<String, (FfMuxSel, Option<String>)> = HashMap::new();
        for (ff_cell, (ff_slice, ff_bel)) in &ff_place {
            let ff_letter = (b'A' + (ff_bel / 2)) as char;
            let d_net = pin_to_net
                .get(&(ff_cell.clone(), "D".to_string()))
                .cloned();
            let sel = d_net
                .as_ref()
                .and_then(|n| lut_driver.get(n))
                .and_then(|lut_cell| lut_place.get(lut_cell))
                .map(|(lut_slice, lut_letter)| {
                    if lut_slice == ff_slice && lut_letter == &ff_letter {
                        FfMuxSel::D6
                    } else {
                        FfMuxSel::Byp
                    }
                })
                .unwrap_or(FfMuxSel::Byp);
            ff_info.insert(ff_cell.clone(), (sel, d_net));
        }
        // First conflict: one slice letter whose BYP users (FFs with
        // a D net selecting BYP) carry two distinct D nets. A FF with
        // no D net consumes no X pin and never counts here.
        let mut conflict: Option<(usize, u8, String, String, u8)> = None;
        'outer: for (idx, slice) in packed.slices.iter().enumerate() {
            for letter in 0..8u8 {
                let mut byp: Vec<(u8, String, String)> = Vec::new();
                for bel in [letter * 2, letter * 2 + 1] {
                    let Some(ff_cell) = slice.ffs.get(&bel) else {
                        continue;
                    };
                    let Some((sel, d_net)) = ff_info.get(ff_cell) else {
                        continue;
                    };
                    if *sel == FfMuxSel::Byp {
                        if let Some(n) = d_net {
                            byp.push((bel, ff_cell.clone(), n.clone()));
                        }
                    }
                }
                byp.sort_by_key(|(bel, _, _)| *bel);
                let mut distinct: Vec<&str> = Vec::new();
                for (_, _, n) in &byp {
                    if !distinct.contains(&n.as_str()) {
                        distinct.push(n.as_str());
                    }
                }
                if distinct.len() > 1 {
                    let keep = byp[0].2.clone();
                    let mover = byp.iter().find(|(_, _, n)| *n != keep).unwrap();
                    conflict = Some((
                        idx,
                        letter,
                        mover.1.clone(),
                        mover.2.clone(),
                        mover.0,
                    ));
                    break 'outer;
                }
            }
        }
        let Some((slice_idx, letter, mover_cell, mover_net, mover_bel)) = conflict else {
            return Ok(());
        };
        // Target letters in the same slice: the driver LUT's letter
        // first (the move then selects D6), then A..H in order.
        let driver_letter: Option<char> = lut_driver
            .get(&mover_net)
            .and_then(|lut_cell| lut_place.get(lut_cell))
            .filter(|(s, _)| *s == slice_idx)
            .map(|(_, l)| *l);
        let mut candidates: Vec<u8> = Vec::new();
        if let Some(l) = driver_letter {
            candidates.push(l as u8 - b'A');
        }
        for l in 0..8u8 {
            if !candidates.contains(&l) {
                candidates.push(l);
            }
        }
        let mut target: Option<(u8, u8)> = None;
        for tletter in candidates {
            if tletter == letter {
                continue;
            }
            for tbel in [tletter * 2, tletter * 2 + 1] {
                if packed.slices[slice_idx].ffs.contains_key(&tbel) {
                    continue;
                }
                // The move selects D6 in the driver letter, so it
                // consumes no X pin there and compatibility is
                // automatic. Else every resident BYP user of the
                // target letter must share the mover's D net.
                let becomes_d6 = driver_letter == Some((b'A' + tletter) as char);
                if !becomes_d6 {
                    let mut ok = true;
                    for rbel in [tletter * 2, tletter * 2 + 1] {
                        if let Some(rcell) = packed.slices[slice_idx].ffs.get(&rbel) {
                            if let Some((FfMuxSel::Byp, Some(rnet))) = ff_info.get(rcell) {
                                if *rnet != mover_net {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                }
                target = Some((tletter, tbel));
                break;
            }
            if target.is_some() {
                break;
            }
        }
        let Some((target_letter, new_bel)) = target else {
            return Err(format!(
                "ULTRA PACK: FF {mover_cell} (D net {mover_net}) shares the X pin of slice {} letter {} with a different BYP D net and no free compatible letter exists in the slice",
                packed.slices[slice_idx].name,
                (b'A' + letter) as char
            ));
        };
        let old_letter = (b'A' + letter) as char;
        let new_letter = (b'A' + target_letter) as char;
        let slice = &mut packed.slices[slice_idx];
        let cell = slice.ffs.remove(&mover_bel).expect("mover FF present");
        debug_assert_eq!(cell, mover_cell);
        let clk = slice.ff_clk.remove(&mover_bel);
        let ce = slice.ff_ce.remove(&mover_bel);
        let sr = slice.ff_sr.remove(&mover_bel);
        slice.ffs.insert(new_bel, cell);
        if let Some(v) = clk {
            slice.ff_clk.insert(new_bel, v);
        }
        if let Some(v) = ce {
            slice.ff_ce.insert(new_bel, v);
        }
        if let Some(v) = sr {
            slice.ff_sr.insert(new_bel, v);
        }
        println!(
            "hang_diag ultra pack xpin_repair ff={mover_cell} d_net={mover_net} slice={} letter {old_letter}->{new_letter}",
            packed.slices[slice_idx].name
        );
    }
    Err(
        "ULTRA PACK: X-pin repair did not converge within 64 passes (internal error)"
            .to_string(),
    )
}

/// Build routable nets from the placed design. Only reached once a real
/// WireDb exists. Nets touching IOBs or clock nets are skipped loudly
/// (one hang_diag line each, naming the net and the gap): IOBs have
/// no placed site in this flow yet, and clock nets are phase 2 for the
/// router. A skipped net is never routed, so the emitted bitstream
/// carries no configuration for it; that is the honest state until IO
/// placement and clock routing land. Carry4 cells fail loud: carry
/// routing is not implemented in this flow, and silently dropping
/// carry nets would misconfigure the design. Intra-slice LUT-O to
/// same-slice same-letter FF-D edges are also skipped, loudly: the FF
/// takes D6 and the edge needs no fabric route (see the filter below).
fn build_routable_nets(
    design: &Design,
    packed: &helion_ultrascale::UltraPacked,
    placed: &helion_ultrascale::UltraPlaced,
) -> Result<Vec<RoutableNet>, String> {
    use helion_ultrascale::UltraSite;
    if let Some(cell) = design
        .cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Carry4))
    {
        return Err(format!(
            "ULTRA ROUTE: cell {} is a Carry4; carry routing is not implemented in the flow yet",
            resolve(cell.name)
        ));
    }
    // Placed site per cell: (x, y) plus BEL name.
    let mut cell_site: HashMap<String, (i32, i32, String)> = HashMap::new();
    for p in &placed.slices {
        let Some(slice) = packed.slices.get(p.packed_idx) else {
            continue;
        };
        for (bel, cell) in &slice.luts {
            let letter = (b'A' + *bel) as char;
            cell_site.insert(cell.clone(), (p.x, p.y, format!("{letter}6LUT")));
        }
        for (bel, cell) in &slice.ffs {
            let letter = (b'A' + (bel / 2)) as char;
            let suffix = if bel % 2 == 0 { "Q" } else { "Q2" };
            cell_site.insert(cell.clone(), (p.x, p.y, format!("{letter}{suffix}")));
        }
        if let Some(cell) = &slice.carry8 {
            cell_site.insert(cell.clone(), (p.x, p.y, "CARRY8".to_string()));
        }
    }
    let site_of = |cell: &str| -> Result<(UltraSite, String), String> {
        cell_site
            .get(cell)
            .map(|(x, y, bel)| {
                (
                    UltraSite {
                        x: *x,
                        y: *y,
                        kind: 0,
                    },
                    bel.clone(),
                )
            })
            .ok_or_else(|| format!("ULTRA ROUTE: cell {cell} has no placed site"))
    };
    // Pin -> net map.
    let mut pin_to_net: HashMap<(helion_ir::Spur, helion_ir::Spur), helion_ir::Spur> =
        HashMap::new();
    for net in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.cell, ep.pin), net.name);
        }
    }
    // Net name -> driver (cell, pin) and sinks [(cell, pin)].
    let mut nets: Vec<RoutableNet> = Vec::new();
    for net in &design.nets {
        let net_name = resolve(net.name).to_string();
        let mut driver: Option<(String, String)> = None;
        let mut sinks: Vec<(String, String)> = Vec::new();
        for ep in &net.endpoints {
            let cell = resolve(ep.cell).to_string();
            let pin = resolve(ep.pin).to_string();
            let kind = design
                .cells
                .iter()
                .find(|c| c.name == ep.cell)
                .map(|c| &c.kind);
            match kind {
                Some(CellKind::Lut6 { .. }) if pin == "O" => {
                    driver = Some((cell, "O6".to_string()));
                }
                Some(CellKind::Hff) if pin == "Q" => {
                    driver = Some((cell, "Q".to_string()));
                }
                Some(CellKind::Lut6 { .. }) => sinks.push((cell, pin)),
                Some(CellKind::Hff) => sinks.push((cell, pin)),
                Some(CellKind::IobOut) | Some(CellKind::IobIn) => {
                    // Top-level port: no placed site, so this endpoint
                    // cannot route. The net drops out below (no driver
                    // or no sinks) and the skip is printed there.
                    println!(
                        "hang_diag ultra route skip net={net_name} reason=IOB cell {cell} has no placed site"
                    );
                }
                Some(CellKind::ClockBuf) => {
                    // Clock net: phase 2 for the router. Same skip as
                    // IOB endpoints above.
                    println!(
                        "hang_diag ultra route skip net={net_name} reason=clock net (driver {cell}); clock routing is phase 2"
                    );
                }
                _ => {}
            }
        }
        let Some((dcell, dpin)) = driver else {
            continue;
        };
        // Intra-slice LUT-O to FF-D edges are not fabric nets: when a
        // LUT O drives a FF D in the same placed slice with the same
        // BEL letter, the FF takes D6 and the edge needs no INT route
        // (FFMUX corpus census: Vivado encodes this edge as D6 in
        // 119,729 cases and as BYP with a fabric route in 7 of
        // 456,354). Drop those sinks from the routable net; a net
        // whose sinks are all intra-slice is dropped entirely. Other
        // sinks on the same net still route, so the LUT O still
        // reaches the fabric where it is consumed elsewhere, and the
        // FF D is never double-driven (no PIPs drive its AX pin for
        // this net). Only LUT O drivers are filtered: an FF Q or a
        // carry output driving an FF D still routes to AX (BYP).
        if dpin == "O6" {
            let n_before = sinks.len();
            if let Some((dx, dy, dbel)) = cell_site.get(&dcell) {
                let dletter = dbel.chars().next();
                sinks.retain(|(scell, spin)| {
                    if spin != "D" {
                        return true;
                    }
                    let Some((sx, sy, sbel)) = cell_site.get(scell) else {
                        return true;
                    };
                    let same_slice = sx == dx && sy == dy;
                    let same_letter = sbel.chars().next() == dletter;
                    !(same_slice && same_letter)
                });
            }
            if sinks.is_empty() && sinks.len() < n_before {
                println!(
                    "hang_diag ultra route skip net={net_name} reason=intra-slice LUT-O to same-letter FF-D (D6, no fabric route)"
                );
            }
        }
        if sinks.is_empty() {
            continue;
        }
        let (dsite, dbel) = site_of(&dcell)?;
        let mut rsinks = Vec::new();
        for (scell, spin) in &sinks {
            let (ssite, sbel) = site_of(scell)?;
            rsinks.push((ssite, sbel, spin.clone()));
        }
        nets.push(RoutableNet {
            name: net_name,
            driver: (dsite, dbel, dpin),
            sinks: rsinks,
            is_clock: false,
        });
    }
    Ok(nets)
}

/// One seed of the multiseed sweep: place with this seed, check
/// legality, run preliminary STA on the placed netlist.
///
/// Route is NOT attempted by the per-seed path: the impl flow routes
/// on the real INT-tile graph, but the sweep runs place plus
/// preliminary STA only, so every seed is route-blocked by
/// construction. `SeedResult::ok` and `SeedResult::fail` both record
/// `route_status: "blocked"`; that hardcoding must change the day the
/// sweep attempts routing. WNS here is a preliminary estimate
/// (ballpark delays, see the `multiseed` module docs), not signoff
/// timing.
struct UltraSeedRun {
    result: SeedResult,
    placed_slices: usize,
    endpoints: usize,
    r2r_ps: i64,
    iob_ps: i64,
}

fn run_one_seed(
    design: &Design,
    packed: &helion_ultrascale::UltraPacked,
    device: &UltraScaleDevice,
    part: &str,
    seed: u64,
) -> UltraSeedRun {
    let t0 = std::time::Instant::now();
    let fail = |reason: String| UltraSeedRun {
        result: SeedResult::fail(seed, t0.elapsed().as_secs_f64(), reason),
        placed_slices: 0,
        endpoints: 0,
        r2r_ps: 0,
        iob_ps: 0,
    };
    let placed = match place_ultra(
        &packed.slices,
        device,
        &UltraPlaceOpts {
            seed,
            timing_weight: 0.0,
        },
    ) {
        Ok(p) => p,
        Err(e) => return fail(format!("ULTRA PLACE: {e}")),
    };
    let placed_slices = placed.slices.len();
    let limits = ControlSetLimits::default();
    let violations = check_legality(&packed.slices, &placed.slices, device, &limits);
    if !violations.is_empty() {
        let mut msg = String::from("ULTRA LEGALITY: placement illegal:");
        for v in &violations {
            msg.push_str(&format!(" {v};"));
        }
        let mut run = fail(msg);
        run.placed_slices = placed_slices;
        return run;
    }
    match sta_on_placed(design, packed, &placed, part) {
        Ok(timing) => UltraSeedRun {
            result: SeedResult::ok(
                seed,
                timing.wns_ps,
                timing.tns_ps,
                t0.elapsed().as_secs_f64(),
            ),
            placed_slices,
            endpoints: timing.endpoints,
            r2r_ps: timing.r2r_ps,
            iob_ps: timing.iob_ps,
        },
        Err(e) => {
            let mut run = fail(e);
            run.placed_slices = placed_slices;
            run
        }
    }
}

/// Write one log file per seed under log_dir (seed-<n>.log) with the
/// placement and preliminary STA summary for that seed. Each worker
/// thread writes only its own seed's file, so no lock is needed. A
/// write failure is reported on stderr and never fails the sweep:
/// the JSONL record is the checkpoint, the log is detail.
fn write_seed_log(log_dir: &str, seed: u64, part: &str, run: &UltraSeedRun, runtime_s: f64) {
    let path = format!("{log_dir}/seed-{seed}.log");
    let mut text = format!(
        "seed={seed} part={part} runtime_s={runtime_s:.3} route_status={}\n",
        run.result.route_status
    );
    if run.result.status == "ok" {
        text += &format!(
            "status=ok wns_ps={} tns_ps={} endpoints={} r2r_ps={} iob_ps={} placed_slices={}\n",
            run.result.wns_ps,
            run.result.tns_ps,
            run.endpoints,
            run.r2r_ps,
            run.iob_ps,
            run.placed_slices
        );
    } else {
        text += &format!(
            "status=fail reason={}\n",
            run.result.reason.replace('\n', " ")
        );
    }
    if let Err(e) = std::fs::write(&path, text) {
        eprintln!("multiseed: cannot write {path}: {e}");
    }
}

/// Write the best-seed decision to a JSON file so a later step can
/// reproduce it without re-running the sweep: the best seed, its
/// numbers, and the exact `helion impl` command that rebuilds it.
/// `helion impl --seed N` routes to the UltraScale+ flow for ultra
/// parts and prints the same preliminary STA numbers before the
/// route gate stops it, so the command reproduces the sweep numbers
/// even while route is blocked. Written even when no seed met the
/// threshold (best_seed null), so scripts can tell "sweep finished,
/// no winner" from "no file".
fn write_best_out(
    path: &str,
    design: &str,
    part: &str,
    threshold: i64,
    best: Option<&SeedResult>,
) -> Result<(), String> {
    let body = match best {
        Some(b) => format!(
            "{{\"best_seed\": {}, \"wns_ps\": {}, \"tns_ps\": {}, \"route_status\": \"{}\", \"design\": \"{}\", \"part\": \"{}\", \"threshold_wns_ps\": {}, \"reproduce\": \"helion impl {} --part {} --seed {}\"}}",
            b.seed,
            b.wns_ps,
            b.tns_ps,
            escape_json_value(&b.route_status),
            escape_json_value(design),
            escape_json_value(part),
            threshold,
            escape_json_value(design),
            escape_json_value(part),
            b.seed
        ),
        None => format!(
            "{{\"best_seed\": null, \"design\": \"{}\", \"part\": \"{}\", \"threshold_wns_ps\": {}}}",
            escape_json_value(design),
            escape_json_value(part),
            threshold
        ),
    };
    std::fs::write(path, body + "\n").map_err(|e| format!("multiseed: cannot write {path}: {e}"))
}

/// Overnight multi-seed timing-closure sweep for an UltraScale+ part.
///
/// Port of the proof on fm/ultra-multiseed (e1c2e47): synth once,
/// pack once (seed independent), then place plus preliminary STA per
/// seed on a rayon pool sized by `jobs`, checkpointing each finished
/// seed to `out_path` as JSONL under a file mutex so a killed run
/// resumes where it stopped. Seeds already present in the file are
/// skipped. Best-seed rule: highest WNS at or above `threshold`,
/// tie-break lowest TNS then lowest seed.
///
/// Honest limits on this base: route is not attempted by the
/// per-seed path (the impl flow routes on the real INT-tile graph,
/// the sweep runs place plus preliminary STA only), so no seed
/// completes the full flow and no bitstream is produced. WNS is a
/// preliminary estimate from `preliminary_xcvu3p` ballpark delays
/// until the xcku3p delay DB exists (GAP-F5). The printed report
/// says both, every run.
///
/// `--log-dir` writes one log file per seed (seed-<n>.log) with the
/// place and preliminary STA summary. `--best-out` writes the
/// best-seed decision plus the exact `helion impl` command that
/// reproduces it. `--emit-best` is accepted and refused loud: the
/// sweep never routes, so no routed design exists to emit for the
/// best seed (run `helion impl --seed N` for the full flow instead).
/// `--retry-failed` re-runs seeds whose checkpoint record
/// is a failure instead of skipping them (transient failures); ok
/// seeds are never re-run.
pub fn run_multiseed(
    path: &str,
    part: &str,
    n_seeds: u64,
    seed_start: u64,
    jobs: usize,
    out_path: &str,
    threshold: i64,
    log_dir: Option<&str>,
    best_out: Option<&str>,
    emit_best: Option<&str>,
    retry_failed: bool,
) -> Result<(), String> {
    let (seed_first, seed_end) = seed_range(seed_start, n_seeds)?;
    if !is_ultra_part(part) {
        return Err(format!(
            "multiseed: part {part} is not an UltraScale+ part (xcku3p, xcvu3p, or xczu3eg)"
        ));
    }
    let jobs = jobs.max(1);

    // Synth once; pack once. Neither depends on the seed.
    let design = super::synth_any(path)?;
    let device = match part {
        "xcku3p" => UltraScaleDevice::xcku3p(),
        "xcvu3p" => UltraScaleDevice::xcvu3p(),
        "xczu3eg" => UltraScaleDevice::xczu3eg(),
        _ => return Err(format!("multiseed: unsupported part {part}")),
    };
    let fabric = match part {
        "xcku3p" => FabricWithPlaceholders::xcku3p(),
        "xcvu3p" => FabricWithPlaceholders::xcvu3p(),
        "xczu3eg" => FabricWithPlaceholders::xczu3eg(),
        _ => return Err(format!("multiseed: unsupported part {part}")),
    };
    let route_blocked = fabric.wires().wire_count() == 0;
    let cells = extract_cells(&design)?;
    let limits = ControlSetLimits::default();
    let mut packed = pack_ultra(&cells.luts, &cells.ffs, &cells.carry_chains, &limits)
        .map_err(|e| format!("ULTRA PACK: {e}"))?;
    // Same X-pin repair as the impl flow, so both flows pack alike.
    repair_ff_xpin_conflicts(&design, &mut packed)?;
    println!(
        "multiseed: design={} part={} synth luts={} ffs={} carry_chains={} packed_slices={}",
        path,
        part,
        cells.luts.len(),
        cells.ffs.len(),
        cells.carry_chains.len(),
        packed.slices.len()
    );

    if let Some(dir) = log_dir {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("multiseed: cannot create log dir {dir}: {e}"))?;
    }

    // Resume from the checkpoint file, parsed the same way the final
    // report parses it (last record per seed wins). The "already done"
    // count covers only seeds in this sweep's range.
    let prior: Vec<SeedResult> = std::fs::read_to_string(out_path)
        .map(|t| dedup_results(parse_results_text(&t)))
        .unwrap_or_default();
    let seeds = seeds_to_run(seed_first, seed_end, &prior, retry_failed);
    let n_already_done = (seed_end - seed_first) as usize - seeds.len();
    println!(
        "multiseed: design={} part={} seeds={}..{} jobs={} threshold_wns_ps={} ({} already done, {} to run) out={} route_blocked={} [WNS is a preliminary estimate from ballpark delays; route is not attempted by the per-seed path]",
        path,
        part,
        seed_first,
        seed_end - 1,
        jobs,
        threshold,
        n_already_done,
        seeds.len(),
        out_path,
        route_blocked
    );
    if seeds.is_empty() {
        println!("multiseed: nothing to run");
    } else {
        use std::io::Write;
        use std::sync::Mutex;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out_path)
            .map_err(|e| format!("multiseed: cannot open {out_path}: {e}"))?;
        let file = Mutex::new(file);
        let log_dir_ref = &log_dir;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .map_err(|e| format!("multiseed: cannot build thread pool: {e}"))?;
        pool.install(|| {
            use rayon::prelude::*;
            seeds.par_iter().for_each(|&seed| {
                let run = run_one_seed(&design, &packed, &device, part, seed);
                if let Some(dir) = log_dir_ref {
                    write_seed_log(dir, seed, part, &run, run.result.runtime_s);
                }
                let result = run.result;
                let line = result.to_jsonl();
                // A poisoned mutex (a worker panicked while holding
                // it) is recovered instead of panicking the sweep.
                let mut f = file.lock().unwrap_or_else(|e| e.into_inner());
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
                eprintln!(
                    "multiseed: seed={} status={} wns_ps={} tns_ps={} runtime_s={:.1} route={}",
                    result.seed,
                    result.status,
                    result.wns_ps,
                    result.tns_ps,
                    result.runtime_s,
                    result.route_status
                );
            });
        });
    }

    // Report: re-read the full JSONL (checkpointed plus this run) so
    // the best-seed decision is deterministic over everything on disk.
    // Last record per seed wins (see dedup_results).
    let mut results: Vec<SeedResult> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(out_path) {
        results = parse_results_text(&text);
    }
    let results = dedup_results(results);
    let summary = summarize_multiseed(&results, threshold);
    println!("{}", best_seed_line(&summary, threshold));
    if let (Some(min), Some(max)) = (summary.min_wns_ps, summary.max_wns_ps) {
        println!(
            "multiseed: WNS range over ok seeds: min={} max={} ps ({} seeds) [preliminary estimate]",
            min, max, summary.n_ok
        );
    }
    if let Some(best_path) = best_out {
        let best = helion_ultrascale::select_best_seed(&results, threshold);
        write_best_out(best_path, path, part, threshold, best)?;
        println!("multiseed: best-seed record wrote {best_path}");
    }
    if emit_best.is_some() {
        return Err(
            "multiseed: --emit-best refused: the sweep never routes, so no routed design exists to emit for the best seed (run `helion impl --seed N` for the full flow instead)"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ultrascale::{PackedUltraSlice, PlacedUltraSlice, SliceKind, UltraPacked, UltraPlaced};

    fn one_slice_design() -> (Design, UltraPacked, UltraPlaced) {
        // One LUT plus one FF in SLICE_X0Y0, with an IOB output and a
        // top-level clock input: the shape of blinky's placed netlist.
        let mut design = Design::new("blinky");
        design.add_cell(
            "u_lut",
            CellKind::Lut6 {
                init: 0x5555555555555555,
            },
        );
        design.add_cell("u_ff", CellKind::Hff);
        design.add_cell("u_iob", CellKind::IobOut);
        design.add_cell("u_clkin", CellKind::IobIn);
        design.connect("q", "u_lut", "O");
        design.connect("q", "u_ff", "D");
        design.connect("led", "u_ff", "Q");
        design.connect("led", "u_iob", "PAD");
        design.connect("clk", "u_clkin", "PAD");
        design.connect("clk", "u_ff", "CLK");
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice.luts.insert(0, "u_lut".to_string());
        slice.lut_inits.insert(0, 0x5555555555555555);
        slice.ffs.insert(0, "u_ff".to_string());
        slice.ff_clk.insert(0, "clk".to_string());
        let packed = UltraPacked {
            slices: vec![slice],
            ..Default::default()
        };
        let placed = UltraPlaced {
            slices: vec![PlacedUltraSlice {
                packed_idx: 0,
                site: "SLICE_X0Y0".to_string(),
                tile: "CLB_X0Y0".to_string(),
                x: 0,
                y: 0,
            }],
            seed: 0,
        };
        (design, packed, placed)
    }

    #[test]
    fn io_and_clock_nets_skip_loudly_not_error() {
        // The q net (LUT O to same-slice same-letter FF D) is
        // intra-slice (D6) and needs no fabric route, so it drops out
        // here; the led net (FF Q to the top-level port) and the clk
        // net (top-level input to FF CLK) drop out because their port
        // endpoints have no placed site. Skipping is printed, not
        // silent, and never errors.
        let (design, packed, placed) = one_slice_design();
        let nets = build_routable_nets(&design, &packed, &placed).unwrap();
        assert_eq!(nets.len(), 0, "no net needs a fabric route here");
    }

    #[test]
    fn ffmux_selections_same_letter_lut_is_d6() {
        // The fixture's FF D is driven by the same-slice, same-letter
        // (A) LUT O, so it takes D6.
        let (design, packed, _) = one_slice_design();
        let sels = ffmux_selections(&design, &packed);
        assert_eq!(sels.get("u_ff"), Some(&FfMuxSel::D6));
    }

    #[test]
    fn ffmux_selections_other_sources_are_byp() {
        // One LUT O drives three FF D pins: the same-slice
        // same-letter FF (D6), a same-slice different-letter FF
        // (BYP), and a FF in another slice (BYP).
        let mut design = Design::new("fanout");
        design.add_cell(
            "u_lut",
            CellKind::Lut6 {
                init: 0x5555555555555555,
            },
        );
        design.add_cell("u_ff", CellKind::Hff);
        design.add_cell("u_ff_b", CellKind::Hff);
        design.add_cell("u_ff_other", CellKind::Hff);
        design.add_cell("u_clkin", CellKind::IobIn);
        design.connect("q", "u_lut", "O");
        design.connect("q", "u_ff", "D");
        design.connect("q", "u_ff_b", "D");
        design.connect("q", "u_ff_other", "D");
        design.connect("clk", "u_clkin", "PAD");
        design.connect("clk", "u_ff", "CLK");
        design.connect("clk", "u_ff_b", "CLK");
        design.connect("clk", "u_ff_other", "CLK");
        let mut slice0 = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        slice0.luts.insert(0, "u_lut".to_string());
        slice0.lut_inits.insert(0, 0x5555555555555555);
        slice0.ffs.insert(0, "u_ff".to_string());
        slice0.ffs.insert(2, "u_ff_b".to_string());
        slice0.ff_clk.insert(0, "clk".to_string());
        slice0.ff_clk.insert(2, "clk".to_string());
        let mut slice1 = PackedUltraSlice::new("SLICE_X0Y1".to_string(), SliceKind::Logic);
        slice1.ffs.insert(0, "u_ff_other".to_string());
        slice1.ff_clk.insert(0, "clk".to_string());
        let packed = UltraPacked {
            slices: vec![slice0, slice1],
            ..Default::default()
        };
        let sels = ffmux_selections(&design, &packed);
        assert_eq!(sels.get("u_ff"), Some(&FfMuxSel::D6));
        assert_eq!(sels.get("u_ff_b"), Some(&FfMuxSel::Byp));
        assert_eq!(sels.get("u_ff_other"), Some(&FfMuxSel::Byp));
        // The shared q net still routes: it has fabric sinks (the two
        // BYP FFs), only the intra-slice same-letter sink is dropped.
        let placed = UltraPlaced {
            slices: vec![
                PlacedUltraSlice {
                    packed_idx: 0,
                    site: "SLICE_X0Y0".to_string(),
                    tile: "CLB_X0Y0".to_string(),
                    x: 0,
                    y: 0,
                },
                PlacedUltraSlice {
                    packed_idx: 1,
                    site: "SLICE_X0Y1".to_string(),
                    tile: "CLB_X0Y1".to_string(),
                    x: 0,
                    y: 1,
                },
            ],
            seed: 0,
        };
        let nets = build_routable_nets(&design, &packed, &placed).unwrap();
        assert_eq!(nets.len(), 1, "the shared q net still routes");
        assert_eq!(nets[0].name, "q");
        assert_eq!(nets[0].sinks.len(), 2, "only the D6 sink is dropped");
    }

    #[test]
    fn carry4_design_fails_loud_at_route_build() {
        // Carry routing is not implemented in the flow: a Carry4 cell
        // fails loud here instead of having its nets silently dropped.
        let mut design = Design::new("adder");
        design.add_cell("u_carry", CellKind::Carry4);
        let err = build_routable_nets(&design, &UltraPacked::default(), &UltraPlaced::default())
            .unwrap_err();
        assert!(err.contains("Carry4"), "got {err}");
    }

    fn workspace_example(name: &str) -> String {
        let rel = format!("examples/{name}");
        if std::path::Path::new(&rel).exists() {
            return rel;
        }
        format!("{}/../../{}", env!("CARGO_MANIFEST_DIR"), rel)
    }

    fn temp_out_paths(design: &str) -> (String, String) {
        let dir = std::env::temp_dir().join(format!("helion-us09-flow-{design}"));
        let _ = std::fs::create_dir_all(&dir);
        (
            dir.join(format!("{design}.xcku3p.bit"))
                .to_string_lossy()
                .into_owned(),
            dir.join(format!("{design}.xcku3p.fasm"))
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// The flow reaches the router and fails loud on the US-01
    /// pin-wire gap (not on the old empty-WireDb gate, not on
    /// IOB/clock nets). Nothing is written when the flow stops
    /// before BITGEN.
    fn assert_stops_at_route_without_pin_wire_map(design: &str, sv: &str) {
        let (out_bit, out_fasm) = temp_out_paths(design);
        let err = run_impl(
            &workspace_example(sv),
            "xcku3p",
            0,
            Some(&out_bit),
            Some(&out_fasm),
        )
        .unwrap_err();
        assert!(
            err.contains("ULTRA ROUTE"),
            "{design}: flow stopped at the wrong stage: {err}"
        );
        assert!(
            err.contains("has no wire"),
            "{design}: route error does not name the missing pin wire: {err}"
        );
        assert!(
            !std::path::Path::new(&out_bit).exists(),
            "{design}: bitstream file exists despite the ROUTE failure"
        );
        assert!(
            !std::path::Path::new(&out_fasm).exists(),
            "{design}: FASM file exists despite the ROUTE failure"
        );
    }

    #[test]
    fn blinky_stops_at_route_without_pin_wire_map() {
        assert_stops_at_route_without_pin_wire_map("blinky", "blinky.sv");
    }

    #[test]
    fn counter_stops_at_route_without_pin_wire_map() {
        assert_stops_at_route_without_pin_wire_map("counter", "counter.sv");
    }

    #[test]
    fn flow_fasm_emits_and_parses_back_with_identical_set_bits() {
        // The CLI's own bitgen path, end to end on the canonical flow
        // FASM for the golden blinky design (SLICE_X0Y0, CLEL_R):
        // emit, parse back, require identical set bits plus the
        // canonical lines.
        let data_dir = ultrascale_data_dir();
        let fasm = "\
SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555\n\
SLICE_X0Y0.AFF.INIT.V0\n\
SLICE_X0Y0.AFF.SRVAL.V0\n\
SLICE_X0Y0.ABCDFF.CEUSED.V0\n\
SLICE_X0Y0.ABCDFF.SRUSED.V0\n\
SLICE_X0Y0.ABCDFF.CLKINV.V0\n\
SLICE_X0Y0.ABCDFF.SRINV.V0\n\
INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n\
INT_X0Y0.PIP.INT_NODE_GLOBAL_0_INT_OUT0.GCLK_B_0_0\n";
        let bytes = emit_ultra_bitstream(fasm, "xcku3p", &data_dir).unwrap();
        assert!(!bytes.is_empty(), "emitted bitstream is empty");
        let back = parse_ultra_bitstream(&bytes, "xcku3p", &data_dir).unwrap();
        assert!(
            back.contains("SLICE_X0Y0.AFF.INIT.V0\n"),
            "parse-back renders canonical .V0, got:\n{back}"
        );
        assert!(
            back.contains("SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h5555555555555555\n"),
            "parse-back renders the LUT INIT value, got:\n{back}"
        );
        assert!(
            fasm_set_bits_equal(fasm, &back, "xcku3p", &data_dir).unwrap(),
            "parse-back re-emits different set bits than the flow FASM"
        );
    }

    #[test]
    fn xczu3eg_device_loads_with_part_idcode_and_frame_total() {
        use helion_ultrascale::bitgen::BitgenDevice;
        let data_dir = ultrascale_data_dir();
        let device = load_ultra_device("xczu3eg", &data_dir).unwrap();
        // Short device name; the full part string lives in part().
        assert_eq!(device.device(), "xczu3eg");
        assert_eq!(device.part(), "xczu3eg-sbva484-1-e");
        // The emitter sees the device through the BitgenDevice
        // trait, whose part() is the short name, not the full part
        // string (the inherent DeviceFile::part shadows it).
        assert_eq!(<DeviceFile as BitgenDevice>::part(&device), "xczu3eg");
        // PL configuration IDCODE, confirmed by public Vivado
        // bitstream P-13 (see load_ultra_device docs); the AMD
        // forum figure 0x14710093 is the PS TAP IDCODE, not this one.
        assert_eq!(device.slr_idcode("SLR0").unwrap(), 0x04A4_2093);
        assert_eq!(device.idcode().unwrap(), 0x04A4_2093);
        // The file's own total. UG1085 Table 1 lists 14,964 for
        // ZU3; the 12-frame gap is 4 padding frames per FAR row x
        // 3 rows, explained by padding frames in real bitstreams.
        // Do not tune this constant to match UG1085.
        assert_eq!(device.total_frames(), 14_952);
        assert_eq!(device.config_order_fars().len(), 14_952);
    }

    #[test]
    fn xczu3eg_flow_device_has_both_column_tables() {
        use helion_ultrascale::bitgen::BitgenDevice;
        let data_dir = ultrascale_data_dir();
        let device = load_ultra_device("xczu3eg", &data_dir).unwrap();
        assert!(device.has_int_columns(), "flow device lacks INT table");
        assert!(device.has_intf_columns(), "flow device lacks INTF table");
        assert_eq!(device.int_column_entries().unwrap().len(), 99);
        assert_eq!(device.intf_column_entries().unwrap().len(), 15);
        // xcku3p is untouched: no table is attached there.
        let ku = load_ultra_device("xcku3p", &data_dir).unwrap();
        assert!(!ku.has_int_columns());
        assert!(!ku.has_intf_columns());
        // INT_INTF/HDIO tiles resolve through the flow device.
        assert_eq!(
            device.tile_base_far("INT_INTF_R_PCIE4_X9Y120"),
            Some(0x81D00)
        );
        assert_eq!(
            device.tile_base_far("HDIO_BOT_RIGHT_X9Y0"),
            Some(0x1E00)
        );
    }

    #[test]
    fn xczu3eg_flow_device_int_columns_match_table_all_99() {
        use helion_ultrascale::bitgen::BitgenDevice;
        use helion_ultrascale::device_file::{Far, FarBlockType};
        let data_dir = ultrascale_data_dir();
        let device = load_ultra_device("xczu3eg", &data_dir).unwrap();
        // Anchors from the tilegrid table (mispredictions.txt):
        // X0/X1 agree with the provisional rule, X2/X10/X20/X32
        // mismatch it in every row.
        let anchors: [(u32, u32, u32); 18] = [
            (0, 0, 0x100),
            (0, 1, 0x400),
            (0, 2, 0x700),
            (0, 10, 0x2000),
            (0, 20, 0x3e00),
            (0, 32, 0x6300),
            (1, 0, 0x40100),
            (1, 1, 0x40400),
            (1, 2, 0x40700),
            (1, 10, 0x42000),
            (1, 20, 0x43e00),
            (1, 32, 0x46300),
            (2, 0, 0x80100),
            (2, 1, 0x80400),
            (2, 2, 0x80700),
            (2, 10, 0x82000),
            (2, 20, 0x83e00),
            (2, 32, 0x86300),
        ];
        for (row, x, far) in anchors {
            let tile = format!("INT_X{x}Y{}", row * 60);
            assert_eq!(
                device.tile_base_far(&tile),
                Some(far),
                "anchor {tile} must resolve to 0x{far:05x}"
            );
        }
        // Full loop over the vendored table: every one of the 99
        // (row, X) entries must resolve to Far(ClbIoClk, row, col, 0).
        let table =
            IntColumnTable::from_path(&data_dir.join("xczu3eg_int_columns.json")).unwrap();
        assert_eq!(table.columns.len(), 99);
        for ((row, x), col) in &table.columns {
            let tile = format!("INT_X{x}Y{}", row * 60);
            let expected = Far::new(FarBlockType::ClbIoClk, *row, *col, 0)
                .unwrap()
                .to_u32();
            assert_eq!(
                device.tile_base_far(&tile),
                Some(expected),
                "{tile} must resolve to table FAR 0x{expected:05x}"
            );
        }
    }

    #[test]
    fn xczu3eg_int_x2_pip_emits_at_table_far_not_provisional() {
        // Flip test for the baseline hand-FASM probe: the blinky PIP
        // INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14 carries a single
        // segbit (frame offset 17, bit 20) on INT_X2Y0, so bit 20 of
        // words[0] must land at FAR 0x711 (table base 0x700 + 17),
        // not at 0x411 (provisional base 0x400 + 17). Band bits live
        // at frame bits 1440..1487, so words[0] bit 20 is the PIP
        // bit alone.
        let data_dir = ultrascale_data_dir();
        let fasm = "INT_X2Y0.PIP.INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14\n";
        let bytes = emit_ultra_bitstream(fasm, "xczu3eg", &data_dir).unwrap();
        let frames = helion_ultrascale::bitgen::parse_frames(&bytes).unwrap();
        let mut hits: Vec<u32> = frames
            .iter()
            .filter(|(_, words)| (words[0] >> 20) & 1 == 1)
            .map(|(far, _)| *far)
            .collect();
        hits.sort_unstable();
        assert_eq!(hits, vec![0x711], "INT_X2Y0 PIP bit must land at 0x711");
    }

    /// Count single-frame FDRI packets in an emitted bitstream by
    /// walking the word stream: a type-1 write header to register 2
    /// (FDRI) with 93 words, then skip the 93 payload words.
    fn count_fdri_frames(bytes: &[u8]) -> usize {
        let words: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let fdri_header = (1u32 << 29) | (2u32 << 27) | (2u32 << 13) | 93;
        let mut count = 0;
        let mut i = 0;
        while i < words.len() {
            if words[i] == fdri_header {
                count += 1;
                i += 1 + 93;
            } else {
                i += 1;
            }
        }
        count
    }

    #[test]
    fn blinky_fasm_emits_and_parses_back_on_xczu3eg() {
        // The 13-line GAP-F5 FASM for blinky on xczu3eg (SLICE_X1Y0,
        // INT_X1Y0), as routed before the FFMUX lane: no FFMUX line,
        // and the LUT-O to FF-D edge still routed through the INT to
        // AX (the last two PIPs). Inlined here so the test does not
        // depend on a log file outside the repo; it pins the bitgen
        // path, not the current flow FASM (the flow now emits 12
        // lines with FFMUXA1.D6 and no AX leg). Every feature must
        // resolve through the vendored segbits DBs on the xczu3eg
        // device: a missing resolution fails loud in emit, it is not
        // skipped.
        let data_dir = ultrascale_data_dir();
        let fasm = "\
SLICE_X1Y0.A6LUT.INIT[63:0] = 0x5555555555555555\n\
SLICE_X1Y0.AFF.INIT.V0\n\
SLICE_X1Y0.AFF.SRVAL.V0\n\
SLICE_X1Y0.ABCDFF.CEUSED.V0\n\
SLICE_X1Y0.ABCDFF.SRUSED.V0\n\
SLICE_X1Y0.ABCDFF.CLKINV.V0\n\
SLICE_X1Y0.ABCDFF.SRINV.V0\n\
INT_X1Y0.PIP.INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14\n\
INT_X1Y0.PIP.BYPASS_W5.INT_NODE_IMUX_42_INT_OUT1\n\
INT_X1Y0.PIP.INT_NODE_IMUX_48_INT_OUT0.BYPASS_W5\n\
INT_X1Y0.PIP.IMUX_W10.INT_NODE_IMUX_48_INT_OUT0\n\
INT_X1Y0.PIP.INODE_W_1_FT1.LOGIC_OUTS_W0\n\
INT_X1Y0.PIP.BOUNCE_W_0_FT1.INODE_W_1_FT1\n";
        assert_eq!(fasm.lines().count(), 13, "blinky FASM line count");
        let bytes = emit_ultra_bitstream(fasm, "xczu3eg", &data_dir).unwrap();
        assert!(!bytes.is_empty(), "emitted bitstream is empty");
        // The emitter writes the PL configuration IDCODE, not the
        // PS TAP figure 0x14710093.
        assert!(
            bytes.windows(4).any(|w| w == [0x04, 0xa4, 0x20, 0x93]),
            "emitted bitstream carries no 0x04a42093 IDCODE word"
        );
        // One single-frame FDRI packet per real FAR: 14,952, not
        // the UG1085 14,964. The 12 padding frames real Vivado
        // streams carry are skipped by the decoder, not required.
        assert_eq!(count_fdri_frames(&bytes), 14_952);
        let back = parse_ultra_bitstream(&bytes, "xczu3eg", &data_dir).unwrap();
        assert!(
            back.contains("SLICE_X1Y0.AFF.INIT.V0\n"),
            "parse-back renders canonical .V0, got:\n{back}"
        );
        assert!(
            back.contains("SLICE_X1Y0.A6LUT.INIT[63:0] = 64'h5555555555555555\n"),
            "parse-back renders the LUT INIT value, got:\n{back}"
        );
        assert!(
            fasm_set_bits_equal(fasm, &back, "xczu3eg", &data_dir).unwrap(),
            "parse-back re-emits different set bits than the flow FASM"
        );
    }

    /// Flow-level test: `helion impl examples/blinky.sv --part xczu3eg`
    /// runs synth to BITGEN and emits a real bitstream. Needs the public
    /// prjuray-db tileconn root, so it skips (like tileconn_route.rs)
    /// when HELION_TILECONN_DB_ROOT is unset or absent. Two part
    /// directories live under zynqusp (sbva484 and sfvc784), so
    /// HELION_TILECONN_PART must also be set or the router fails loud
    /// on the ambiguous part. Run with:
    /// HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp
    /// HELION_TILECONN_PART=xczu3eg-sbva484-1-e cargo test --release
    /// -j1 -p helion-cli blinky_flow_emits_bitstream_on_xczu3eg
    ///
    /// What this pins, all measured on the 2026-11-22 flow run:
    /// - the flow reaches BITGEN (exit Ok) and writes .bit + .fasm;
    /// - the emitted FASM is exactly the 12-line FFMUX-flow FASM:
    ///   LUT INIT, FF INIT/SRVAL, FFMUXA1.D6 (the LUT O feeds the FF
    ///   D intra-slice, so the AX leg is gone and only the 4 PIPs of
    ///   the AQ feedback leg remain), FF control, all PIPs on
    ///   INT_X1Y0 (this run routes only through INT_X1, so any PIP on
    ///   INT_X2+ fails this test; INT_X2+ itself now resolves per the
    ///   table, see xczu3eg_int_x2_pip_emits_at_table_far_not_provisional);
    /// - IDCODE 0x04a42093 and 14,952 FDRI frames (not UG1085 14,964);
    /// - every stored H-1 band checks Exact under masked MAP-A
    ///   (no Q residual, no mismatch) over all 14,952 frames;
    /// - the standalone decoder round-trips all 12 flow lines, and
    ///   reports zero unexplained bits: the decoder now applies the
    ///   N7 band check, so the H-1 band bits (41 of them on the
    ///   GAP-F5 FASM, counted here before the port because the band
    ///   code is computed from frame contents) are explained instead
    ///   of listed. The band check above is Exact on every frame, so
    ///   a non-zero unexplained count here would mean the decoder's
    ///   mask derivation disagrees with the emitter, not a band
    ///   error.
    ///
    /// Deliberately NOT asserted here: US-09 verify-bit. The flow now
    /// emits FFMUXA1.D6, so the route leg's missing-driver failure is
    /// gone on the flow side, but cosim still depends on the
    /// unresolved FF INIT polarity flag (INTERFACES.md item 5): with
    /// D6 the extracted values invert against the RTL until one side
    /// of the V0/V1 mapping is fixed, and that fix belongs to the
    /// checker lane (tools/us09v/), not this flow. Asserting
    /// verify-bit here would enshrine a known-open polarity question
    /// as the pass bar.
    #[test]
    fn blinky_flow_emits_bitstream_on_xczu3eg() {
        use helion_ultrascale::band_code::{BandCheck, check_stored_band};
        let Ok(db_root) = std::env::var("HELION_TILECONN_DB_ROOT") else {
            println!(
                "blinky_flow_emits_bitstream_on_xczu3eg: skipping, \
                 HELION_TILECONN_DB_ROOT is unset"
            );
            return;
        };
        if !std::path::Path::new(&db_root).is_dir() {
            println!(
                "blinky_flow_emits_bitstream_on_xczu3eg: skipping, \
                 HELION_TILECONN_DB_ROOT {db_root} is absent"
            );
            return;
        }
        let Ok(part) = std::env::var("HELION_TILECONN_PART") else {
            println!(
                "blinky_flow_emits_bitstream_on_xczu3eg: skipping, \
                 HELION_TILECONN_PART is unset (zynqusp holds two part dirs)"
            );
            return;
        };
        assert_eq!(
            part, "xczu3eg-sbva484-1-e",
            "HELION_TILECONN_PART must be the sbva484 part for this test, got {part}"
        );
        let dir = std::env::temp_dir().join("helion-us09-flow-blinky-xczu3eg");
        let _ = std::fs::create_dir_all(&dir);
        let out_bit = dir.join("blinky.xczu3eg.bit");
        let out_fasm = dir.join("blinky.xczu3eg.fasm");
        let _ = std::fs::remove_file(&out_bit);
        let _ = std::fs::remove_file(&out_fasm);
        run_impl(
            &workspace_example("blinky.sv"),
            "xczu3eg",
            0,
            Some(&out_bit.to_string_lossy()),
            Some(&out_fasm.to_string_lossy()),
        )
        .unwrap();
        let bytes = std::fs::read(&out_bit).unwrap();
        assert!(!bytes.is_empty(), "emitted bitstream is empty");
        assert!(
            bytes.windows(4).any(|w| w == [0x04, 0xa4, 0x20, 0x93]),
            "emitted bitstream carries no 0x04a42093 IDCODE word"
        );
        assert_eq!(count_fdri_frames(&bytes), 14_952);
        let fasm = std::fs::read_to_string(&out_fasm).unwrap();
        let expected: [&str; 12] = [
            "SLICE_X1Y0.A6LUT.INIT[63:0] = 0x5555555555555555",
            "SLICE_X1Y0.AFF.INIT.V0",
            "SLICE_X1Y0.AFF.SRVAL.V0",
            "SLICE_X1Y0.FFMUXA1.D6",
            "SLICE_X1Y0.ABCDFF.CEUSED.V0",
            "SLICE_X1Y0.ABCDFF.SRUSED.V0",
            "SLICE_X1Y0.ABCDFF.CLKINV.V0",
            "SLICE_X1Y0.ABCDFF.SRINV.V0",
            "INT_X1Y0.PIP.INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14",
            "INT_X1Y0.PIP.BYPASS_W5.INT_NODE_IMUX_42_INT_OUT1",
            "INT_X1Y0.PIP.INT_NODE_IMUX_48_INT_OUT0.BYPASS_W5",
            "INT_X1Y0.PIP.IMUX_W10.INT_NODE_IMUX_48_INT_OUT0",
        ];
        assert_eq!(fasm.lines().count(), 12, "flow FASM line count");
        for line in expected {
            assert!(
                fasm.lines().any(|l| l == line),
                "flow FASM is missing line: {line}"
            );
        }
        for line in fasm.lines() {
            if let Some(rest) = line.strip_prefix("INT_X") {
                assert!(
                    rest.starts_with("1Y0."),
                    "PIP on an INT tile other than INT_X1Y0: {line}"
                );
            }
        }
        // H-1 band: blinky carries no LUTRAM, so the mask is all
        // false. Every frame must check Exact (a Q residual is
        // accepted by check_stored_band but must not appear here).
        let frames = helion_ultrascale::bitgen::parse_frames(&bytes).unwrap();
        assert_eq!(frames.len(), 14_952);
        let mask = [false; helion_ultrascale::bitgen::FRAME_BITS];
        let mut exact = 0usize;
        for (far, words) in &frames {
            match check_stored_band(*far, words, &mask) {
                BandCheck::Exact => exact += 1,
                BandCheck::QResidual => panic!("unexpected Q-residual band at far=0x{far:08x}"),
                BandCheck::Mismatch { stored, expected } => panic!(
                    "band mismatch far=0x{far:08x} stored=0x{stored:012x} \
                     expected=0x{expected:012x}"
                ),
            }
        }
        assert_eq!(exact, 14_952, "band Exact frame count");
        // Standalone decoder round-trip: the decoder now applies the
        // N7 band check, so the H-1 band bits are explained and the
        // unexplained set must be empty.
        let decoded =
            helion_ultrascale::bitstream_decode::decode_bitstream(&bytes, &db_root).unwrap();
        for line in expected {
            assert!(
                decoded.fasm.iter().any(|l| l == line),
                "decoded FASM is missing flow line: {line}"
            );
        }
        assert!(
            decoded.unexplained.is_empty(),
            "unexplained bits must be empty now that the decoder explains band bits: {:?}",
            decoded.unexplained
        );
    }

    /// Flow-level test: `helion impl examples/counter.sv --part xczu3eg`
    /// runs synth to BITGEN and emits a real bitstream. Same tileconn
    /// env gate as the blinky flow test above; run with:
    /// HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp
    /// HELION_TILECONN_PART=xczu3eg-sbva484-1-e cargo test --release
    /// -j1 -p helion-cli counter_flow_emits_bitstream_on_xczu3eg
    ///
    /// What this pins: the counter routes (6 nets, 0 overuse) after
    /// the X-pin repair and the own-trunk router fix, and BITGEN
    /// emits a 5,982,532-byte bitstream with IDCODE 0x04a42093 and
    /// 14,952 FDRI frames. Only the stable FASM lines are pinned:
    /// the 4 LUT INITs, the 8 FF INIT/SRVAL lines, the 4 FFMUX
    /// lines (A1.D6, A2.BYP, B1.BYP, D1.D6: the repair moved cnt_2's
    /// FF to letter D, so its D takes D6), and the 6 FF control
    /// lines. The PIP set is NOT pinned: the router is
    /// nondeterministic across processes (fanout order comes from a
    /// HashMap), so the PIPs vary run to run; the test asserts only
    /// that every PIP is on INT_X1Y0 and that at least one PIP was
    /// emitted. On the base this test fails at ROUTE with
    /// OveruseTimeout and writes no .bit.
    #[test]
    fn counter_flow_emits_bitstream_on_xczu3eg() {
        use helion_ultrascale::band_code::{BandCheck, check_stored_band};
        let Ok(db_root) = std::env::var("HELION_TILECONN_DB_ROOT") else {
            println!(
                "counter_flow_emits_bitstream_on_xczu3eg: skipping, \
                 HELION_TILECONN_DB_ROOT is unset"
            );
            return;
        };
        if !std::path::Path::new(&db_root).is_dir() {
            println!(
                "counter_flow_emits_bitstream_on_xczu3eg: skipping, \
                 HELION_TILECONN_DB_ROOT {db_root} is absent"
            );
            return;
        }
        let Ok(part) = std::env::var("HELION_TILECONN_PART") else {
            println!(
                "counter_flow_emits_bitstream_on_xczu3eg: skipping, \
                 HELION_TILECONN_PART is unset (zynqusp holds two part dirs)"
            );
            return;
        };
        assert_eq!(
            part, "xczu3eg-sbva484-1-e",
            "HELION_TILECONN_PART must be the sbva484 part for this test, got {part}"
        );
        let dir = std::env::temp_dir().join("helion-us09-flow-counter-xczu3eg");
        let _ = std::fs::create_dir_all(&dir);
        let out_bit = dir.join("counter.xczu3eg.bit");
        let out_fasm = dir.join("counter.xczu3eg.fasm");
        let _ = std::fs::remove_file(&out_bit);
        let _ = std::fs::remove_file(&out_fasm);
        run_impl(
            &workspace_example("counter.sv"),
            "xczu3eg",
            0,
            Some(&out_bit.to_string_lossy()),
            Some(&out_fasm.to_string_lossy()),
        )
        .unwrap();
        let bytes = std::fs::read(&out_bit).unwrap();
        assert!(!bytes.is_empty(), "emitted bitstream is empty");
        assert!(
            bytes.windows(4).any(|w| w == [0x04, 0xa4, 0x20, 0x93]),
            "emitted bitstream carries no 0x04a42093 IDCODE word"
        );
        assert_eq!(count_fdri_frames(&bytes), 14_952);
        let fasm = std::fs::read_to_string(&out_fasm).unwrap();
        // Stable lines only (see the doc comment above): LUT INITs,
        // FF INIT/SRVAL, FFMUX, FF control.
        let stable: [&str; 22] = [
            "SLICE_X1Y0.A6LUT.INIT[63:0] = 0x6aaa6aaa6aaa6aaa",
            "SLICE_X1Y0.B6LUT.INIT[63:0] = 0x5555555555555555",
            "SLICE_X1Y0.C6LUT.INIT[63:0] = 0x6666666666666666",
            "SLICE_X1Y0.D6LUT.INIT[63:0] = 0x7878787878787878",
            "SLICE_X1Y0.AFF.INIT.V0",
            "SLICE_X1Y0.AFF.SRVAL.V0",
            "SLICE_X1Y0.AFF2.INIT.V0",
            "SLICE_X1Y0.AFF2.SRVAL.V0",
            "SLICE_X1Y0.BFF.INIT.V0",
            "SLICE_X1Y0.BFF.SRVAL.V0",
            "SLICE_X1Y0.DFF.INIT.V0",
            "SLICE_X1Y0.DFF.SRVAL.V0",
            "SLICE_X1Y0.FFMUXA1.D6",
            "SLICE_X1Y0.FFMUXA2.BYP",
            "SLICE_X1Y0.FFMUXB1.BYP",
            "SLICE_X1Y0.FFMUXD1.D6",
            "SLICE_X1Y0.ABCDFF.CEUSED.V0",
            "SLICE_X1Y0.ABCDFF2.CEUSED.V0",
            "SLICE_X1Y0.ABCDFF.SRUSED.V0",
            "SLICE_X1Y0.ABCDFF2.SRUSED.V0",
            "SLICE_X1Y0.ABCDFF.CLKINV.V0",
            "SLICE_X1Y0.ABCDFF.SRINV.V0",
        ];
        for line in stable {
            assert!(
                fasm.lines().any(|l| l == line),
                "flow FASM is missing line: {line}"
            );
        }
        let mut pips = 0usize;
        for line in fasm.lines() {
            if line.contains(".PIP.") {
                pips += 1;
                assert!(
                    line.starts_with("INT_X1Y0."),
                    "PIP on an INT tile other than INT_X1Y0: {line}"
                );
            }
        }
        assert!(pips > 0, "flow FASM carries no PIPs");
        // H-1 band: the counter carries no LUTRAM, so the mask is
        // all false, as in the blinky flow test.
        let frames = helion_ultrascale::bitgen::parse_frames(&bytes).unwrap();
        assert_eq!(frames.len(), 14_952);
        let mask = [false; helion_ultrascale::bitgen::FRAME_BITS];
        let mut exact = 0usize;
        for (far, words) in &frames {
            match check_stored_band(*far, words, &mask) {
                BandCheck::Exact => exact += 1,
                BandCheck::QResidual => panic!("unexpected Q-residual band at far=0x{far:08x}"),
                BandCheck::Mismatch { stored, expected } => panic!(
                    "band mismatch far=0x{far:08x} stored=0x{stored:012x} \
                     expected=0x{expected:012x}"
                ),
            }
        }
        assert_eq!(exact, 14_952, "band Exact frame count");
        // Standalone decoder round-trip over the stable lines.
        let decoded =
            helion_ultrascale::bitstream_decode::decode_bitstream(&bytes, &db_root).unwrap();
        for line in stable {
            assert!(
                decoded.fasm.iter().any(|l| l == line),
                "decoded FASM is missing flow line: {line}"
            );
        }
    }

    #[test]
    fn xcvu3p_still_fails_loud_with_provisional_message() {
        let data_dir = ultrascale_data_dir();
        let err = load_ultra_device("xcvu3p", &data_dir).unwrap_err();
        assert!(err.contains("xcvu3p"), "got {err}");
        assert!(err.contains("provisional"), "got {err}");
        // Same through the emit path, which is the gate the flow hits.
        let err = emit_ultra_bitstream(
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555\n",
            "xcvu3p",
            &data_dir,
        )
        .unwrap_err();
        assert!(err.contains("xcvu3p"), "got {err}");
        assert!(err.contains("provisional"), "got {err}");
    }

    #[test]
    fn unknown_part_fails_loud_at_bitgen_load() {
        let data_dir = ultrascale_data_dir();
        let err = load_ultra_device("xc7a35t", &data_dir).unwrap_err();
        assert!(err.contains("xc7a35t"), "got {err}");
        assert!(err.contains("has no emitter device yet"), "got {err}");
    }
}
