//! Clock-distribution raw codes (ZB-10): HROW word-50 bits that no segbits
//! feature names, measured from real Vivado builds on the USF cluster.
//!
//! SCOPE AND PROVENANCE. 12 single-trunk samples (cnt8/blinky designs,
//! GCLK trunks 0-3, BOT-region and TOP-region taps through MUX_OUT_R8,
//! Vivado 2017.3) plus cross-checks: the same topologies on 2022.1, three
//! place/route directive variants, two extra BOT rows, and the 2023.2
//! oracle itself. Every sample below agrees across versions and
//! directives for its (trunk, tap tile) cell, including the oracle.
//! Multi-trunk designs (ck3 sample: 4 trunks) use a different, denser
//! code this table does not cover; BUFH-fed clocks (bufh sample) set no
//! trunk bits at all. Trunks above 3 are unmeasured (samples stranded
//! by a dropped cluster tunnel) and gated out, never guessed.
//!
//! WHAT THE BITS ARE. Word 50 low 14 bits of HROW minors 26-28 (BOT
//! tile) and minor 27 (TOP tile) plus the tap-home minor (28 for even
//! trunks, 29 for odd) in whichever tile carries the tap mux. The ACTIVE
//! bits and mux-select bits in the same words ARE segbits-named and
//! stay on the FASM path; only the unnamed low-14 distribution codes
//! live here. Returned as `(tile, minor, word, mask)` tuples applied
//! with OR semantics via
//! [`crate::frames::FrameStore::apply_raw_bits`]; tile names come from
//! the routed spine, frame bases from the tilegrid layout, so nothing
//! here hardcodes a frame address.

/// Which HROW tile carries the tap mux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HrowTile {
    Bot,
    Top,
}

/// A clock tap: HROW tile, L/R side, and mux index (e.g. R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HrowTap {
    pub tile: HrowTile,
    pub side_r: bool,
    pub mux: u8,
}

/// Raw word-50 codes for one (trunk, tap) cell: `(minor, word, mask)`.
/// The caller resolves tile names (BOT/TOP from the routed spine) and
/// frame bases (tilegrid) around these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HrowCodes {
    /// BOT tile minor 26 word 50 low 14 bits (trunk entry region).
    pub bot_26: u32,
    /// BOT tile minor 27 word 50 low 14 bits (trunk entry region).
    pub bot_27: u32,
    /// TOP tile minor 27 word 50 low 14 bits (through traffic or tap stubs).
    pub top_27: u32,
    /// Tap-home minor (28 even trunks, 29 odd) and tap code.
    pub tap_minor: u8,
    pub tap_code: u32,
}

/// The measured xc7z020 HROW tile pair: every sample programmed exactly
/// these two tiles (the second TOP tile, X87Y130, stayed zero throughout).
/// Callers must resolve frame bases through the tilegrid layout and let
/// unknown tiles fail loudly; never compute frame addresses by hand.
pub fn hrow_tile_names() -> (&'static str, &'static str) {
    ("CLK_HROW_BOT_R_X87Y26", "CLK_HROW_TOP_R_X87Y78")
}

/// Leaf-buffer distribution codes for the trunk's HCLK leaf buffer
/// (word 50 low bits of two HCLK tiles). Measured on the same BOT-tap
/// samples as [`hrow_codes`]: trunks 0 and 1 agree exactly (further
/// trunks stranded by a dropped cluster tunnel), TOP taps use different
/// leaf tiles outside this table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeafCodes {
    /// HCLK_L_X174Y26 minor 3 word 50 low bits (buffer input code).
    pub l174_m03: u32,
    /// HCLK_L_X174Y26 minor 5 word 50 low bits (buffer tap code).
    pub l174_m05: u32,
    /// HCLK_CMT_L_X178Y26 minor 27 word 50 low bits (CMT feed code).
    pub cmtl_m27: u32,
}

/// HCLK tiles carrying [`LeafCodes`], in field order.
pub fn leaf_tile_names() -> (&'static str, &'static str) {
    ("HCLK_L_X174Y26", "HCLK_CMT_L_X178Y26")
}

pub fn leaf_codes(gclk: u8, tap: &HrowTap) -> Option<LeafCodes> {
    if tap.tile != HrowTile::Bot || tap.side_r != true || tap.mux != 8 {
        return None;
    }
    match gclk {
        0 | 1 => Some(LeafCodes {
            l174_m03: 0x9b9,
            l174_m05: 0x19b7,
            cmtl_m27: 0x9b5,
        }),
        _ => None,
    }
}

/// Clock-input distribution codes (HCLK_IOI3_X1Y26 word 50): programmed
/// identically by every sample (8 builds across 2017.3, 2022.1, oracle;
/// single- and multi-clock; BOT- and TOP-region loads; BUFG and BUFH
/// clocks) because all share the same Y9 clock-pin input path. Gated to
/// single-clock designs like the rest of this module.
pub fn input_tile_name() -> &'static str {
    "HCLK_IOI3_X1Y26"
}

/// `(minor, word, full-word value)` for the clock-input tile. Full
/// values (not just low bits): every sample agrees bit-for-bit here.
pub fn input_codes() -> [(u32, u32, u32); 3] {
    [(29, 50, 0x1a2d), (38, 50, 0x0a3e), (39, 50, 0x1a39)]
}

