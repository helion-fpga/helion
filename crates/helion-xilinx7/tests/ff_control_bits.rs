//! FF control-signal bit verification (workstream ffctrl).
//!
//! The FASM that [`emit_fasm`](helion_xilinx7::emit::emit_fasm) produces for
//! flip-flops with CE/SR nets must resolve through the real X-Ray segbits
//! DB to exactly the documented configuration bits:
//!
//! ```text
//! CLBLL_L.SLICEL_X0.CEUSEDMUX 01_39
//! CLBLL_L.SLICEL_X0.SRUSEDMUX 01_35
//! CLBLL_L.SLICEL_X0.AFF.ZRST  30_12
//! ```
//!
//! Requires the X-Ray zynq7 DB at `~/workspace/xilinx-dbsrc` (or
//! `HELION_XILINX_DB`), same as `zedboard_e2e`.

use helion_xilinx7::device::default_db_root;
use helion_xilinx7::emit::{emit_fasm, EmitDevice};
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{
    BlockType, FrameAddr, FrameStore, SegBitsDb, TileBitLayout,
};
use helion_xilinx7::pack::{Packed7, PackedFf};
use helion_xilinx7::place::{Placed7, PlacedFf};
use helion_xilinx7::route::{RouteKind, Routed7, RoutedNet7};

struct StubEmit;
impl EmitDevice for StubEmit {
    fn tile_type(&self, _tile: &str) -> Option<String> {
        Some("CLBLL_L".into())
    }
    fn site_rel(&self, tile: &str, _site: &str) -> Option<String> {
        if tile.starts_with("CLBLL") {
            Some("SLICEL_X0".into())
        } else {
            None
        }
    }
}

/// One AFF with optional CE/SR nets, no LUT pairing.
fn routed_ff(ce: Option<&str>, sr: Option<&str>) -> Routed7 {
    let packed = Packed7 {
        ffs: vec![PackedFf {
            name: "u_ff".into(),
            d_net: "d".into(),
            q_net: "q".into(),
            clk_net: "clk".into(),
            ce_net: ce.map(str::to_string),
            sr_net: sr.map(str::to_string),
        }],
        ..Default::default()
    };
    let placed = Placed7 {
        packed: packed.clone(),
        ffs: vec![PlacedFf {
            name: "u_ff".into(),
            tile: "CLBLL_L_X0Y0".into(),
            site: "SLICE_X0Y0".into(),
            bel: "AFF".into(),
        }],
        ..Default::default()
    };
    Routed7 {
        placed,
        nets: vec![RoutedNet7 {
            net: "d".into(),
            kind: RouteKind::SiteInternal,
        }],
    }
}

struct StubLayout;
impl TileBitLayout for StubLayout {
    fn frame_addr(&self, tile: &str, _block: BlockType) -> Option<FrameAddr> {
        if tile == "CLBLL_L_X0Y0" {
            Some(FrameAddr {
                baseaddr: 0,
                offset: 0,
            })
        } else {
            None
        }
    }
    fn tile_type(&self, tile: &str) -> Option<&str> {
        if tile == "CLBLL_L_X0Y0" {
            Some("CLBLL_L")
        } else {
            None
        }
    }
}

fn load_clbll_segbits() -> SegBitsDb {
    let path = default_db_root().join("zynq7/segbits_clbll_l.db");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("need X-Ray DB at {}: {e}", path.display()));
    let mut db = SegBitsDb::new();
    db.add_segbits("CLBLL_L", BlockType::ClbIoClk, &text)
        .expect("parse segbits_clbll_l.db");
    db
}

