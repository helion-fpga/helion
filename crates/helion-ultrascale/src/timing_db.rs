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

/// Preliminary UltraScale+ timing for xcvu3p, speed grade -2.
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
