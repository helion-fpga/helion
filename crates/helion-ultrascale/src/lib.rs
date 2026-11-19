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

pub mod bel;
pub mod bitgen;
pub mod bitgen_device;
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
pub mod pack;
pub mod place;
pub mod router;
pub mod segbits;
pub mod sta;
pub mod timing_db;
pub mod uram;

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
pub use device::{UltraScaleDevice, Xcku3pSpec, Xcvu3pSpec};
pub use dsp::{DSP48E2_ATTRIBUTES, is_dsp48e2_entry};
pub use device_file::{
    ArchSummary, BitLocator, BramResolution, DeviceFile, DeviceFileError, Far, FarBlockType,
    NUM_18K_BRAM_PER_COLUMN, ProvisionalNote,
};
pub use device_grid::{DeviceColumn, DeviceFileGrid};
pub use fabric::{
    EmptyTimingDb, EmptyWireDb, FabricWithPlaceholders, TimingDb, UltraScaleFabric, UltraSite,
    WireDb, WireId, WireInfo,
};
pub use fasm_gen::{FfAttr, emit_ff_fasm, ff_init_fasm_line, ff_srval_fasm_line};
pub use int_pip::{
    EmittedPip, IntPip, IntPipError, MissingPip, PipResolution, emit_pips, emitted_to_fasm,
    missing_pip_report, resolve_pips, verify_zero_bits,
};
pub use legality::{LegalityViolation, check_legality};
pub use pack::{ControlSetLimits, PackedUltraSlice, SliceKind, UltraBel, UltraPacked, pack_ultra};
pub use place::{
    PlacedUltraSlice, UltraGrid, UltraPlaceOpts, UltraPlaced, place_ultra, render_placed_netlist,
    seeded_order,
};
pub use router::{
    RoutableNet, RouteError, RouteErrorKind, UltraRouteOpts, UltraRouted, route_ultra,
};
pub use segbits::{BitRef, FileSegbitsDb, SegbitsDb, SegbitsLoadError};
pub use sta::{StaNetlist, TimingReport, sta_routed};
pub use timing_db::{
    documented_xcku3p, documented_xcvu3p, sta_db_for_part, DelaySource, SourcedDelay,
    UltraScaleTimingDb, preliminary_xcku3p, preliminary_xcvu3p,
};
