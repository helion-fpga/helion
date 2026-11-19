//! Synthesis mapping: generic synth cells to UltraScale primitives (GAP-F1).
//!
//! The seam between the synthesis front end (which emits backend-neutral
//! [`SourceCell`]s: LUT6, HFF, Carry4, IOBs, clock buffers, and the 7-series
//! flavoured DSP/BRAM cells) and the UltraScale packer (which consumes
//! LUT6, FDRE, CARRY8, DSP48E2, RAMB36E2/RAMB18E2, URAM288).
//!
//! Rules enforced here, per the CTO spec:
//! - A cell with no mapping fails loud with the missing primitive and its
//!   unblocker. It is never silently dropped and never remapped to a
//!   different primitive with different semantics.
//! - A LUT6 carrying SRL or distributed-RAM attributes is not a plain
//!   LUT6: mapping it as one would miscompile. It fails loud (LUTRAM
//!   emission is BLOCKED per the G4 ruling; see `bitloc::LutramBlocked`).
//! - Never add a mapping primarily to make a test pass. Every mapping
//!   below is justified by the cited UG reference or DB entry.
//!
//! Primitive inventory: every UltraScale primitive this flow needs,
//! its source, and its mapping status. BLOCKED means the mapping fails
//! loud with the named unblocker; nothing here is silently dropped or
//! remapped to a different primitive.
//!
//! - LUT6: Lut6, plain. Mapped, INIT verbatim.
//! - FDRE: Hff. Mapped, CE/SR presence; control FASM in
//!   [`emit_ff_control_fasm`].
//! - CARRY8: Carry4. Mapped, 2:1 pairing in [`pair_carry4_chains`];
//!   the pack wiring is the pack worker's interface (PackedUltraSlice
//!   carries one carry cell per slice today, so pairing is not yet
//!   consumed by pack_ultra).
//! - SRL16E / SRLC32E: Lut6 with SRL_TYPE/SHREG_EXTRACT. BLOCKED (G4):
//!   no LUTRAM table or Vivado logic-location data.
//! - RAM64X1S / RAM128X1S / RAM256X1S / RAM32M16: Lut6 with MEMORY=1.
//!   BLOCKED (G4): distributed RAM, SLICEM-only.
//! - RAMB36E2: native Ramb36e2. Mapped (width validation); INIT/INITP
//!   encoding is the BRAM worker's scope.
//! - RAMB18E2: native Ramb18e2. Mapped (width validation); same BRAM
//!   scope note.
//! - URAM288: native Uram288. Mapped (fixed 4096x72 validation);
//!   content encoding BLOCKED (GAP-B4, URAM worker).
//! - DSP48E2: native Dsp48e2. Mapped (control-value validation); the
//!   46 primitive attributes have no public encoding, BLOCKED (GAP-B3,
//!   DSP worker).
//! - IBUF / OBUF: IobIn / IobOut. Outside pack (IOB placement).
//! - BUFGCE / BUFGCTRL: ClockBuf. Outside pack (clocking).
//! - MMCME4 / PLLE4: no source. Not sourced: no front-end inference
//!   emits them; fail loud if one ever arrives.
//!
//! 7-series sources that must NOT remap: Dsp48e1 and Mac27 (no
//! semantics-preserving DSP48E2 remap), Bram18e1 and Bram18 (no
//! RAMB18E2 remap), Ila (no UltraScale ILA encoding), BlackBox
//! (unelaborated IP). All fail loud in [`map_source_cell`].
//!
//! Front-end rule (Nov 19 agreement): when UltraScale synth mapping
//! needs front-end inference, extend fm/g3-dsp-v2 (v2 front end,
//! `*` to DSP48E1) to DSP48E2 instead of writing a second inference.
//! The native kinds below are constructed by that front-end
//! inference, not by extract_cells (`crates/helion-cli/src/ultra.rs`),
//! which only translates the 12 backend-neutral CellKind variants.
//!
//! Sources:
//! - UG574 (CLB: 8 LUT6, 16 FF, 1 CARRY8 per slice; control sets).
//! - UG974 (CARRY8, CARRY_TYPE SINGLE_CY8 / DUAL_CY4; FDRE CE/SR).
//! - UG578 (DSP48E2), UG573 (RAMB36E2/RAMB18E2, URAM288).
//! - Vendored prjuray-db segbits (CLEL_L/CLEM): `CARRY8.CARRY_TYPE.*`,
//!   `ABCDFF/EFGHFF[2].CEUSED/SRUSED/CLKINV/SRINV.V0/V1`,
//!   `ABCDFF/EFGHFF.SYNC.SYNC/ASYNC`.
//!
//! No em dashes anywhere.

use crate::pack::PackedUltraSlice;
use crate::place::UltraPlaced;
use std::collections::HashMap;

/// Backend-neutral source cell as produced by synthesis.
///
/// The CLI (`crates/helion-cli/src/ultra.rs`) builds this from
/// `helion_ir::CellKind`. This crate does not depend on `helion-ir` by
/// design, so the struct is the documented interface between the mapper
/// (which owns `CellKind`) and the UltraScale packer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCell {
    pub name: String,
    pub kind: SourceKind,
}

/// Source cell kinds. Only the fields the mapping needs are carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// Plain 6-input LUT. `init[addr]` is the output for inputs
    /// `{I5..I0}` as addr. `is_memory` is the MEMORY attribute
    /// (distributed RAM); `is_srl` is SRL_TYPE/SHREG_EXTRACT.
    Lut6 {
        init: u64,
        is_memory: bool,
        is_srl: bool,
    },
    /// Flip-flop. `has_ce` / `has_sr` record whether a CE / SR pin net
    /// is connected. Inversion is not represented in the IR today, so
    /// CLKINV/SRINV always emit V0 (see `emit_ff_control_fasm`).
    Ff { has_ce: bool, has_sr: bool },
    /// 7-series 4-bit carry cell. Maps to one CARRY8 nibble; see
    /// [`pair_carry4_chains`].
    Carry4,
    /// Top-level input port buffer.
    IobIn,
    /// Top-level output port buffer.
    IobOut,
    /// Global clock buffer.
    ClockBuf,
    /// 7-series DSP48E1. NOT a DSP48E2 source: different widths
    /// (25x18 vs 27x18) and opmode/alumode encodings.
    Dsp48e1 { opmode: u8, alumode: u8 },
    /// Deprecated MAC27 with no explicit pin interface.
    Mac27,
    /// Inserted ILA core.
    Ila,
    /// Deprecated BRAM18 with no explicit pin interface.
    Bram18,
    /// 7-series RAMB18E1. NOT a RAMB18E2 source: different pinout and
    /// INIT/INITP widths.
    Bram18e1 {
        read_width_a: u8,
        read_width_b: u8,
        write_width_a: u8,
        write_width_b: u8,
    },
    /// Hierarchical black box (unelaborated IP / DFX partition).
    BlackBox { module: String },
    /// Native UltraScale DSP48E2 (UG578). Produced by front-end DSP
    /// inference (fm/g3-dsp-v2 extended to DSP48E2), never by
    /// remapping Dsp48e1/Mac27.
    ///
    /// `opmode` (9 bits), `alumode` (4 bits), `inmode` (5 bits) are
    /// the dynamic control-port values, verified against the vendored
    /// site_type_DSP48E2.json pins (A 30, B 18, C 48, P 48, OPMODE 9,
    /// ALUMODE 4, INMODE 5). They are routing, not config bits: the
    /// 46 primitive attributes (AREG, BREG, ...) have no public
    /// encoding and their emission stays BLOCKED (GAP-B3, DSP worker).
    Dsp48e2 {
        opmode: u16,
        alumode: u8,
        inmode: u8,
    },
    /// Native UltraScale RAMB36E2 (UG573). Read/write widths per port
    /// must each be in {1, 2, 4, 9, 18, 36}. One tile is one RAMB36E2
    /// OR two RAMB18E2 halves, never both (G3). INIT/INITP encoding is
    /// the BRAM worker's scope.
    Ramb36e2 {
        read_width_a: u8,
        read_width_b: u8,
        write_width_a: u8,
        write_width_b: u8,
    },
    /// Native UltraScale RAMB18E2 (UG573). Widths per port in
    /// {1, 2, 4, 9, 18}. Lower/upper half of a RAMB36 tile (L/U).
    Ramb18e2 {
        read_width_a: u8,
        read_width_b: u8,
        write_width_a: u8,
        write_width_b: u8,
    },
    /// Native UltraScale URAM288 (UG573). Fixed geometry 4096 x 72.
    /// Content encoding is BLOCKED (GAP-B4, URAM worker).
    Uram288 { data_width: u8, depth: u32 },
}

