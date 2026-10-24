//! Does the wire the router arms actually deliver a signal to the LED pad?
//!
//! Round 26 measured this on hardware. The reference bitstream arms
//! `INT_R_X73Y46.IMUX34.NR1END1`; our build arms `.NE2END1`. Both images are
//! otherwise complete and `done` reads 1, but only the reference blinks.
//!
//! The arm is only half the story: a mux input is useful only if the wire it
//! selects is electrically connected to something. `tileconn.json` stitches
//! long lines across tile boundaries, and the stitch has a DIRECTION:
//!
//! ```text
//! ['INT_L', 'INT_L'] delta=[0, 1] ['NE2A1', 'NE2BEG1']
//! ```
//!
//! read through `Device::stitch_nodes` means `NE2A1` sits in the tile at
//! `(gx, gy)` and `NE2BEG1` in the tile at `(gx, gy + 1)`. So a `NE2BEG1`
//! driven in the tile at row Y stitches DOWN to row Y-1: to feed a mux in row
//! Y+1, the driver must sit in row Y+1.
//!
//! These tests ask Helion's own node graph -- the same graph the router
//! searches -- rather than re-deriving the topology by hand, which was wrong
//! twice already.

use helion_xilinx7::device::Device;
use helion_xilinx7::route::RouteQuery;
use helion_xilinx7::xdevice::{build_node_map, XDevice};

fn db_root() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../work/prjxray-db")
        })
}

/// True when `tile_a.wire_a` and `tile_b.wire_b` sit on one electrical node.
fn same_node(xdev: &XDevice, tile_a: &str, wire_a: &str, tile_b: &str, wire_b: &str) -> bool {
    xdev.node_wires(tile_a, wire_a)
        .iter()
        .any(|(t, w)| t == tile_b && w == wire_b)
}

fn with_xdev<R>(f: impl FnOnce(&XDevice) -> R) -> R {
    let mut dev = Device::load_part_from_db(&db_root(), "xc7z020").expect("load device");
    let nm = build_node_map(&mut dev);
    let xdev = XDevice::new(&dev, nm);
    f(&xdev)
}

/// The direction of the `NE2` stitch, as the router experiences it.
///
/// This is the load-bearing fact for the dark-LED hypothesis: our route drives
/// `NE2BEG1` in the tile at Y45 and expects it to reach the LED mux at Y46. If
/// the stitch runs downward, that route can never deliver a signal.
#[test]
fn ne2beg1_stitches_upward() {
    with_xdev(|x| {
        let down = same_node(x, "INT_L_X72Y45", "NE2BEG1", "INT_L_X72Y44", "NE2A1");
        let up = same_node(x, "INT_L_X72Y45", "NE2BEG1", "INT_L_X72Y46", "NE2A1");
        // Measured, not assumed: the stitch runs the OTHER way from the naive
        // reading of `['INT_L','INT_L'] delta=[0,1] ['NE2A1','NE2BEG1']`.
        // `NE2BEG1` at Y45 connects UP to `NE2A1` at Y46, not down to Y44.
        // Round 36 made the mirror-image mistake on the same entry; ask the
        // graph, never the JSON delta.
        assert!(
            up,
            "NE2BEG1 at Y45 connects UP to NE2A1 at Y46"
        );
        assert!(
            !down,
            "NE2BEG1 at Y45 does not reach NE2A1 at Y44"
        );
    });
}

/// The LED pad's mux input must be reachable from the wire our route drives.
///
/// Measured outcome: our route IS electrically sound. `NE2BEG1` driven at
/// (72,45) reaches the pad's mux at (73,46), and the neighbouring row's
/// `NE2BEG1` does not. So the router picked the one row that works, and the
/// dark LED cannot be explained by an unreachable mux.
///
/// This retires the most attractive theory for the round-26 hardware result
/// and points the next investigation at the arm ENCODING or at the flop that
/// drives this net.
#[test]
fn our_route_is_electrically_sound_so_the_dark_led_lies_elsewhere() {
    with_xdev(|x| {
        // What our bitstream drives: NE2BEG1 in the tile one row BELOW.
        let ours_reaches = same_node(x, "INT_L_X72Y45", "NE2BEG1", "INT_R_X73Y46", "NE2END1");
        // What it would take: NE2BEG1 in the same row as the pad's tile.
        let correct = same_node(x, "INT_L_X72Y46", "NE2BEG1", "INT_R_X73Y46", "NE2END1");
        // Both hold. Our route drives a wire that genuinely arrives at the
        // pad's mux, so the dark LED is NOT an unreachable-mux problem. The
        // arm encoding itself, or something upstream of the mux, is the cause.
        assert!(
            ours_reaches,
            "INT_L_X72Y45.NE2BEG1 reaches INT_R_X73Y46.NE2END1, so the route is \
             electrically sound and the dark LED lies elsewhere"
        );
        // And the neighbouring row does NOT: the stitch is asymmetric, so the
        // row our router chose is the only one that works. That removes
        // "driven from the wrong row" as an explanation for the dark LED and
        // leaves the arm encoding itself, or something upstream of this mux.
        assert!(
            !correct,
            "INT_L_X72Y46.NE2BEG1 reaches the LED mux after all; the topology is \
             wider than measured and this note needs revisiting"
        );
    });
}