/// Measured HROW word-50 distribution codes for a single-GCLK-trunk clock
/// through the given tap. Returns `None` outside the measured domain
/// (trunk above 3, tap mux other than R8, or no trunk): the caller must
/// leave those bits unset, exactly today's behavior, never guess.
pub fn hrow_codes(gclk: u8, tap: &HrowTap) -> Option<HrowCodes> {
    if tap.side_r != true || tap.mux != 8 {
        return None;
    }
    // Tap code: trunk pairs share it (0,1 -> 0x19b7; 2,3 -> 0x19b8),
    // in the parity minor of the tap tile. Verified on all 12 samples.
    let tap_code = match gclk {
        0 | 1 => 0x19b7,
        2 | 3 => 0x19b8,
        _ => return None,
    };
    let tap_minor = 28 + (gclk % 2);
    let (bot_26, bot_27, top_27) = match (gclk, tap.tile) {
        // BOT-region taps (blinky/oracle topology is (0, Bot)).
        (0, HrowTile::Bot) => (0x1cb2, 0x10cf, 0x0643),
        (1, HrowTile::Bot) => (0x16ae, 0x0ac3, 0x1653),
        (2, HrowTile::Bot) => (0x1c93, 0x10cf, 0x1663),
        (3, HrowTile::Bot) => (0x068e, 0x1ac2, 0x0673),
        // TOP-region taps.
        (0, HrowTile::Top) => (0x06be, 0x1c8d, 0x0ac0),
        (1, HrowTile::Top) => (0x16ae, 0x0c9d, 0x0ac0),
        (2, HrowTile::Top) => (0x169e, 0x0cad, 0x0ac0),
        (3, HrowTile::Top) => (0x068e, 0x1cbd, 0x0ac0),
        _ => return None,
    };
    Some(HrowCodes {
        bot_26,
        bot_27,
        top_27,
        tap_minor,
        tap_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r8(tile: HrowTile) -> HrowTap {
        HrowTap {
            tile,
            side_r: true,
            mux: 8,
        }
    }

    /// Every measured (trunk, tap tile) cell reproduces its samples.
    /// Cells: G0-3 x BOT/TOP through R8; samples: control/dr-place/
    /// dr-route/dr-xnd/bufgloc/sw-r44/sw-r10 (BOT) and sw-r100/sw-r140/
    /// sw-t1/sw-t2/sw-t3 (TOP), cross-checked on 2022.1 and the oracle.
    #[test]
    fn table_reproduces_all_measured_cells() {
        let cases: &[(u8, HrowTile, u32, u32, u32, u8, u32)] = &[
            (0, HrowTile::Bot, 0x1cb2, 0x10cf, 0x0643, 28, 0x19b7),
            (1, HrowTile::Bot, 0x16ae, 0x0ac3, 0x1653, 29, 0x19b7),
            (2, HrowTile::Bot, 0x1c93, 0x10cf, 0x1663, 28, 0x19b8),
            (3, HrowTile::Bot, 0x068e, 0x1ac2, 0x0673, 29, 0x19b8),
            (0, HrowTile::Top, 0x06be, 0x1c8d, 0x0ac0, 28, 0x19b7),
            (1, HrowTile::Top, 0x16ae, 0x0c9d, 0x0ac0, 29, 0x19b7),
            (2, HrowTile::Top, 0x169e, 0x0cad, 0x0ac0, 28, 0x19b8),
            (3, HrowTile::Top, 0x068e, 0x1cbd, 0x0ac0, 29, 0x19b8),
        ];
        for &(g, tile, b26, b27, t27, tm, tc) in cases {
            let got = hrow_codes(g, &r8(tile)).expect("measured cell");
            assert_eq!(
                got,
                HrowCodes {
                    bot_26: b26,
                    bot_27: b27,
                    top_27: t27,
                    tap_minor: tm,
                    tap_code: tc,
                },
                "cell gclk={g} tile={tile:?}"
            );
        }
    }

    /// Leaf-buffer cells reproduce the BOT-tap samples for trunks 0-1.
    #[test]
    fn leaf_table_reproduces_measured_cells() {
        for g in [0u8, 1] {
            let got = leaf_codes(g, &r8(HrowTile::Bot)).expect("leaf cell");
            assert_eq!(
                got,
                LeafCodes {
                    l174_m03: 0x9b9,
                    l174_m05: 0x19b7,
                    cmtl_m27: 0x9b5,
                },
                "leaf cell gclk={g}"
            );
        }
        assert!(leaf_codes(2, &r8(HrowTile::Bot)).is_none(), "trunk 2 leaf");
        assert!(leaf_codes(0, &r8(HrowTile::Top)).is_none(), "TOP leaf");
    }

    /// Input codes are fixed full words shared by every sample.
    #[test]
    fn input_codes_are_fixed() {
        assert_eq!(
            input_codes(),
            [(29, 50, 0x1a2d), (38, 50, 0x0a3e), (39, 50, 0x1a39)]
        );
    }

    /// Outside the measured domain the lookup refuses: trunk 4+,
    /// non-R8 muxes, and L-side taps have no samples.
    #[test]
    fn unmeasured_domain_gated_out() {
        assert!(hrow_codes(4, &r8(HrowTile::Bot)).is_none(), "trunk 4");
        assert!(hrow_codes(0, &HrowTap { tile: HrowTile::Bot, side_r: true, mux: 9 }).is_none(), "mux R9");
        assert!(hrow_codes(0, &HrowTap { tile: HrowTile::Bot, side_r: false, mux: 0 }).is_none(), "mux L0");
    }
}
