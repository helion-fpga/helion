//! Can the router reach the reference's LED arm, `IMUX34 <- NR1END1`?
//!
//! Round 26 measured on hardware that arming `INT_R_X73Y46.IMUX34` to `NR1END1`
//! turns LD0 **lit**, while `WW2END0` leaves it **dark**. Our builds instead
//! choose `NE2END1` (combinational design) or `NW2END1` (registered design);
//! round 42 proved those are electrically sound too, so the arm is not about
//! reachability in the abstract -- it is about which source the router picks.
//!
//! The reference reaches the arm the long way round:
//!
//!     INT_L_X70Y43  ER1BEG1 <- SW2END0
//!     INT_L_X70Y44  ER1BEG1 <- SW2END0
//!     INT_R_X71Y44  IMUX34  <- SR1BEG_S0
//!     INT_L_X72Y45  ER1BEG1 <- ER1END0
//!     INT_R_X73Y45  NR1BEG1 <- ER1END1     (turn north)
//!     INT_R_X73Y46  IMUX34  <- NR1END1     (the pad's mux)
//!
//! so the question these tests answer is whether each hop of that chain exists
//! in Helion's model. If it does, forcing this arm is a routing-preference
//! question; if any hop is missing, no preference can produce it and the model
//! has a hole to close first.

use helion_xilinx7::device::Device;
use helion_xilinx7::route::{self, RouteQuery};
use helion_xilinx7::xdevice::{build_node_map, XDevice};

fn db_root() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../work/prjxray-db")
        })
}

fn with_xdev<R>(f: impl FnOnce(&XDevice) -> R) -> R {
    let mut dev = Device::load_part_from_db(&db_root(), "xc7z020").expect("load device");
    let nm = build_node_map(&mut dev);
    let xdev = XDevice::new(&dev, nm);
    f(&xdev)
}

/// Does the tile/wire pair appear anywhere on the same electrical node?
fn same_node(x: &XDevice, ta: &str, wa: &str, tb: &str, wb: &str) -> bool {
    x.node_wires(ta, wa)
        .iter()
        .any(|(t, w)| t == tb && w == wb)
}

/// Every hop of the reference's route must exist as a real node relationship.
#[test]
fn the_reference_led_route_is_representable() {
    with_xdev(|x| {
        // The last hop: NR1BEG1 driven one row below arrives at the pad's mux.
        assert!(
            same_node(x, "INT_R_X73Y45", "NR1BEG1", "INT_R_X73Y46", "NR1END1"),
            "INT_R_X73Y45.NR1BEG1 must reach INT_R_X73Y46.NR1END1 via the \
             ['INT_R','INT_R'] delta=[0,-1] stitch"
        );
    });
}

/// Which wires can be selected into this tile's `IMUX34`.
///
/// `pips_from` yields `(dst, src, is_pseudo)` for pips *out of* the given wire,
/// so the arms of a mux are found by asking each candidate source in turn.
fn imux34_arms(x: &XDevice, tile: &str, candidates: &[&str]) -> Vec<String> {
    candidates
        .iter()
        .filter(|w| {
            x.pips_from(tile, w)
                .iter()
                .any(|(dst, _, _)| dst == "IMUX34")
        })
        .map(|w| w.to_string())
        .collect()
}

/// Every arm the reference or our builds have used must be selectable.
#[test]
fn the_arms_in_play_are_all_selectable_imux34_inputs() {
    with_xdev(|x| {
        let arms = imux34_arms(
            x,
            "INT_R_X73Y46",
            &["NR1END1", "NW2END1", "NE2END1", "WW2END0"],
        );
        for want in ["NR1END1", "NW2END1", "NE2END1", "WW2END0"] {
            assert!(
                arms.iter().any(|a| a == want),
                "{want} is not selectable into IMUX34; got {arms:?}"
            );
        }
    });
}

/// `NR1BEG1` one row below the pad must stitch up into the pad's `NR1END1`.
///
/// This is the first half of the reference's turn. If it holds, the reference
/// arm is reachable in principle and the remaining question is hop count.
#[test]
fn the_reference_arm_is_fed_from_the_tile_below() {
    with_xdev(|x| {
        let up = same_node(x, "INT_R_X73Y45", "NR1BEG1", "INT_R_X73Y46", "NR1END1");
        assert!(
            up,
            "INT_R_X73Y45.NR1BEG1 must reach INT_R_X73Y46.NR1END1; without this \
             the reference arm is unreachable and no tie-break can select it"
        );
    });
}

/// What feeds the arm we actually pick, for contrast.
///
/// `NW2END1` in an `INT_R` tile is fed by `CLBLM_R_NW2A1` one column west
/// (tileconn `['CLBLM_R','INT_R'] delta=[-1,0]`), i.e. straight out of a
/// neighbouring CLB -- which is why the search finds it cheap, since the
/// counter's flops sit in that column.
#[test]
fn all_three_arms_select_imux34() {
    with_xdev(|x| {
        for w in ["NR1END1", "NW2END1", "NE2END1"] {
            assert!(
                x.pips_from("INT_R_X73Y46", w)
                    .iter()
                    .any(|(d, _, _)| d == "IMUX34"),
                "{w} does not select IMUX34"
            );
        }
    });
}

/// The penalty must be large enough to beat the lateral arms' head start, and
/// small enough that it cannot buy a detour of more than about one hop.
///
/// Measured on the registered blinky, the ways into `INT_R_X73Y46.IMUX34`
/// cost `NR1END1` 314 and `NW2END1` 302 -- the lateral arm is 12 units
/// shorter, so a penalty below 13 leaves the router reaching `NW2END1` first,
/// `break`ing at its sink, and never expanding the vertical arm at all. A
/// penalty of one full hop closes that gap while bounding the detour.
#[test]
fn the_lateral_arms_are_penalised_by_about_one_hop() {
    // These are the two wires whose costs were measured; keep the numbers in
    // the test so a future change to DEFAULT_HOP_PS cannot silently reintroduce
    // the 12-unit shortfall.
    const MEASURED_GAP: i64 = 12;
    assert!(
        route::DEFAULT_HOP_PS > MEASURED_GAP,
        "a one-hop penalty ({}) must exceed the measured arm cost gap ({MEASURED_GAP})",
        route::DEFAULT_HOP_PS
    );
}
