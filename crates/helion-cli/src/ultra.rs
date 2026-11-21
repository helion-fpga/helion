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
//!   INT_X{x}Y{y} until the xcku3p tilegrid lands). It fails loud with
//!   NoPath naming the first BEL pin with no wire: no public source
//!   maps BEL pins to INT wires (US-01 data gap), so no placed design
//!   routes yet.
//! - ROUTE-TO-PIP translates routed nets to INT PIPs via the WireDb
//!   naming seam (CTO ruling 2026-11-07, option A); unnamed wires and
//!   non-edges fail loud. Multi-sink nets route as a flattened union
//!   of per-sink paths, which is not an ordered path, so translation
//!   fails loud on those until the router emits trees.
//! - BITGEN is wired (emit on the real xcku3p device file plus the
//!   vendored segbits DBs, with a parse-back set-bit check), but is
//!   unreachable until a design routes. CARRY8 designs fail loud at
//!   BITGEN: the flow has no CARRY8 FASM generator yet.
//! - FF FASM (complete, INIT/SRVAL default V0), FF control FASM, and
//!   LUT INIT FASM are generated after legality.
//! - Clock nets are phase 2 for the router and IOBs have no placed
//!   site in this flow yet; nets touching them are skipped loudly at
//!   ROUTE, not routed.

use helion_ir::{CellKind, Design, intern, resolve};
use helion_ultrascale::bitgen::{
    BitgenError, emit_bitstream, emit_frames, parse_bitstream,
};
use helion_ultrascale::device_file::DeviceFile;
use helion_ultrascale::{
    best_seed_line, check_legality, dedup_results, emit_ff_control_fasm, emit_ff_fasm,
    emit_lut_fasm, escape_json_value, map_source_cell, pack_ultra, parse_results_text, place_ultra,
    route_ultra, seed_range, seeds_to_run, sta_db_for_part, sta_routed, summarize_multiseed,
    translated_to_fasm, ControlSetLimits, FabricWithPlaceholders, FileSegbitsDb, IntTileWireDb,
    RoutableNet, SeedResult, SourceCell, SourceKind, StaNetlist, TimingReport, TranslatedRoute,
    UltraPlaceOpts, UltraRouteOpts, UltraScaleDevice, UltraScaleFabric, WireDb,
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
/// majors, INT base FARs by the provisional rule, RAMB18 from the
/// BRAM content columns, full config order). Only `xcku3p` is wired:
/// `xcvu3p.json` is provisional (computed frame total 73,035 vs
/// UG570 Table 1's 71,820) until that gap is resolved by a CTO
/// decision; any other part fails loud here. The flow emits no HPIO
/// lines (no IO placement), so no HPIO bridge is needed here.
pub fn load_ultra_device(part: &str, data_dir: &Path) -> Result<DeviceFile, String> {
    match part {
        "xcku3p" => DeviceFile::from_path(&data_dir.join("xcku3p.json")).map_err(|e| {
            format!(
                "ULTRA BITGEN: cannot load device file {}: {e}",
                data_dir.join("xcku3p.json").display()
            )
        }),
        other => Err(format!(
            "ULTRA BITGEN: part {other} has no emitter device yet: only xcku3p is wired; \
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
    let packed = pack_ultra(&cells.luts, &cells.ffs, &cells.carry_chains, &limits)
        .map_err(|e| format!("ULTRA PACK: {e}"))?;
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

    // Preliminary STA on the placed netlist (Manhattan estimates, ballpark
    // delays; NOT signoff timing). The DB is selected by part (F3 fix:
    // xcku3p no longer silently runs on the xcvu3p DB) and the report is
    // labeled estimate=1 until a real delay DB exists.
    let timing = sta_on_placed(&design, &packed, &placed, part)?;
    println!("{}", timing.hang_diag_line(0));

    // ROUTE: build the real INT-tile routing graph for the placed
    // slices (vendored INT tile JSON) and route on it. The graph is
    // real DB data; what it does NOT have is the BEL-pin to INT-wire
    // map (US-01, no public source), so route_ultra fails loud naming
    // the first pin with no wire. That failure is the named data gap,
    // not a code gap. The graph is installed into the fabric (the
    // EmptyWireDb placeholder is gone); the flow keeps its own Arc
    // handle for the translator's bidirectional-PIP resolver.
    let data_dir = ultrascale_data_dir();
    let tile_json = std::fs::read_to_string(data_dir.join("tile_type_INT.json"))
        .map_err(|e| format!("ULTRA ROUTE: cannot read INT tile JSON: {e}"))?;
    let wire_db = Arc::new(
        IntTileWireDb::from_placed_slices(&tile_json, &placed)
            .map_err(|e| format!("ULTRA ROUTE: {e}"))?,
    );
    println!(
        "hang_diag ultra wiredb tiles={} wires={}",
        wire_db.tile_count(),
        wire_db.wire_count()
    );
    let fabric_wires: Arc<dyn WireDb> = wire_db.clone();
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
    // Multi-sink nets route as a flattened union of per-sink paths,
    // which is not an ordered path: translation fails loud on those
    // (NotConnected) until the router emits trees the translator can
    // follow.
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

/// Build routable nets from the placed design. Only reached once a real
/// WireDb exists. Nets touching IOBs or clock nets are skipped loudly
/// (one hang_diag line each, naming the net and the gap): IOBs have
/// no placed site in this flow yet, and clock nets are phase 2 for the
/// router. A skipped net is never routed, so the emitted bitstream
/// carries no configuration for it; that is the honest state until IO
/// placement and clock routing land. Carry4 cells fail loud: carry
/// routing is not implemented in this flow, and silently dropping
/// carry nets would misconfigure the design.
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
    let packed = pack_ultra(&cells.luts, &cells.ffs, &cells.carry_chains, &limits)
        .map_err(|e| format!("ULTRA PACK: {e}"))?;
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
        // The q net (LUT O to FF D) routes; the led net (FF Q to the
        // top-level port) and the clk net (top-level input to FF CLK)
        // drop out because their port endpoints have no placed site.
        // Skipping is printed, not silent, and never errors.
        let (design, packed, placed) = one_slice_design();
        let nets = build_routable_nets(&design, &packed, &placed).unwrap();
        assert_eq!(nets.len(), 1, "only the cell-driven net routes");
        assert_eq!(nets[0].name, "q");
        assert_eq!(nets[0].driver.1, "A6LUT");
        assert_eq!(nets[0].driver.2, "O6");
        assert_eq!(nets[0].sinks.len(), 1);
        assert_eq!(nets[0].sinks[0].1, "AQ");
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
    fn unknown_part_fails_loud_at_bitgen_load() {
        let data_dir = ultrascale_data_dir();
        let err = emit_ultra_bitstream(
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555\n",
            "xcvu3p",
            &data_dir,
        )
        .unwrap_err();
        assert!(err.contains("xcvu3p"), "got {err}");
    }
}
