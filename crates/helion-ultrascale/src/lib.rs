//! helion-ultrascale: Virtex UltraScale+ device model.
//!
//! Owns everything UltraScale-specific for the xcvu3p target. Nothing in this
//! crate leaks into shared crates; consumers (pack-place, router, timing)
//! code against the [`fabric::UltraScaleFabric`] trait.
//!
//! Sources:
//! - UG574 (UltraScale Architecture Configurable Logic Block): CLB structure
//! - UG974 (UltraScale Libraries Guide): CARRY8 primitive, CARRY_TYPE
//! - AMD XMP103 (UltraScale+ Product Selection Guide): xcvu3p resource counts
//! - BitFiltrator (EPFL, FPL 2022): tile type vocabulary (CLEL_L/R, CLEM/R,
//!   DSP, BRAM), confirmed against Vivado tile naming

pub mod band_code;
pub mod bel;
pub mod bitgen;
pub mod bitgen_device;
pub mod bitstream_decode;
pub mod bitstream_packets;
pub mod bitloc;
pub mod clk;
pub mod device;
pub mod device_file;
pub mod device_grid;
pub mod dsp;
pub mod fabric;
pub mod fasm_gen;
pub mod grid;
pub mod int_pip;
pub mod legality;
pub mod multiseed;
pub mod pack;
pub mod place;
pub mod route_to_pip;
pub mod router;
pub mod segbits;
pub mod sta;
pub mod synthmap;
pub mod tileconn;
pub mod timing_db;
pub mod uram;
pub mod wire_db;

pub use bel::{Bel, BelKind, ClbBelInventory, TileId, TileKind};
pub use bitgen::{
    BitgenDevice, BitgenError, DUMMY_WORD, FRAME_WORDS, FasmEntry, FasmValue, FrameWords,
    SYNC_WORD, StubDevice, StubSegbitsDb, far_decode, far_encode,
};
pub use bitloc::{
    BRAM_MEM_BITS, BRAM_PARITY_BITS, BitLoc, BitLocError, Block, BramHalf, BramHalfContent,
    BramTileUse, ClbTileType, FRAME_BITS, LUT_INIT_BITS, LutMode, NUM_BRAM18_PER_COLUMN,
    NUM_CLB_PER_COLUMN, PlacedBit,
};
pub use clk::{
    BUFGCE_INVERSION_PIPS, BufceLeafConfig, BufgceHdioConfig, ClkError, ClkResolution,
    MissingClkFeature, ResolvedClkFeature, bufce_row_site_config, bufgce_div_site_config,
    bufgce_hdio_ce_blocked, bufgce_site_config, bufgctrl_site_config, clk_features_to_fasm,
    cmt_hdistr_used_feature, cmt_hroute_used_feature, mmcm_site_config, pll_site_config,
    require_no_clock_manager, resolve_bufce_leaf, resolve_bufgce_hdio, resolve_clk_feature,
    resolve_clk_features, resolve_cmt_feature,
};
pub use device::{UltraScaleDevice, Xcku3pSpec, Xcvu3pSpec, Xczu3egSpec};
pub use dsp::{DSP48E2_ATTRIBUTES, is_dsp48e2_entry};
pub use device_file::{
    ArchSummary, BitLocator, BramResolution, DeviceFile, DeviceFileError, Far, FarBlockType,
    IntColumnTable, IntfColumnTable, NUM_18K_BRAM_PER_COLUMN, ProvisionalNote,
};
pub use device_grid::{DeviceColumn, DeviceFileGrid};
pub use fabric::{
    EmptyTimingDb, EmptyWireDb, FabricWithPlaceholders, TimingDb, UltraScaleFabric, UltraSite,
    WireDb, WireId, WireInfo,
};
pub use fasm_gen::{
    emit_ff_fasm, emit_ffmux_fasm, emit_lut_fasm, ff_init_fasm_line, ff_srval_fasm_line,
    ffmux_fasm_line, lut_init_fasm_line, FfAttr, FfMuxSel,
};
pub use int_pip::{
    BidirDir, EmittedPip, IntPip, IntPipError, MissingPip, PipResolution, emit_pips,
    emitted_to_fasm, missing_pip_report, resolve_pips, verify_zero_bits,
};
pub use legality::{LegalityViolation, check_legality};
pub use multiseed::{
    MultiseedSummary, SeedResult, best_seed_line, dedup_results, escape_json_value,
    parse_results_text, read_completed_seeds, seed_range, seeds_to_run, select_best_seed,
    summarize_multiseed,
};
pub use pack::{ControlSetLimits, PackedUltraSlice, SliceKind, UltraBel, UltraPacked, pack_ultra};
pub use place::{
    PlacedUltraSlice, UltraGrid, UltraPlaceOpts, UltraPlaced, place_ultra, render_placed_netlist,
    seeded_order,
};
pub use route_to_pip::{
    RouteToPipError, TranslatedRoute, translate_route, translate_route_with, translate_routes,
    translate_routes_with, translated_to_fasm,
};
pub use router::{
    RoutableNet, RouteError, RouteErrorKind, UltraRouteOpts, UltraRouted, route_ultra,
};
pub use segbits::{BitRef, FileSegbitsDb, SegbitsDb, SegbitsLoadError};
pub use sta::{StaNetlist, TimingReport, sta_routed};
pub use synthmap::{
    AND6_INIT, Carry8Mapping, FfControlGroup, MappedCell, SourceCell, SourceKind, Unmapped,
    emit_ff_control_fasm, map_source_cell, pair_carry4_chains,
};
pub use timing_db::{
    documented_xcku3p, documented_xcvu3p, sta_db_for_part, DelaySource, SourcedDelay,
    UltraScaleTimingDb, preliminary_xcku3p, preliminary_xcvu3p, preliminary_xczu3eg,
};
pub use tileconn::{PinCoverage, PinWire, SiteTile, TileconnDb, TileconnError, TileconnWireDb};
pub use wire_db::{IntTile, IntTileWireDb, WireDbError};
