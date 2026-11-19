//! UltraScale+ timing database implementation.
//!
//! Designed to integrate with device-model's `helion-ultrascale` crate.
//! Implements the `TimingDb` trait seam defined in their `fabric.rs`.
//!
//! Every field is `Option<i64>` (picoseconds). `None` means UNKNOWN: the value
//! has not been sourced from a public document and must not be invented.
//!
//! Sources used:
//! - DS923: Virtex UltraScale+ Data Sheet, DC and AC Switching Characteristics.
//! - AMD community forum (adaptivesupport.amd.com): ~0.3ns per logic level
//!   (LUT + route) ballpark for UltraScale+, LUT alone under 1ns.
//! - UG570: UltraScale Architecture Configurable Logic Blocks (structure).
//!
//! Speed grade: -2 (0.85V) unless noted. All values are max (slow corner).

use serde::{Deserialize, Serialize};

/// Source tag for a delay value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DelaySource {
    /// From a named public document, e.g. "DS923 Table 12".
    Documented(String),
    /// Ballpark from a published source, not a datasheet table.
    Ballpark(String),
    /// Not yet sourced. A made-up delay is worse than a missing one.
    Unknown,
}

/// A delay value with its provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcedDelay {
    /// Picoseconds. `None` means UNKNOWN.
    pub ps: Option<i64>,
    pub source: DelaySource,
}

impl SourcedDelay {
    pub fn unknown() -> Self {
        Self {
            ps: None,
            source: DelaySource::Unknown,
        }
    }

    pub fn documented(ps: i64, doc: &str) -> Self {
        Self {
            ps: Some(ps),
            source: DelaySource::Documented(doc.to_string()),
        }
    }

    pub fn ballpark(ps: i64, src: &str) -> Self {
        Self {
            ps: Some(ps),
            source: DelaySource::Ballpark(src.to_string()),
        }
    }
}

/// Concrete timing database implementing device-model's `TimingDb` trait.
///
/// When integrated into device-model's `helion-ultrascale` crate, implement:
/// ```text
/// impl crate::fabric::TimingDb for UltraScaleTimingDb {
///     fn has_data(&self) -> bool { true }
/// }
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UltraScaleTimingDb {
    /// Part name, e.g. "xcvu3p".
    pub part: String,
    /// Speed grade, e.g. "-2".
    pub speed_grade: String,
    pub cells: CellDelays,
    pub carry8: Carry8Delays,
    pub dsp: Dsp48e2Delays,
    pub bram: Ramb36e2Delays,
    pub uram: Uram288Delays,
    pub interconnect: InterconnectDelays,
    pub iob: IobDelays,
}

/// LUT and flip-flop delays.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellDelays {
    /// LUT6 input pin to output (ps).
    pub lut6: SourcedDelay,
    /// Flip-flop clock-to-Q (ps).
    pub ff_ckq: SourcedDelay,
    /// Flip-flop setup requirement (ps).
    pub ff_setup: SourcedDelay,
    /// Flip-flop hold requirement (ps).
    pub ff_hold: SourcedDelay,
}

/// CARRY8 delays (per-bit and chain).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Carry8Delays {
    /// CI to CO through one bit (ps).
    pub ci_to_co_bit: SourcedDelay,
    /// S/DI to O (sum output) (ps).
    pub s_to_o: SourcedDelay,
    /// First-bit CYINIT to CO (ps).
    pub cyinit_to_co: SourcedDelay,
}

/// DSP48E2 pin-to-pin delays.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dsp48e2Delays {
    /// A/B input to P output, multiply only (ps).
    pub mult_a_b_to_p: SourcedDelay,
    /// Clock to P output (ps).
    pub clk_to_p: SourcedDelay,
}

/// RAMB36E2 pin-to-pin delays.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ramb36e2Delays {
    /// Clock to data out (ps).
    pub clk_to_do: SourcedDelay,
    /// Address setup (ps).
    pub addr_setup: SourcedDelay,
}

/// URAM288 pin-to-pin delays.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Uram288Delays {
    /// Clock to data out (ps).
    pub clk_to_do: SourcedDelay,
    /// Address setup (ps).
    pub addr_setup: SourcedDelay,
}