/// The UltraScale primitive a source cell maps to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MappedCell {
    /// LUT6, INIT preserved verbatim.
    Lut6 { init: u64 },
    /// FDRE flip-flop with clock enable and synchronous reset.
    FdRe { has_ce: bool, has_sr: bool },
    /// Carry4: maps to one CARRY8 nibble. Nibble assignment and
    /// CARRY_TYPE live in [`Carry8Mapping`], not here, because they
    /// depend on the chain, not the single cell.
    Carry4,
    /// IBUF: handled outside pack (IOB placement).
    IobIn,
    /// OBUF: handled outside pack (IOB placement).
    IobOut,
    /// BUFGCE/BUFGCTRL: handled outside pack (clocking).
    ClockBuf,
    /// DSP48E2 with dynamic control values (config attributes blocked).
    Dsp48e2 {
        opmode: u16,
        alumode: u8,
        inmode: u8,
    },
    /// RAMB36E2 with validated port widths.
    Ramb36e2 {
        read_width_a: u8,
        read_width_b: u8,
        write_width_a: u8,
        write_width_b: u8,
    },
    /// RAMB18E2 with validated port widths.
    Ramb18e2 {
        read_width_a: u8,
        read_width_b: u8,
        write_width_a: u8,
        write_width_b: u8,
    },
    /// URAM288, fixed 4096 x 72.
    Uram288 { data_width: u8, depth: u32 },
}

/// Why a source cell has no UltraScale mapping, with its unblocker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unmapped {
    /// Source cell name.
    pub cell: String,
    /// The UltraScale primitive that would be needed.
    pub needs: &'static str,
    /// Exact missing piece and who owns it.
    pub unblocker: std::borrow::Cow<'static, str>,
}

impl std::fmt::Display for Unmapped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ULTRA SYNTHMAP: cell {} has no UltraScale mapping (needs {}): {}",
            self.cell, self.needs, self.unblocker
        )
    }
}

impl std::error::Error for Unmapped {}

/// True for a legal RAMB port width. RAMB36E2 allows 36; RAMB18E2 does
/// not. Same-port read/write equality is NOT enforced here: UG573
/// lists the per-port width sets, and whether a port's read and write
/// widths must match is the BRAM worker's emission-time check, not a
/// rule this mapper invents.
fn valid_bram_width(width: u8, allow_36: bool) -> bool {
    matches!(width, 1 | 2 | 4 | 9 | 18) || (allow_36 && width == 36)
}

