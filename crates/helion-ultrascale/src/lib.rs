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
pub mod bel;
pub mod bitgen;
pub mod bitloc;
pub mod device;
pub mod device_file;
pub mod fabric;
pub mod grid;
pub mod int_pip;
pub mod legality;
pub mod pack;
pub mod place;
pub mod router;
pub mod segbits;
pub mod sta;
pub mod timing_db;

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
pub use device::{UltraScaleDevice, Xcku3pSpec, Xcvu3pSpec};
pub use device_file::{
    ArchSummary, BitLocator, BramResolution, DeviceFile, DeviceFileError, Far, FarBlockType,
    NUM_18K_BRAM_PER_COLUMN, ProvisionalNote,
};
pub use fabric::{
    EmptyTimingDb, EmptyWireDb, FabricWithPlaceholders, TimingDb, UltraScaleFabric, UltraSite,
    WireDb, WireId, WireInfo,
};
pub use int_pip::{
    EmittedPip, IntPip, IntPipError, MissingPip, PipResolution, emit_pips, emitted_to_fasm,
    missing_pip_report, resolve_pips, verify_zero_bits,
};
pub use legality::{check_legality, LegalityViolation};
pub use pack::{
    pack_ultra, ControlSetLimits, PackedUltraSlice, SliceKind, UltraBel, UltraPacked,
};
pub use place::{place_ultra, seeded_order, PlacedUltraSlice, UltraGrid, UltraPlaceOpts, UltraPlaced};
pub use router::{route_ultra, RoutableNet, RouteError, RouteErrorKind, UltraRouted, UltraRouteOpts};
pub use segbits::{BitRef, FileSegbitsDb, SegbitsDb, SegbitsLoadError};
pub use sta::{sta_routed, StaNetlist, TimingReport};
pub use timing_db::{preliminary_xcvu3p, DelaySource, SourcedDelay, UltraScaleTimingDb};
