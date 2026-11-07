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
pub mod bitloc;
pub mod device;
pub mod device_file;
pub mod fabric;
pub mod int_pip;
pub mod segbits;

pub use bel::{Bel, BelKind, ClbBelInventory, TileId, TileKind};
pub use bitgen::{
    BitRef, BitgenDevice, BitgenError, DUMMY_WORD, FRAME_BITS, FRAME_WORDS, FasmEntry, FasmValue,
    FrameWords, SYNC_WORD, SegbitsDb, StubDevice, StubSegbitsDb, far_decode, far_encode,
};
pub use bitloc::{
    BRAM_MEM_BITS, BRAM_PARITY_BITS, BitLoc, BitLocError, Block, BramHalf, BramHalfContent,
    BramTileUse, ClbTileType, FRAME_BITS, LUT_INIT_BITS, LutMode, NUM_BRAM18_PER_COLUMN,
    NUM_CLB_PER_COLUMN, PlacedBit,
};
pub use device::{UltraScaleDevice, Xcvu3pSpec};
pub use device_file::{
    ArchSummary, BitLocator, BramResolution, DeviceFile, DeviceFileError, Far, FarBlockType,
    NUM_18K_BRAM_PER_COLUMN, ProvisionalNote,
};
pub use fabric::{
    EmptyTimingDb, EmptyWireDb, FabricWithPlaceholders, TimingDb, UltraScaleFabric, WireDb,
};
pub use int_pip::{
    EmittedPip, IntPip, IntPipError, MissingPip, PipResolution, emit_pips, emitted_to_fasm,
    missing_pip_report, resolve_pips, verify_zero_bits,
};
pub use segbits::{BitRef, FileSegbitsDb, SegbitsDb, SegbitsLoadError};