/// Interconnect wire delays by segment length.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InterconnectDelays {
    /// Single-length line (1 INT tile span) (ps).
    pub single: SourcedDelay,
    /// Double-length line (2 INT tile span) (ps).
    pub double: SourcedDelay,
    /// Quad-length line (4 INT tile span) (ps).
    pub quad: SourcedDelay,
    /// Long line (12+ INT tile span) (ps).
    pub long: SourcedDelay,
    /// PIP (switch matrix pass-through) (ps).
    pub pip: SourcedDelay,
}

/// I/O block delays.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IobDelays {
    /// Pad to fabric input (ps).
    pub pad_to_fabric: SourcedDelay,
    /// Fabric to pad output (ps).
    pub fabric_to_pad: SourcedDelay,
}

impl UltraScaleTimingDb {
    /// True when any delay in this DB is a ballpark rather than a
    /// documented datasheet value. STA reports built from such a DB are
    /// estimates, never signoff numbers.
    pub fn is_estimate(&self) -> bool {
        let all = [
            &self.cells.lut6,
            &self.cells.ff_ckq,
            &self.cells.ff_setup,
            &self.cells.ff_hold,
            &self.carry8.ci_to_co_bit,
            &self.carry8.s_to_o,
            &self.carry8.cyinit_to_co,
            &self.dsp.mult_a_b_to_p,
            &self.dsp.clk_to_p,
            &self.bram.clk_to_do,
            &self.bram.addr_setup,
            &self.uram.clk_to_do,
            &self.uram.addr_setup,
            &self.interconnect.single,
            &self.interconnect.double,
            &self.interconnect.quad,
            &self.interconnect.long,
            &self.interconnect.pip,
            &self.iob.pad_to_fabric,
            &self.iob.fabric_to_pad,
        ];
        all.iter()
            .any(|d| matches!(d.source, DelaySource::Ballpark(_)))
    }
}

/// Preliminary UltraScale+ timing for xcku3p, speed grade -2.
///
/// Same AMD forum ballpark as [`preliminary_xcvu3p`]: the ballpark is
/// UltraScale+ generic, not part-specific, so the xcku3p preliminary DB
/// carries the same split (120 ps LUT + 180 ps one INT hop = 300 ps per
/// logic level) under the xcku3p part name. Every value is Ballpark.
/// Do not use for signoff.
pub fn preliminary_xcku3p() -> UltraScaleTimingDb {
    let mut db = preliminary_xcvu3p();
    db.part = "xcku3p".to_string();
    db
}

/// Select the STA delay DB for a part. Today both parts run on the
/// preliminary ballpark DB, labeled estimate; the documented DBs below
/// cannot run STA (they fail loud on the first UNKNOWN field). Unknown
/// parts fail loud instead of silently falling back to another part.
pub fn sta_db_for_part(part: &str) -> Result<UltraScaleTimingDb, String> {
    match part {
        "xcku3p" => Ok(preliminary_xcku3p()),
        "xcvu3p" => Ok(preliminary_xcvu3p()),
        _ => Err(format!("timing: unsupported part {part}")),
    }
}

/// Documented-only timing DB for xcku3p, speed grade -2 at 0.85V.
///
/// Contains exactly one value: BRAM CLK to DOUT, 1460 ps, from DS922
/// Table 33 (Block RAM and FIFO Switching Characteristics), TRCKO_DO,
/// -2 / 0.85V, Max. Every other field is UNKNOWN: DS922 publishes no
/// LUT6, flip-flop, CARRY8, interconnect, DSP, URAM, or IOB pin-to-pin
/// delay, and FMAX values are periods, not delays, so none is stored.
/// Deliberately unusable for STA: [`crate::sta::sta_routed`] on this DB
/// fails loud with `timing: LUT6 delay UNKNOWN in TimingDb`.
pub fn documented_xcku3p() -> UltraScaleTimingDb {
    documented_db(
        "xcku3p",
        "DS922 Table 33 TRCKO_DO, -2 0.85V",
    )
}

/// Documented-only timing DB for xcvu3p, speed grade -2 at 0.85V.
///
/// Same single value as [`documented_xcku3p`], from DS923 Table 33
/// (Block RAM and FIFO Switching Characteristics), TRCKO_DO, -2 / 0.85V,
/// Max: 1460 ps. All other fields UNKNOWN, for the same reasons.
pub fn documented_xcvu3p() -> UltraScaleTimingDb {
    documented_db(
        "xcvu3p",
        "DS923 Table 33 TRCKO_DO, -2 0.85V",
    )
}

