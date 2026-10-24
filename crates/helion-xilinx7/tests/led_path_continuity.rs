//! The routed LED path must be electrically continuous from the flop to the pad.
//!
//! Round 42 established the *route* is sound: every hop is a real pip and the
//! sink is reachable. What it did not establish -- and what the dark LED
//! turned out to hinge on -- is that the flop at the *source* end actually has
//! data. That half is asserted by `undriven_ff_guard.rs` and by the `same_bit`
//! carry linkage; this file keeps the routing half settled, so a future dark
//! LED is not misattributed to it again.
//!
//! Everything here queries Helion's own node graph rather than reading wire
//! names, because reading names is what produced the `NR1END1`-vs-`NE2END1`
//! misreading twice.

use helion_xilinx7::device::Device;
use helion_xilinx7::route::RouteQuery;
use helion_xilinx7::xdevice::{build_node_map, XDevice};

fn db_root() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .expect("HELION_XILINX_DB must point at a prjxray-db checkout")
}

/// Do two `(tile, wire)` pairs resolve to one electrical node?
///
/// Long lines span tiles through tileconn stitches, which are not pips, so
/// this is the only sound way to ask whether a begin wire connects upward.
fn same_node(x: &XDevice, ta: &str, wa: &str, tb: &str, wb: &str) -> bool {
    if ta == tb && wa == wb {
        return true;
    }
    // `node_wires` returns the OTHER members of the node, not including the
    // queried pair -- so containment is the test, not set intersection.
    x.node_wires(ta, wa)
        .iter()
        .any(|(t, w)| t == tb && w == wb)
}

fn with_xdev<R>(f: impl FnOnce(&XDevice) -> R) -> R {
    let mut dev = Device::load_part_from_db(&db_root(), "xc7z020").expect("load device");
    let nm = build_node_map(&mut dev);
    let xdev = XDevice::new(&dev, nm);
    f(&xdev)
}

/// Every arm the round-26 hardware test exercised must be selectable into the
/// pad's mux.
///
/// `NR1END1` lights LD0; `WW2END0` leaves it dark. If the model could not
/// select one of them, the hardware result would be unexplainable and the arm
/// choice would not be the router's to make.
#[test]
fn every_measured_arm_reaches_the_pad_mux() {
    with_xdev(|x| {
        for arm in ["NR1END1", "NE2END1", "NW2END1", "WW2END0"] {
            let pips = x.pips_from("INT_R_X73Y46", arm);
            assert!(
                pips.iter().any(|(dst, _, _)| dst == "IMUX34"),
                "`{arm}` must be able to drive IMUX34 on the LED pad's tile"
            );
        }
    });
}

/// The two hops Helion takes to reach the pad must both exist.
///
/// This is the continuity property the dark LED was blamed on for several
/// rounds. Our route drives `NR1BEG1` from `EE2END1` where the reference uses
/// `ER1END1`; both are legitimate in-tile sources of the same wire, so the
/// assertion is on the wire reached, not on the feeder.
#[test]
fn the_column_reaches_the_pad_mux() {
    with_xdev(|x| {
        let into_beg = x.pips_from("INT_R_X73Y45", "EE2END1");
        assert!(
            into_beg.iter().any(|(dst, _, _)| dst == "NR1BEG1"),
            "EE2END1 must be able to drive NR1BEG1 -- the turn Helion takes"
        );
        // `NR1BEG1` is a *begin* wire: it has no in-tile pips out. The hop to
        // `NR1END1` one row up is a tileconn stitch, so it must be checked
        // through the node graph. `pips_from` returns [] here, and reading
        // that as "the hop does not exist" is what misled round 36 -- twice.
        // tileconn says `['INT_R','INT_R'] delta=[0,-1] ['NR1BEG1','NR1END1']`,
        // and a non-zero delta anchors on the SECOND wire's tile: the begin
        // wire sits one row ABOVE the end wire, not below. Reading the pair as
        // "begin is the lower one" is the round-36 inversion, which has now
        // been made in both directions -- so probe rather than assume.
        // The stitch is upward: `NR1BEG1` at Y45 *is* `NR1END1` at Y46. The
        // tileconn entry `['INT_R','INT_R'] delta=[0,-1] ['NR1BEG1','NR1END1']`
        // reads the other way if the anchor is taken to be the first wire, and
        // that misreading has now happened in both directions across rounds 36
        // and 42. Ask the node map instead -- it is the thing that decides.
        assert!(
            same_node(x, "INT_R_X73Y45", "NR1BEG1", "INT_R_X73Y46", "NR1END1"),
            "NR1BEG1 at Y45 must be the same node as NR1END1 at Y46 -- this is \
             the turn Helion's route takes"
        );
        let pad = x.pips_from("INT_R_X73Y46", "NR1END1");
        assert!(
            pad.iter().any(|(dst, _, _)| dst == "IMUX34"),
            "NR1END1 must reach the pad's mux input"
        );
    });
}

/// The reference's own feeder must work too, or the two routes would not be
/// interchangeable and the difference would matter after all.
#[test]
fn the_reference_feeder_is_also_legitimate() {
    with_xdev(|x| {
        let pips = x.pips_from("INT_R_X73Y45", "ER1END1");
        assert!(
            pips.iter().any(|(dst, _, _)| dst == "NR1BEG1"),
            "ER1END1 must also drive NR1BEG1"
        );
    });
}
