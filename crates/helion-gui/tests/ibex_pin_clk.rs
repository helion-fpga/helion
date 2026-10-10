use helion_gui::IdeModel;
use std::path::PathBuf;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// FM-HEL-LOUD (10-design benchmark, 2026-10-10): Ibex packs ~11.6k LUTFFs
/// and 36 BRAMs against HL10T-C32-1's 8,192 LUT6 / 8 BRAM18. The old flow
/// silently clipped it into a non-functional bitstream; implement must now
/// fail loudly at the pack→place boundary instead — never clip, never ship.
#[test]
fn ibex_pin_wrap_over_capacity_fails_loud() {
    let mut ide = IdeModel::new();
    ide.open_source(&example("ibex_pin_wrap.prj"))
        .expect("open ibex_pin_wrap.prj");
    let impl_r = ide.implement();
    eprintln!("ibex implement={impl_r:?}");
    let err =
        impl_r.expect_err("over-capacity Ibex must fail loudly, never clip silently");
    assert!(
        err.contains("over capacity") && err.contains("LUT6"),
        "implement must name the over-capacity resource with counts, got: {err}"
    );
}