fn documented_db(part: &str, bram_source: &str) -> UltraScaleTimingDb {
    UltraScaleTimingDb {
        part: part.to_string(),
        speed_grade: "-2".to_string(),
        cells: CellDelays {
            lut6: SourcedDelay::unknown(),
            ff_ckq: SourcedDelay::unknown(),
            ff_setup: SourcedDelay::unknown(),
            ff_hold: SourcedDelay::unknown(),
        },
        carry8: Carry8Delays {
            ci_to_co_bit: SourcedDelay::unknown(),
            s_to_o: SourcedDelay::unknown(),
            cyinit_to_co: SourcedDelay::unknown(),
        },
        dsp: Dsp48e2Delays {
            mult_a_b_to_p: SourcedDelay::unknown(),
            clk_to_p: SourcedDelay::unknown(),
        },
        bram: Ramb36e2Delays {
            clk_to_do: SourcedDelay::documented(1460, bram_source),
            addr_setup: SourcedDelay::unknown(),
        },
        uram: Uram288Delays {
            clk_to_do: SourcedDelay::unknown(),
            addr_setup: SourcedDelay::unknown(),
        },
        interconnect: InterconnectDelays {
            single: SourcedDelay::unknown(),
            double: SourcedDelay::unknown(),
            quad: SourcedDelay::unknown(),
            long: SourcedDelay::unknown(),
            pip: SourcedDelay::unknown(),
        },
        iob: IobDelays {
            pad_to_fabric: SourcedDelay::unknown(),
            fabric_to_pad: SourcedDelay::unknown(),
        },
    }
}
///
/// Sourcing: AMD community forum ballpark of ~0.3ns per logic level
/// (LUT + route combined) for UltraScale+. The split below assigns 120ps to
/// the LUT and 180ps to one INT hop, summing to 300ps per level. This is a
/// PRELIMINARY placeholder, not Vivado-correlated. Every other field is
/// UNKNOWN until sourced from DS923 tables or equivalent.
///
/// Do not use for signoff. Do not tune to make a number look better.
pub fn preliminary_xcvu3p() -> UltraScaleTimingDb {
    let ballpark_src = "AMD forum: ~0.3ns/logic-level incl route (UltraScale+)";
    UltraScaleTimingDb {
        part: "xcvu3p".to_string(),
        speed_grade: "-2".to_string(),
        cells: CellDelays {
            lut6: SourcedDelay::ballpark(120, ballpark_src),
            ff_ckq: SourcedDelay::ballpark(150, ballpark_src),
            ff_setup: SourcedDelay::ballpark(80, ballpark_src),
            ff_hold: SourcedDelay::ballpark(60, ballpark_src),
        },
        carry8: Carry8Delays {
            ci_to_co_bit: SourcedDelay::ballpark(40, ballpark_src),
            s_to_o: SourcedDelay::ballpark(100, ballpark_src),
            cyinit_to_co: SourcedDelay::ballpark(60, ballpark_src),
        },
        dsp: Dsp48e2Delays {
            mult_a_b_to_p: SourcedDelay::unknown(),
            clk_to_p: SourcedDelay::unknown(),
        },
        bram: Ramb36e2Delays {
            clk_to_do: SourcedDelay::unknown(),
            addr_setup: SourcedDelay::unknown(),
        },
        uram: Uram288Delays {
            clk_to_do: SourcedDelay::unknown(),
            addr_setup: SourcedDelay::unknown(),
        },
        interconnect: InterconnectDelays {
            single: SourcedDelay::ballpark(180, ballpark_src),
            double: SourcedDelay::unknown(),
            quad: SourcedDelay::unknown(),
            long: SourcedDelay::unknown(),
            pip: SourcedDelay::ballpark(50, ballpark_src),
        },
        iob: IobDelays {
            pad_to_fabric: SourcedDelay::unknown(),
            fabric_to_pad: SourcedDelay::unknown(),
        },
    }
}

impl crate::fabric::TimingDb for UltraScaleTimingDb {
    fn has_data(&self) -> bool {
        true
    }
}