#[test]
fn ff_control_features_hit_documented_db_bits() {
    let db = load_clbll_segbits();
    // Exact (frame_offset, bitpos, invert) triples from segbits_clbll_l.db.
    for (feature, want) in [
        ("SLICEL_X0.CEUSEDMUX", (1u32, 39u32, false)),
        ("SLICEL_X0.SRUSEDMUX", (1u32, 35u32, false)),
        ("SLICEL_X0.AFF.ZRST", (30u32, 12u32, false)),
    ] {
        let (block, bits) = db
            .feature_bits("CLBLL_L", feature, None)
            .unwrap_or_else(|| panic!("{feature} missing from segbits_clbll_l.db"));
        assert_eq!(block, BlockType::ClbIoClk, "{feature}");
        assert_eq!(bits.len(), 1, "{feature}");
        assert_eq!(
            (bits[0].frame_offset, bits[0].bitpos, bits[0].invert),
            want,
            "{feature}"
        );
    }
}

#[test]
fn emitted_fasm_sets_documented_bits() {
    let db = load_clbll_segbits();
    let bs = emit_fasm(&routed_ff(Some("ce"), Some("rst")), &StubEmit).unwrap();
    let text = bs.fasm.join("\n");
    assert!(
        text.contains("CLBLL_L_X0Y0.SLICEL_X0.CEUSEDMUX"),
        "missing CEUSEDMUX:\n{text}"
    );
    assert!(
        text.contains("CLBLL_L_X0Y0.SLICEL_X0.SRUSEDMUX"),
        "missing SRUSEDMUX:\n{text}"
    );
    assert!(
        text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZRST"),
        "missing ZRST:\n{text}"
    );

    let lines = parse_fasm(&text).expect("parse emitted FASM");
    let mut store = FrameStore::new(&[0x01, 0x1E, 0x1F]);
    let applied = store
        .apply_fasm(&lines, &db, &StubLayout)
        .expect("apply FASM");
    assert!(applied > 0, "no features applied");

    // CEUSEDMUX 01_39 -> frame 0x01, word 1, bit 7.
    // SRUSEDMUX 01_35 -> frame 0x01, word 1, bit 3.
    let w1 = store.frame_words(0x01).unwrap();
    assert_eq!(w1[1], (1 << 7) | (1 << 3), "frame 0x01 word 1 = {:#010x}", w1[1]);
    assert_eq!(w1[0], 0, "frame 0x01 word 0 touched: {:#010x}", w1[0]);

    // AFF.ZRST 30_12 -> frame 0x1E, word 0, bit 12.
    // AFFMUX.AX (!30_00 30_01 !30_02 !30_03) also sets bit 1 here.
    let w1e = store.frame_words(0x1E).unwrap();
    assert_eq!(
        w1e[0],
        (1 << 12) | (1 << 1),
        "frame 0x1E word 0 = {:#010x}",
        w1e[0]
    );

    // AFF.ZINI 31_03 -> frame 0x1F, word 0, bit 3 (FF inits to 0).
    let w1f = store.frame_words(0x1F).unwrap();
    assert_eq!(w1f[0], 1 << 3, "frame 0x1F word 0 = {:#010x}", w1f[0]);
}

#[test]
fn default_ff_leaves_control_bits_clear() {
    // The blinky case: no CE/SR nets. The mux enables must stay clear, which
    // is the correct default: CE tied high (FF always clock-enabled), SR
    // inactive. ZRST is don't-care with SRUSEDMUX clear; it must be untouched.
    let db = load_clbll_segbits();
    let bs = emit_fasm(&routed_ff(None, None), &StubEmit).unwrap();
    let text = bs.fasm.join("\n");
    assert!(!text.contains("CEUSEDMUX"), "unexpected CEUSEDMUX:\n{text}");
    assert!(!text.contains("SRUSEDMUX"), "unexpected SRUSEDMUX:\n{text}");
    assert!(!text.contains("ZRST"), "unexpected ZRST:\n{text}");

    let lines = parse_fasm(&text).expect("parse emitted FASM");
    let mut store = FrameStore::new(&[0x01, 0x1E, 0x1F]);
    store
        .apply_fasm(&lines, &db, &StubLayout)
        .expect("apply FASM");
    assert_eq!(
        store.frame_words(0x01).unwrap()[1],
        0,
        "mux enable bits set without CE/SR"
    );
    assert_eq!(
        store.frame_words(0x1E).unwrap()[0] & (1 << 12),
        0,
        "ZRST set without SR"
    );
}