/// Map one source cell to its UltraScale primitive.
///
/// Fails loud (`Err(Unmapped)`) for every cell with no mapping. Carry4
/// maps per cell to [`MappedCell::Carry4`]; the nibble assignment is a
/// chain-level decision in [`pair_carry4_chains`].
pub fn map_source_cell(cell: &SourceCell) -> Result<MappedCell, Unmapped> {
    let name = cell.name.clone();
    match &cell.kind {
        SourceKind::Lut6 {
            init,
            is_memory,
            is_srl,
        } => {
            if *is_srl {
                return Err(Unmapped {
                    cell: name,
                    needs: "SRL16E/SRLC32E (SLICEM shift register)",
                    unblocker: std::borrow::Cow::Borrowed(
                        "LUTRAM/SRL emission is BLOCKED (G4 ruling): no LUTRAM \
                        table or Vivado logic-location data exists; the LUT6 must not \
                        be packed as a plain LUT6",
                    ),
                });
            }
            if *is_memory {
                return Err(Unmapped {
                    cell: name,
                    needs: "distributed RAM (SLICEM LUTRAM)",
                    unblocker: std::borrow::Cow::Borrowed(
                        "LUTRAM emission is BLOCKED (G4 ruling): no LUTRAM \
                        table or Vivado logic-location data exists; the LUT6 must not \
                        be packed as a plain LUT6",
                    ),
                });
            }
            Ok(MappedCell::Lut6 { init: *init })
        }
        SourceKind::Ff { has_ce, has_sr } => Ok(MappedCell::FdRe {
            has_ce: *has_ce,
            has_sr: *has_sr,
        }),
        SourceKind::Carry4 => Ok(MappedCell::Carry4),
        SourceKind::IobIn => Ok(MappedCell::IobIn),
        SourceKind::IobOut => Ok(MappedCell::IobOut),
        SourceKind::ClockBuf => Ok(MappedCell::ClockBuf),
        SourceKind::Dsp48e1 { .. } => Err(Unmapped {
            cell: name,
            needs: "DSP48E2",
            unblocker: std::borrow::Cow::Borrowed(
                "no semantics-preserving Dsp48e1 to DSP48E2 remap exists \
                (25x18 vs 27x18, different opmode/alumode); DSP inference must \
                target DSP48E2 directly (synth/QoR crew) and DSP48E2 config \
                encoding is BLOCKED on the DSP block worker (segbits-vendor)",
            ),
        }),
        SourceKind::Mac27 => Err(Unmapped {
            cell: name,
            needs: "DSP48E2",
            unblocker: std::borrow::Cow::Borrowed(
                "Mac27 is deprecated with no explicit pin interface; DSP \
                inference must target DSP48E2 directly (synth/QoR crew)",
            ),
        }),
        SourceKind::Ila => Err(Unmapped {
            cell: name,
            needs: "ILA core (UltraScale)",
            unblocker: std::borrow::Cow::Borrowed(
                "no ILA bitgen encoding exists in this flow; the ILA \
                insertion pass must be disabled or given an UltraScale target",
            ),
        }),
        SourceKind::Bram18 => Err(Unmapped {
            cell: name,
            needs: "RAMB18E2",
            unblocker: std::borrow::Cow::Borrowed(
                "Bram18 is deprecated with no explicit pin interface; BRAM \
                inference must target RAMB18E2/RAMB36E2 directly (synth/QoR crew)",
            ),
        }),
        SourceKind::Bram18e1 { .. } => Err(Unmapped {
            cell: name,
            needs: "RAMB18E2",
            unblocker: std::borrow::Cow::Borrowed(
                "no semantics-preserving Bram18e1 to RAMB18E2 remap exists \
                (7-series pinout and INIT/INITP widths differ); BRAM inference \
                must target RAMB18E2/RAMB36E2 directly and RAMB INIT encoding is \
                BLOCKED on the BRAM block worker",
            ),
        }),
        SourceKind::BlackBox { module } => Err(Unmapped {
            cell: name.clone(),
            needs: "elaborated UltraScale netlist",
            unblocker: std::borrow::Cow::Owned(format!(
                "black-box instance {name} ({module}) is not supported for UltraScale+; \
                 the IP must be elaborated before pack"
            )),
        }),
        SourceKind::Dsp48e2 {
            opmode,
            alumode,
            inmode,
        } => {
            if *opmode > 0x1FF {
                return Err(Unmapped {
                    cell: name.clone(),
                    needs: "DSP48E2",
                    unblocker: std::borrow::Cow::Owned(format!(
                        "malformed DSP48E2 source cell {name}: opmode {opmode} out of range 0..=511 (9 bits)"
                    )),
                });
            }
            if *alumode > 0xF {
                return Err(Unmapped {
                    cell: name.clone(),
                    needs: "DSP48E2",
                    unblocker: std::borrow::Cow::Owned(format!(
                        "malformed DSP48E2 source cell {name}: alumode {alumode} out of range 0..=15 (4 bits)"
                    )),
                });
            }
            if *inmode > 0x1F {
                return Err(Unmapped {
                    cell: name.clone(),
                    needs: "DSP48E2",
                    unblocker: std::borrow::Cow::Owned(format!(
                        "malformed DSP48E2 source cell {name}: inmode {inmode} out of range 0..=31 (5 bits)"
                    )),
                });
            }
            Ok(MappedCell::Dsp48e2 {
                opmode: *opmode,
                alumode: *alumode,
                inmode: *inmode,
            })
        }
        SourceKind::Ramb36e2 {
            read_width_a,
            read_width_b,
            write_width_a,
            write_width_b,
        } => {
            for (field, w) in [
                ("read_width_a", *read_width_a),
                ("read_width_b", *read_width_b),
                ("write_width_a", *write_width_a),
                ("write_width_b", *write_width_b),
            ] {
                if !valid_bram_width(w, true) {
                    return Err(Unmapped {
                        cell: name.clone(),
                        needs: "RAMB36E2",
                        unblocker: std::borrow::Cow::Owned(format!(
                            "malformed RAMB36E2 source cell {name}: {field} {w} not in {{1, 2, 4, 9, 18, 36}}"
                        )),
                    });
                }
            }
            Ok(MappedCell::Ramb36e2 {
                read_width_a: *read_width_a,
                read_width_b: *read_width_b,
                write_width_a: *write_width_a,
                write_width_b: *write_width_b,
            })
        }
        SourceKind::Ramb18e2 {
            read_width_a,
            read_width_b,
            write_width_a,
            write_width_b,
        } => {
            for (field, w) in [
                ("read_width_a", *read_width_a),
                ("read_width_b", *read_width_b),
                ("write_width_a", *write_width_a),
                ("write_width_b", *write_width_b),
            ] {
                if !valid_bram_width(w, false) {
                    return Err(Unmapped {
                        cell: name.clone(),
                        needs: "RAMB18E2",
                        unblocker: std::borrow::Cow::Owned(format!(
                            "malformed RAMB18E2 source cell {name}: {field} {w} not in {{1, 2, 4, 9, 18}}"
                        )),
                    });
                }
            }
            Ok(MappedCell::Ramb18e2 {
                read_width_a: *read_width_a,
                read_width_b: *read_width_b,
                write_width_a: *write_width_a,
                write_width_b: *write_width_b,
            })
        }
        SourceKind::Uram288 { data_width, depth } => {
            if *data_width != 72 || *depth != 4096 {
                return Err(Unmapped {
                    cell: name.clone(),
                    needs: "URAM288",
                    unblocker: std::borrow::Cow::Owned(format!(
                        "malformed URAM288 source cell {name}: geometry {data_width}x{depth} is not the fixed 4096x72"
                    )),
                });
            }
            Ok(MappedCell::Uram288 {
                data_width: *data_width,
                depth: *depth,
            })
        }
    }
}

/// LUT6 INIT for a 6-input AND of I0..I5: only address 63 (all inputs 1)
/// produces 1, so only bit 63 is set.
///
/// This is the case class that caught the qor2-mapper miscompile
/// (6-input AND to const 0). The regression test asserts the bitloc
/// round trip preserves exactly this INIT; see `and6_init_roundtrip`.
pub const AND6_INIT: u64 = 0x8000_0000_0000_0000;

/// One CARRY8 placement produced from Carry4 cells.
///
/// A CARRY8 holds two 4-bit nibbles. `low` is always present (bits 3:0);
/// `high` is present when two Carry4 cells share the CARRY8 (bits 7:4).
/// `dual_cy4` selects the segbits CARRY_TYPE: SINGLE_CY8 when the two
/// nibbles are one chained 8-bit adder, DUAL_CY4 when they are two
/// independent 4-bit chains. A lone low nibble is always SINGLE_CY8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carry8Mapping {
    /// Low-nibble Carry4 cell (bits 3:0).
    pub low: String,
    /// High-nibble Carry4 cell (bits 7:4), if any.
    pub high: Option<String>,
    /// True for two independent chains (DUAL_CY4), false for one
    /// chained 8-bit adder or a lone nibble (SINGLE_CY8).
    pub dual_cy4: bool,
}

impl Carry8Mapping {
    /// FASM line for the CARRY_TYPE of this CARRY8 at `site`
    /// (placed slice name, e.g. `SLICE_X0Y0`). The feature strings are
    /// verbatim from segbits_clel_l.db: `CARRY8.CARRY_TYPE.DUAL_CY4`
    /// (sets bit 12_45) and `CARRY8.CARRY_TYPE.SINGLE_CY8` (clears it).
    pub fn carry_type_fasm(&self, site: &str) -> String {
        if self.dual_cy4 {
            format!("{site}.CARRY8.CARRY_TYPE.DUAL_CY4")
        } else {
            format!("{site}.CARRY8.CARRY_TYPE.SINGLE_CY8")
        }
    }
}

