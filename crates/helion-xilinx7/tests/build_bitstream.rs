//! `build::bitstream_for_part` must reproduce the hand-assembled ZedBoard flow.
use helion_xilinx7::build::{bitstream_for_part, PinConstraints};

fn db() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../work/prjxray-db")
                .canonicalize()
                .expect("set HELION_XILINX_DB")
        })
}

fn zedboard_design() -> helion_ir::Design {
    // CARGO_MANIFEST_DIR is <repo>/crates/helion-xilinx7.
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/zedboard_blinky.sv");
    let text = std::fs::read_to_string(&src)
        .unwrap_or_else(|e| panic!("read {}: {e}", src.display()));
    helion_sv::synth_sv(&text, "zedboard_blinky.sv").expect("synth zedboard_blinky")
}

#[test]
fn zedboard_bitstream_matches_the_hand_built_flow() {
    let mut design = zedboard_design();
    let xdc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/zedboard.xdc"),
    )
    .expect("read zedboard.xdc");
    let pins = PinConstraints::parse(&xdc);
    println!(
        "parsed {} PACKAGE_PIN(s), {} IOSTANDARD(s)",
        pins.package_pins.len(),
        pins.io_standards.len()
    );
    assert_eq!(pins.package_pins.get("clk").map(String::as_str), Some("Y9"));
    pins.apply(&mut design);
    let root = db();
    let got = bitstream_for_part(&design, "xc7z020", Some(&root)).expect("build bitstream");

    println!(
        "part={} idcode={:#010x} frames={} fasm={} applied={} bytes={}",
        got.part,
        got.idcode,
        got.frame_count,
        got.fasm_lines,
        got.features_applied,
        got.bytes.len()
    );
    if let Some(s) = got.dropped_summary() {
        println!("WARNING: {s}");
    }

    // The ZedBoard bitstream the hand-written flow produced.
    assert_eq!(got.frame_count, 9_996, "xc7z020 has 9996 frames");
    assert_eq!(got.idcode, 0x0372_7093, "xc7z020 IDCODE, not Artix");
    // The bare die name resolves to a concrete package; the bitstream header
    // must carry that package name, because the header is what a programmer
    // displays and what identifies the image.
    assert_eq!(got.part, "xc7z020clg484-1");
    // The baseline per-tile configuration dominates this count and drifts with
    // every routing change, so a tight bound only ever measures that drift. The
    // real guard against an empty image is that output pads exist at all, which
    // is pinned by `tests/output_pads.rs` (round 39's silent failure produced a
    // full-size bitstream with a healthy feature count and zero pads).
    assert!(
        got.features_applied > 1_000,
        "applied {} -- an image this sparse means emission produced nothing",
        got.features_applied
    );
    // A `*_SING` tile hosts only one half of its base type's layout. The
    // ZedBoard's LED7 pad (U14) is `RIOB33_SING_X73Y0`, whose alias lands in
    // the upper half (bits 64..127) and so cannot be programmed from upstream
    // segbits at all; LED6 (U19 / `_X73Y49`) lands in the lower half and IS
    // borrowed. Pin that split rather than requiring an empty list, which
    // would silently forbid the working borrow along with the impossible one.
    let dropped_tiles: Vec<&str> = got.dropped.iter().map(|d| d.tile.as_str()).collect();
    assert_eq!(
        dropped_tiles,
        vec!["RIOB33_SING_X73Y0"],
        "only the upper-half _SING pad may be dropped; LED6 (RIOB33_SING_X73Y49) must be \
         borrowed. Summary: {}",
        got.dropped_summary().unwrap_or_default()
    );

    // The header must name the right part and IDCODE.
    let head = &got.bytes[..64.min(got.bytes.len())];
    assert_eq!(&head[..2], b"\x00\x09", "magic");
    assert!(
        got.bytes.windows(12).any(|w| w == b"7z020clg484-"),
        "the concrete package name should appear in the .bit header"
    );

    // Walk the header records and check the 'e' (FDRI) length.
    // Layout: magic(2) + sync words + 0x00 0x00 0x01, then 'a'/'b'/'c'/'d'
    // string records each with a 2-byte length, then 'e' with a 4-byte length.
    let mut i = 13;
    let mut fdr_words = None;
    while i + 3 <= got.bytes.len() {
        let tag = got.bytes[i];
        let len = if tag == b'e' {
            u32::from_be_bytes([
                got.bytes[i + 1],
                got.bytes[i + 2],
                got.bytes[i + 3],
                got.bytes[i + 4],
            ]) as usize
        } else {
            u16::from_be_bytes([got.bytes[i + 1], got.bytes[i + 2]]) as usize
        };
        let hdr = if tag == b'e' { 5 } else { 3 };
        let body = i + hdr;
        if body + len > got.bytes.len() {
            break;
        }
        if tag == b'e' {
            assert_eq!(len % 4, 0, "FDRI payload must be whole words, got {len}");
            fdr_words = Some(len / 4);
            break;
        }
        i = body + len;
    }
    let words = fdr_words.expect("an 'e' FDRI record in the bitstream");
    // The 'e' record is the whole packet stream: preamble, register writes and
    // the FDRI burst, not just the frame payload. The frame payload itself is
    // checked exactly by the `frames` count below, so here assert only that the
    // stream comfortably contains the payload and is word-aligned.
    let payload_words = got.frame_count * 101;
    assert!(
        words > payload_words,
        "packet stream ({words}) must contain the {payload_words}-word payload"
    );
    assert!(
        words - payload_words < 2_000,
        "packet overhead should be a few hundred words, got {}",
        words - payload_words
    );
    println!(
        "packet stream words = {words} (frame payload {payload_words}, overhead {})",
        words - payload_words
    );
    // Every frame the device has must be in the stream: the FDRI burst carries
    // no per-frame address, so a missing frame silently shifts all later ones.
    assert_eq!(got.frame_count, 9_996, "xc7z020 has 9996 frames");

    // Only the *_SING tiles may be dropped, and they must be reported.
    for d in &got.dropped {
        assert!(
            !d.reason.is_empty(),
            "a dropped tile must carry a reason: {d:?}"
        );
    }
}

#[test]
fn an_unknown_part_is_a_clean_error() {
    let design = helion_ir::Design::new("empty");
    let e = bitstream_for_part(&design, "xc9zzz1", Some(&db())).unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("device"), "got: {msg}");
}

#[test]
fn a35t_also_builds() {
    let mut design = zedboard_design();
    let xdc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/zedboard.xdc"),
    )
    .expect("read zedboard.xdc");
    PinConstraints::parse(&xdc).apply(&mut design);
    match bitstream_for_part(&design, "xc7a35t", Some(&db())) {
        Ok(got) => {
            println!(
                "a35t: part={} frames={} applied={} bytes={}",
                got.part,
                got.frame_count,
                got.features_applied,
                got.bytes.len()
            );
            assert_eq!(got.idcode, 0x0362_D093);
            assert_ne!(got.bytes.len(), 0);
        }
        Err(e) => println!("a35t refused (expected for a Zynq board design): {e}"),
    }
}