/// Pair Carry4 chains into CARRY8 placements.
///
/// Input is the chain list from extraction: each chain is a list of
/// Carry4 cell names in carry order (cell[i+1].CI is driven by
/// cell[i].CO3). A chain of length 1 is independent by construction
/// (its CI is not driven by another Carry4).
///
/// Rules, each justified by the CARRY8 structure (UG974):
/// - Consecutive cells of one chain pair into one CARRY8, SINGLE_CY8
///   (low = earlier cell, high = later cell): the 8-bit carry ripples
///   low to high inside the CARRY8 exactly as it rippled between the
///   two Carry4 cells.
/// - A leftover single from a chain of length > 1 is carry-dependent:
///   it takes its own CARRY8, SINGLE_CY8, low nibble only. It must NOT
///   be paired DUAL_CY4 with an independent cell: its carry-in comes
///   from the previous CARRY8, and DUAL_CY4 feeds the high nibble from
///   CI_TOP, not from the low nibble.
/// - Leftover singles from chains of length 1 are independent: they
///   pair into DUAL_CY4 CARRY8s (two independent 4-bit chains per
///   CARRY8, per UG974). An odd one out takes its own CARRY8,
///   SINGLE_CY8, low nibble only.
pub fn pair_carry4_chains(chains: &[Vec<String>]) -> Vec<Carry8Mapping> {
    let mut out: Vec<Carry8Mapping> = Vec::new();
    let mut independent_singles: Vec<String> = Vec::new();
    for chain in chains {
        let mut i = 0;
        while i + 1 < chain.len() {
            out.push(Carry8Mapping {
                low: chain[i].clone(),
                high: Some(chain[i + 1].clone()),
                dual_cy4: false,
            });
            i += 2;
        }
        if i < chain.len() {
            if chain.len() == 1 {
                independent_singles.push(chain[i].clone());
            } else {
                out.push(Carry8Mapping {
                    low: chain[i].clone(),
                    high: None,
                    dual_cy4: false,
                });
            }
        }
    }
    let mut j = 0;
    while j + 1 < independent_singles.len() {
        out.push(Carry8Mapping {
            low: independent_singles[j].clone(),
            high: Some(independent_singles[j + 1].clone()),
            dual_cy4: true,
        });
        j += 2;
    }
    if j < independent_singles.len() {
        out.push(Carry8Mapping {
            low: independent_singles[j].clone(),
            high: None,
            dual_cy4: false,
        });
    }
    out
}

/// FF control group for the CEUSED/SRUSED/CLKINV/SRINV/SYNC segbits features.
///
/// The vendored DBs carry these features per group, not per FF:
/// `ABCDFF` (FFs A-D, Q), `EFGHFF` (FFs E-H, Q), `ABCDFF2` (FFs A-D,
/// Q2), `EFGHFF2` (FFs E-H, Q2) for CEUSED and SRUSED; CLKINV, SRINV,
/// and SYNC exist only on the Q groups (`ABCDFF`, `EFGHFF`), shared
/// with the Q2 FFs of the same letters. BEL index 0..15 maps letter =
/// index/2 (A-H), Q2 = odd index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FfControlGroup {
    Abcd,
    Efgh,
    Abcd2,
    Efgh2,
}

impl FfControlGroup {
    /// CEUSED group for a BEL index 0..15.
    pub fn for_ff(ff_index: u8) -> Result<FfControlGroup, String> {
        if ff_index >= 16 {
            return Err(format!(
                "ULTRA SYNTHMAP: FF BEL index {ff_index} out of range 0..16"
            ));
        }
        let half = ff_index < 8;
        let q2 = ff_index % 2 == 1;
        Ok(match (half, q2) {
            (true, false) => FfControlGroup::Abcd,
            (false, false) => FfControlGroup::Efgh,
            (true, true) => FfControlGroup::Abcd2,
            (false, true) => FfControlGroup::Efgh2,
        })
    }

    /// FASM feature prefix for the CEUSED bit of this group.
    pub fn ceused_feature(&self) -> &'static str {
        match self {
            FfControlGroup::Abcd => "ABCDFF.CEUSED",
            FfControlGroup::Efgh => "EFGHFF.CEUSED",
            FfControlGroup::Abcd2 => "ABCDFF2.CEUSED",
            FfControlGroup::Efgh2 => "EFGHFF2.CEUSED",
        }
    }

    /// FASM feature prefix for the CLKINV bit shared by this group and
    /// its Q2 partner group. Only the Q groups carry the feature.
    pub fn clkinv_feature(&self) -> &'static str {
        match self {
            FfControlGroup::Abcd | FfControlGroup::Abcd2 => "ABCDFF.CLKINV",
            FfControlGroup::Efgh | FfControlGroup::Efgh2 => "EFGHFF.CLKINV",
        }
    }

    /// FASM feature prefix for the SRINV bit, same sharing as CLKINV.
    pub fn srinv_feature(&self) -> &'static str {
        match self {
            FfControlGroup::Abcd | FfControlGroup::Abcd2 => "ABCDFF.SRINV",
            FfControlGroup::Efgh | FfControlGroup::Efgh2 => "EFGHFF.SRINV",
        }
    }

    /// FASM feature prefix for the SRUSED bit of this group.
    pub fn srused_feature(&self) -> &'static str {
        match self {
            FfControlGroup::Abcd => "ABCDFF.SRUSED",
            FfControlGroup::Efgh => "EFGHFF.SRUSED",
            FfControlGroup::Abcd2 => "ABCDFF2.SRUSED",
            FfControlGroup::Efgh2 => "EFGHFF2.SRUSED",
        }
    }

    /// FASM feature prefix for the SYNC bit shared by this group and
    /// its Q2 partner group. Only the Q groups carry the feature.
    pub fn sync_feature(&self) -> &'static str {
        match self {
            FfControlGroup::Abcd | FfControlGroup::Abcd2 => "ABCDFF.SYNC",
            FfControlGroup::Efgh | FfControlGroup::Efgh2 => "EFGHFF.SYNC",
        }
    }
}

/// Emit FF control FASM for a placed design: one CEUSED line and one
/// SRUSED line per used group, plus one CLKINV, one SRINV, and (when
/// the half has an SR net) one SYNC line per used half (ABCD/EFGH),
/// in deterministic order.
///
/// Polarity (INTERFACES.md, resolved): for CEUSED, SRUSED, CLKINV,
/// and SRINV, V0 clears the bit and V1 sets it, the opposite of
/// INIT/SRVAL.
/// - CEUSED is the OR over the group: V1 if any FF in the group has a
///   CE net, else V0. SRUSED is the same OR over SR nets: V1 if any FF
///   in the group has an SR net, else V0. SRUSED.V0 is the all-zero
///   default and means the SR input is ignored, so omitting the line
///   for an SR-carrying group would silently drop the reset.
/// - CLKINV/SRINV are always V0 today: the IR carries no clock or
///   reset inversion, so no inversion is ever specified. If the IR
///   grows inversion, this function must take it as input; emitting V0
///   unconditionally then would be a miscompile, so the limitation is
///   stated here, not hidden.
/// - SYNC follows the FDRE mapping: Hff maps to FDRE, a synchronous
///   reset FF, so a half with any SR net emits `{half}.SYNC.SYNC`
///   (sets the bit; the all-zero default is ASYNC). A half with no SR
///   net emits no SYNC line: SRUSED.V0 ignores SR there, so SYNC is
///   don't-care. If a future front end emits asynchronous-reset FFs,
///   they must map to FDCE and this emitter must take sync/async as
///   input; emitting SYNC.SYNC for them would be wrong.
/// - MODE needs no line: the DB default (bit clear) is MODE.FF, and
///   Hff is always a flip-flop, never a latch.
/// - Fail loud if two FFs in one CEUSED or SRUSED group have DIFFERENT
///   CE or SR nets: the group shares one config bit per feature in the
///   DB, so the group cannot express two different enables or resets.
///   This is a conservative backstop: pack does not enforce CEUSED or
///   SRUSED groups today, so the emitter must. If the CTO rules CE/SR
///   is per-FF and CEUSED/SRUSED is only an OR enable, this check
///   relaxes to the OR rule above.
pub fn emit_ff_control_fasm(
    packed: &[PackedUltraSlice],
    placed: &UltraPlaced,
) -> Result<Vec<String>, String> {
    // (any_ce, ce_net, any_sr, sr_net) for one FF control group: the
    // net is the first CE/SR net seen in the group; a second, different
    // net fails loud below.
    type FfControlState = (bool, Option<String>, bool, Option<String>);
    let mut groups: HashMap<(String, FfControlGroup), FfControlState> = HashMap::new();
    // site -> (half ABCD?, half EFGH?) present.
    let mut halves: HashMap<String, (bool, bool)> = HashMap::new();
    for p in &placed.slices {
        let slice = packed.get(p.packed_idx).ok_or_else(|| {
            format!(
                "ULTRA SYNTHMAP: placed slice {} references packed index {} out of range",
                p.site, p.packed_idx
            )
        })?;
        for bel in slice.ffs.keys() {
            let group = FfControlGroup::for_ff(*bel)?;
            let ce = slice.ff_ce.get(bel).cloned().flatten();
            let sr = slice.ff_sr.get(bel).cloned().flatten();
            let entry = groups
                .entry((p.site.clone(), group))
                .or_insert((false, None, false, None));
            if ce.is_some() {
                if let Some(seen) = &entry.1 {
                    if seen != ce.as_ref().unwrap() {
                        return Err(format!(
                            "ULTRA SYNTHMAP: FFs in CEUSED group {:?} at {} have different \
                             CE nets ({seen} vs {}); the group shares one CEUSED config bit",
                            group,
                            p.site,
                            ce.as_ref().unwrap()
                        ));
                    }
                }
                entry.0 = true;
                entry.1 = ce;
            }
            if sr.is_some() {
                if let Some(seen) = &entry.3 {
                    if seen != sr.as_ref().unwrap() {
                        return Err(format!(
                            "ULTRA SYNTHMAP: FFs in SRUSED group {:?} at {} have different \
                             SR nets ({seen} vs {}); the group shares one SRUSED config bit",
                            group,
                            p.site,
                            sr.as_ref().unwrap()
                        ));
                    }
                }
                entry.2 = true;
                entry.3 = sr;
            }
            let h = halves.entry(p.site.clone()).or_insert((false, false));
            if *bel < 8 {
                h.0 = true;
            } else {
                h.1 = true;
            }
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut group_keys: Vec<(String, FfControlGroup)> = groups.keys().cloned().collect();
    group_keys.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| (a.1 as u8).cmp(&(b.1 as u8))));
    for (site, group) in &group_keys {
        let (any_ce, _, _, _) = groups[&(site.clone(), *group)];
        let v = if any_ce { 1 } else { 0 };
        lines.push(format!("{site}.{}.V{v}", group.ceused_feature()));
    }
    for (site, group) in &group_keys {
        let (_, _, any_sr, _) = groups[&(site.clone(), *group)];
        let v = if any_sr { 1 } else { 0 };
        lines.push(format!("{site}.{}.V{v}", group.srused_feature()));
    }
    let mut sites: Vec<String> = halves.keys().cloned().collect();
    sites.sort();
    for site in sites {
        let (abcd, efgh) = halves[&site];
        let half_has_sr = |group: FfControlGroup, group2: FfControlGroup| {
            groups
                .get(&(site.clone(), group))
                .map(|g| g.2)
                .unwrap_or(false)
                || groups
                    .get(&(site.clone(), group2))
                    .map(|g| g.2)
                    .unwrap_or(false)
        };
        if abcd {
            lines.push(format!(
                "{site}.{}.V0",
                FfControlGroup::Abcd.clkinv_feature()
            ));
            lines.push(format!(
                "{site}.{}.V0",
                FfControlGroup::Abcd.srinv_feature()
            ));
            if half_has_sr(FfControlGroup::Abcd, FfControlGroup::Abcd2) {
                lines.push(format!(
                    "{site}.{}.SYNC",
                    FfControlGroup::Abcd.sync_feature()
                ));
            }
        }
        if efgh {
            lines.push(format!(
                "{site}.{}.V0",
                FfControlGroup::Efgh.clkinv_feature()
            ));
            lines.push(format!(
                "{site}.{}.V0",
                FfControlGroup::Efgh.srinv_feature()
            ));
            if half_has_sr(FfControlGroup::Efgh, FfControlGroup::Efgh2) {
                lines.push(format!(
                    "{site}.{}.SYNC",
                    FfControlGroup::Efgh.sync_feature()
                ));
            }
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitloc::{ClbTileType, LutMode, lut_placed_bits};
    use crate::pack::{ControlSetLimits, SliceKind, pack_ultra};
    use crate::place::{UltraGrid, UltraPlaceOpts, place_ultra};

    fn cell(name: &str, kind: SourceKind) -> SourceCell {
        SourceCell {
            name: name.to_string(),
            kind,
        }
    }

    #[test]
    fn lut6_maps_with_init_verbatim() {
        let m = map_source_cell(&cell(
            "u_lut",
            SourceKind::Lut6 {
                init: 0x5555_5555_5555_5555,
                is_memory: false,
                is_srl: false,
            },
        ))
        .unwrap();
        assert_eq!(
            m,
            MappedCell::Lut6 {
                init: 0x5555_5555_5555_5555
            }
        );
    }

    #[test]
    fn and6_init_roundtrip() {
        // 6-input AND regression: the qor2-mapper miscompile class.
        // INIT 0x8000000000000000 must survive bitloc as exactly one
        // set bit (bit 63) and 63 clear bits, and reconstruct to the
        // same INIT. A mapping to const 0 would show 0 set bits here.
        assert_eq!(AND6_INIT, 0x8000_0000_0000_0000);
        let placed = lut_placed_bits(ClbTileType::ClelL, 0, 0, LutMode::Logic, AND6_INIT).unwrap();
        assert_eq!(placed.len(), 64);
        let set: Vec<usize> = placed
            .iter()
            .enumerate()
            .filter(|(_, b)| b.value)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(set, vec![63], "AND6 must set exactly bit 63, got {set:?}");
        let mut rebuilt: u64 = 0;
        for (i, b) in placed.iter().enumerate() {
            if b.value {
                rebuilt |= 1u64 << i;
            }
        }
        assert_eq!(rebuilt, AND6_INIT);
    }

    #[test]
    fn inverter_init_roundtrip() {
        // Blinky inverter: INIT 0x5555555555555555, 32 set bits.
        let placed = lut_placed_bits(
            ClbTileType::ClelL,
            0,
            0,
            LutMode::Logic,
            0x5555_5555_5555_5555,
        )
        .unwrap();
        assert_eq!(placed.iter().filter(|b| b.value).count(), 32);
    }

    #[test]
    fn srl_lut6_fails_loud_not_plain_lut() {
        let err = map_source_cell(&cell(
            "u_srl",
            SourceKind::Lut6 {
                init: 0xAAAA_AAAA_AAAA_AAAA,
                is_memory: false,
                is_srl: true,
            },
        ))
        .unwrap_err();
        assert!(err.unblocker.contains("BLOCKED"), "got {err}");
        assert_eq!(err.needs, "SRL16E/SRLC32E (SLICEM shift register)");
    }

    #[test]
    fn memory_lut6_fails_loud_not_plain_lut() {
        let err = map_source_cell(&cell(
            "u_mem",
            SourceKind::Lut6 {
                init: 0,
                is_memory: true,
                is_srl: false,
            },
        ))
        .unwrap_err();
        assert!(err.needs.contains("distributed RAM"), "got {err}");
    }

    #[test]
    fn ff_maps_with_ce_sr_presence() {
        let m = map_source_cell(&cell(
            "q_reg",
            SourceKind::Ff {
                has_ce: true,
                has_sr: false,
            },
        ))
        .unwrap();
        assert_eq!(
            m,
            MappedCell::FdRe {
                has_ce: true,
                has_sr: false
            }
        );
    }

    #[test]
    fn dsp48e1_fails_loud_with_unblocker() {
        let err = map_source_cell(&cell(
            "u_dsp",
            SourceKind::Dsp48e1 {
                opmode: 0b0000101,
                alumode: 0,
            },
        ))
        .unwrap_err();
        assert_eq!(err.needs, "DSP48E2");
        assert!(err.unblocker.contains("DSP48E2"), "got {err}");
    }

    #[test]
    fn bram18e1_fails_loud_with_unblocker() {
        let err = map_source_cell(&cell(
            "u_bram",
            SourceKind::Bram18e1 {
                read_width_a: 18,
                read_width_b: 18,
                write_width_a: 18,
                write_width_b: 18,
            },
        ))
        .unwrap_err();
        assert_eq!(err.needs, "RAMB18E2");
    }

    #[test]
    fn blackbox_fails_loud() {
        let err = map_source_cell(&cell(
            "u_ip",
            SourceKind::BlackBox {
                module: "my_ip".to_string(),
            },
        ))
        .unwrap_err();
        assert!(err.to_string().contains("u_ip"), "got {err}");
    }

    #[test]
    fn dsp48e2_native_maps_with_control_values() {
        let m = map_source_cell(&cell(
            "u_dsp",
            SourceKind::Dsp48e2 {
                opmode: 0b000_000_101,
                alumode: 0,
                inmode: 0,
            },
        ))
        .unwrap();
        assert_eq!(
            m,
            MappedCell::Dsp48e2 {
                opmode: 0b000_000_101,
                alumode: 0,
                inmode: 0
            }
        );
    }

    #[test]
    fn dsp48e2_control_values_validated() {
        // OPMODE is 9 bits, ALUMODE 4, INMODE 5 (vendored
        // site_type_DSP48E2.json pins). One past each bound fails loud.
        for (opmode, alumode, inmode) in [
            (0x200u16, 0u8, 0u8),
            (0u16, 0x10u8, 0u8),
            (0u16, 0u8, 0x20u8),
        ] {
            let err = map_source_cell(&cell(
                "u_dsp",
                SourceKind::Dsp48e2 {
                    opmode,
                    alumode,
                    inmode,
                },
            ))
            .unwrap_err();
            assert_eq!(err.needs, "DSP48E2");
            assert!(err.unblocker.contains("malformed"), "got {err}");
        }
        // The bounds themselves map.
        assert!(
            map_source_cell(&cell(
                "u_dsp",
                SourceKind::Dsp48e2 {
                    opmode: 0x1FF,
                    alumode: 0xF,
                    inmode: 0x1F,
                },
            ))
            .is_ok()
        );
    }

    #[test]
    fn ramb36e2_widths_validated() {
        let m = map_source_cell(&cell(
            "u_bram",
            SourceKind::Ramb36e2 {
                read_width_a: 36,
                read_width_b: 18,
                write_width_a: 9,
                write_width_b: 1,
            },
        ))
        .unwrap();
        assert_eq!(
            m,
            MappedCell::Ramb36e2 {
                read_width_a: 36,
                read_width_b: 18,
                write_width_a: 9,
                write_width_b: 1,
            }
        );
        for bad in [0u8, 3, 8, 72] {
            let err = map_source_cell(&cell(
                "u_bram",
                SourceKind::Ramb36e2 {
                    read_width_a: bad,
                    read_width_b: 18,
                    write_width_a: 18,
                    write_width_b: 18,
                },
            ))
            .unwrap_err();
            assert_eq!(err.needs, "RAMB36E2");
            assert!(err.unblocker.contains("read_width_a"), "got {err}");
        }
    }

    #[test]
    fn ramb18e2_width_36_rejected() {
        // RAMB18E2 has no 36-bit port (18Kb); 36 is a RAMB36E2 width.
        let err = map_source_cell(&cell(
            "u_bram",
            SourceKind::Ramb18e2 {
                read_width_a: 36,
                read_width_b: 18,
                write_width_a: 18,
                write_width_b: 18,
            },
        ))
        .unwrap_err();
        assert_eq!(err.needs, "RAMB18E2");
        assert!(err.unblocker.contains("36"), "got {err}");
        assert!(
            map_source_cell(&cell(
                "u_bram",
                SourceKind::Ramb18e2 {
                    read_width_a: 18,
                    read_width_b: 9,
                    write_width_a: 4,
                    write_width_b: 2,
                },
            ))
            .is_ok()
        );
    }

    #[test]
    fn uram288_fixed_geometry() {
        assert!(
            map_source_cell(&cell(
                "u_uram",
                SourceKind::Uram288 {
                    data_width: 72,
                    depth: 4096,
                },
            ))
            .is_ok()
        );
        for (width, depth) in [(36u8, 4096u32), (72u8, 2048u32)] {
            let err = map_source_cell(&cell(
                "u_uram",
                SourceKind::Uram288 {
                    data_width: width,
                    depth,
                },
            ))
            .unwrap_err();
            assert_eq!(err.needs, "URAM288");
            assert!(err.unblocker.contains("4096x72"), "got {err}");
        }
    }

    #[test]
    fn every_source_kind_maps_or_fails_loud() {
        // Pins the primitive inventory: every SourceKind variant has a
        // verdict here. A new variant without a map arm fails to
        // compile (exhaustive match in map_source_cell); a new arm
        // without a verdict here fails this test.
        let cases: Vec<(SourceCell, bool)> = vec![
            (
                cell(
                    "lut",
                    SourceKind::Lut6 {
                        init: 0,
                        is_memory: false,
                        is_srl: false,
                    },
                ),
                true,
            ),
            (
                cell(
                    "srl",
                    SourceKind::Lut6 {
                        init: 0,
                        is_memory: false,
                        is_srl: true,
                    },
                ),
                false,
            ),
            (
                cell(
                    "mem",
                    SourceKind::Lut6 {
                        init: 0,
                        is_memory: true,
                        is_srl: false,
                    },
                ),
                false,
            ),
            (
                cell(
                    "ff",
                    SourceKind::Ff {
                        has_ce: false,
                        has_sr: false,
                    },
                ),
                true,
            ),
            (cell("carry", SourceKind::Carry4), true),
            (cell("ibin", SourceKind::IobIn), true),
            (cell("ibout", SourceKind::IobOut), true),
            (cell("clk", SourceKind::ClockBuf), true),
            (
                cell(
                    "dsp1",
                    SourceKind::Dsp48e1 {
                        opmode: 5,
                        alumode: 0,
                    },
                ),
                false,
            ),
            (cell("mac", SourceKind::Mac27), false),
            (cell("ila", SourceKind::Ila), false),
            (cell("bram", SourceKind::Bram18), false),
            (
                cell(
                    "bram1",
                    SourceKind::Bram18e1 {
                        read_width_a: 18,
                        read_width_b: 18,
                        write_width_a: 18,
                        write_width_b: 18,
                    },
                ),
                false,
            ),
            (
                cell(
                    "bb",
                    SourceKind::BlackBox {
                        module: "ip".to_string(),
                    },
                ),
                false,
            ),
            (
                cell(
                    "dsp2",
                    SourceKind::Dsp48e2 {
                        opmode: 5,
                        alumode: 0,
                        inmode: 0,
                    },
                ),
                true,
            ),
            (
                cell(
                    "r36",
                    SourceKind::Ramb36e2 {
                        read_width_a: 18,
                        read_width_b: 18,
                        write_width_a: 18,
                        write_width_b: 18,
                    },
                ),
                true,
            ),
            (
                cell(
                    "r18",
                    SourceKind::Ramb18e2 {
                        read_width_a: 18,
                        read_width_b: 18,
                        write_width_a: 18,
                        write_width_b: 18,
                    },
                ),
                true,
            ),
            (
                cell(
                    "uram",
                    SourceKind::Uram288 {
                        data_width: 72,
                        depth: 4096,
                    },
                ),
                true,
            ),
        ];
        assert_eq!(
            cases.len(),
            18,
            "one case per inventory cell (16 kinds, LUT6 x3)"
        );
        for (source, maps) in &cases {
            let result = map_source_cell(source);
            if *maps {
                assert!(
                    result.is_ok(),
                    "cell {} must map, got {result:?}",
                    source.name
                );
            } else {
                let err = result.unwrap_err();
                assert!(
                    !err.needs.is_empty(),
                    "cell {} needs a primitive",
                    source.name
                );
                assert!(
                    !err.unblocker.is_empty(),
                    "cell {} needs an unblocker",
                    source.name
                );
            }
        }
    }

    #[test]
    fn carry4_chain_pairs_single_cy8() {
        let chains = vec![vec!["c0".to_string(), "c1".to_string()]];
        let paired = pair_carry4_chains(&chains);
        assert_eq!(
            paired,
            vec![Carry8Mapping {
                low: "c0".to_string(),
                high: Some("c1".to_string()),
                dual_cy4: false,
            }]
        );
        assert_eq!(
            paired[0].carry_type_fasm("SLICE_X0Y0"),
            "SLICE_X0Y0.CARRY8.CARRY_TYPE.SINGLE_CY8"
        );
    }

    #[test]
    fn carry4_chain_of_three_leftover_gets_own_carry8() {
        // c2 is carry-dependent (CI from c1): it must NOT pair DUAL_CY4.
        let chains = vec![vec!["c0".to_string(), "c1".to_string(), "c2".to_string()]];
        let paired = pair_carry4_chains(&chains);
        assert_eq!(paired.len(), 2);
        assert_eq!(paired[0].high.as_deref(), Some("c1"));
        assert!(!paired[0].dual_cy4);
        assert_eq!(paired[1].low, "c2");
        assert_eq!(paired[1].high, None);
        assert!(!paired[1].dual_cy4);
    }

    #[test]
    fn independent_carry4_pair_dual_cy4() {
        let chains = vec![vec!["a".to_string()], vec!["b".to_string()]];
        let paired = pair_carry4_chains(&chains);
        assert_eq!(paired.len(), 1);
        assert_eq!(paired[0].low, "a");
        assert_eq!(paired[0].high.as_deref(), Some("b"));
        assert!(paired[0].dual_cy4);
        assert_eq!(
            paired[0].carry_type_fasm("SLICE_X0Y0"),
            "SLICE_X0Y0.CARRY8.CARRY_TYPE.DUAL_CY4"
        );
    }

    #[test]
    fn odd_independent_carry4_leftover_single() {
        let chains = vec![
            vec!["a".to_string()],
            vec!["b".to_string()],
            vec!["c".to_string()],
        ];
        let paired = pair_carry4_chains(&chains);
        assert_eq!(paired.len(), 2);
        assert!(paired[0].dual_cy4);
        assert_eq!(paired[1].low, "c");
        assert_eq!(paired[1].high, None);
        assert!(!paired[1].dual_cy4);
    }

    #[test]
    fn ff_control_group_mapping() {
        // BEL 0 AQ -> ABCD, 1 AQ2 -> ABCD2, 8 EQ -> EFGH, 15 HQ2 -> EFGH2.
        assert_eq!(FfControlGroup::for_ff(0).unwrap(), FfControlGroup::Abcd);
        assert_eq!(FfControlGroup::for_ff(1).unwrap(), FfControlGroup::Abcd2);
        assert_eq!(FfControlGroup::for_ff(8).unwrap(), FfControlGroup::Efgh);
        assert_eq!(FfControlGroup::for_ff(15).unwrap(), FfControlGroup::Efgh2);
        assert!(FfControlGroup::for_ff(16).is_err());
        assert_eq!(FfControlGroup::Abcd.ceused_feature(), "ABCDFF.CEUSED");
        // CLKINV is shared with the Q2 partner group.
        assert_eq!(FfControlGroup::Abcd2.clkinv_feature(), "ABCDFF.CLKINV");
    }

    struct SmallGrid;
    impl UltraGrid for SmallGrid {
        fn slice_sites(&self) -> Vec<(String, String, i32, i32, SliceKind)> {
            let mut sites = Vec::new();
            for y in 0..4 {
                for x in 0..4 {
                    sites.push((
                        format!("SLICE_X{x}Y{y}"),
                        format!("CLB_X{x}Y{y}"),
                        x,
                        y,
                        SliceKind::Logic,
                    ));
                }
            }
            sites
        }
        fn grid_size(&self) -> (i32, i32) {
            (4, 4)
        }
    }

    #[test]
    fn ff_control_fasm_no_ce_emits_v0() {
        let limits = ControlSetLimits::default();
        let luts = vec![("lut0".to_string(), 0xAAAAAAAAAAAAAAAAu64, false)];
        let ffs = vec![("q_reg".to_string(), "clk".to_string(), None, None)];
        let packed = pack_ultra(&luts, &ffs, &[], &limits).unwrap();
        let placed = place_ultra(&packed.slices, &SmallGrid, &UltraPlaceOpts::default()).unwrap();
        let lines = emit_ff_control_fasm(&packed.slices, &placed).unwrap();
        let site = &placed.slices[0].site;
        assert!(
            lines.contains(&format!("{site}.ABCDFF.CEUSED.V0")),
            "got {lines:?}"
        );
        assert!(
            lines.contains(&format!("{site}.ABCDFF.CLKINV.V0")),
            "got {lines:?}"
        );
        assert!(
            lines.contains(&format!("{site}.ABCDFF.SRINV.V0")),
            "got {lines:?}"
        );
        assert!(
            lines.contains(&format!("{site}.ABCDFF.SRUSED.V0")),
            "got {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("SYNC")),
            "no SR net, so no SYNC line is emitted, got {lines:?}"
        );
    }

    #[test]
    fn ff_control_fasm_with_ce_emits_v1() {
        let limits = ControlSetLimits::default();
        let luts = vec![("lut0".to_string(), 0xAAAAAAAAAAAAAAAAu64, false)];
        let ffs = vec![(
            "q_reg".to_string(),
            "clk".to_string(),
            Some("en".to_string()),
            None,
        )];
        let packed = pack_ultra(&luts, &ffs, &[], &limits).unwrap();
        let placed = place_ultra(&packed.slices, &SmallGrid, &UltraPlaceOpts::default()).unwrap();
        let lines = emit_ff_control_fasm(&packed.slices, &placed).unwrap();
        let site = &placed.slices[0].site;
        assert!(
            lines.contains(&format!("{site}.ABCDFF.CEUSED.V1")),
            "got {lines:?}"
        );
    }

    #[test]
    fn ff_control_fasm_with_sr_emits_srused_v1_and_sync() {
        // An FF with an SR net but no CE: CEUSED stays V0, SRUSED goes
        // V1, and the half emits SYNC.SYNC (FDRE, synchronous reset).
        let limits = ControlSetLimits::default();
        let luts = vec![("lut0".to_string(), 0xAAAAAAAAAAAAAAAAu64, false)];
        let ffs = vec![(
            "q_reg".to_string(),
            "clk".to_string(),
            None,
            Some("rst".to_string()),
        )];
        let packed = pack_ultra(&luts, &ffs, &[], &limits).unwrap();
        let placed = place_ultra(&packed.slices, &SmallGrid, &UltraPlaceOpts::default()).unwrap();
        let lines = emit_ff_control_fasm(&packed.slices, &placed).unwrap();
        let site = &placed.slices[0].site;
        assert!(
            lines.contains(&format!("{site}.ABCDFF.CEUSED.V0")),
            "got {lines:?}"
        );
        assert!(
            lines.contains(&format!("{site}.ABCDFF.SRUSED.V1")),
            "got {lines:?}"
        );
        assert!(
            lines.contains(&format!("{site}.ABCDFF.SYNC.SYNC")),
            "got {lines:?}"
        );
        assert!(
            lines.contains(&format!("{site}.ABCDFF.CLKINV.V0")),
            "got {lines:?}"
        );
        assert!(
            lines.contains(&format!("{site}.ABCDFF.SRINV.V0")),
            "got {lines:?}"
        );
    }

    #[test]
    fn ff_control_fasm_different_sr_in_one_group_fails_loud() {
        // Two FFs in the ABCD group (BELs 0 and 2) with different SR
        // nets: the shared SRUSED bit cannot express both.
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        for (bel, name, sr) in [(0u8, "qa", "rst_a"), (2u8, "qb", "rst_b")] {
            slice.ffs.insert(bel, name.to_string());
            slice.ff_clk.insert(bel, "clk".to_string());
            slice.ff_ce.insert(bel, None);
            slice.ff_sr.insert(bel, Some(sr.to_string()));
        }
        let packed = vec![slice];
        let placed = UltraPlaced {
            slices: vec![crate::place::PlacedUltraSlice {
                packed_idx: 0,
                site: "SLICE_X0Y0".to_string(),
                tile: "CLB_X0Y0".to_string(),
                x: 0,
                y: 0,
            }],
            seed: 0,
        };
        let err = emit_ff_control_fasm(&packed, &placed).unwrap_err();
        assert!(err.contains("different"), "got {err}");
        assert!(err.contains("SRUSED"), "got {err}");
    }

    #[test]
    fn ff_control_group_srused_sync_features() {
        assert_eq!(FfControlGroup::Abcd.srused_feature(), "ABCDFF.SRUSED");
        assert_eq!(FfControlGroup::Efgh.srused_feature(), "EFGHFF.SRUSED");
        assert_eq!(FfControlGroup::Abcd2.srused_feature(), "ABCDFF2.SRUSED");
        assert_eq!(FfControlGroup::Efgh2.srused_feature(), "EFGHFF2.SRUSED");
        // SYNC is shared with the Q2 partner group, like CLKINV/SRINV.
        assert_eq!(FfControlGroup::Abcd2.sync_feature(), "ABCDFF.SYNC");
        assert_eq!(FfControlGroup::Efgh2.sync_feature(), "EFGHFF.SYNC");
    }

    #[test]
    fn ff_control_fasm_different_ce_in_one_group_fails_loud() {
        // Two FFs in the ABCD group (BELs 0 and 2) with different CE
        // nets: the shared CEUSED bit cannot express both.
        let mut slice = PackedUltraSlice::new("SLICE_X0Y0".to_string(), SliceKind::Logic);
        for (bel, name, ce) in [(0u8, "qa", "en_a"), (2u8, "qb", "en_b")] {
            slice.ffs.insert(bel, name.to_string());
            slice.ff_clk.insert(bel, "clk".to_string());
            slice.ff_ce.insert(bel, Some(ce.to_string()));
            slice.ff_sr.insert(bel, None);
        }
        let packed = vec![slice];
        let placed = UltraPlaced {
            slices: vec![crate::place::PlacedUltraSlice {
                packed_idx: 0,
                site: "SLICE_X0Y0".to_string(),
                tile: "CLB_X0Y0".to_string(),
                x: 0,
                y: 0,
            }],
            seed: 0,
        };
        let err = emit_ff_control_fasm(&packed, &placed).unwrap_err();
        assert!(err.contains("different"), "got {err}");
        assert!(err.contains("CEUSED"), "got {err}");
    }
}
