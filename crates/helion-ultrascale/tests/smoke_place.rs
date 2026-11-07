//! Smoke test: place a LUT and a FF by hand through the fabric API.
//!
//! This exercises the day-1 contract the pack-place agent will code against:
//! find a CLB tile, take a LUT BEL and an FF BEL from it, and verify they are
//! distinct, correctly named per UG574, and traceable back to their tile.

use helion_ultrascale::{BelKind, FabricWithPlaceholders, TileId, TileKind, UltraScaleFabric};

/// A minimal placement record: which BEL holds which logical cell.
struct Placement {
    bel_tile: TileId,
    bel_kind: BelKind,
    bel_index: u32,
    cell: String,
}

#[test]
fn place_lut_and_ff_through_fabric_api() {
    let fabric = FabricWithPlaceholders::xcvu3p();

    // Find the first CLB tile (SLICEL).
    let clb_tile = (0..fabric.device().grid_height())
        .flat_map(|y| (0..fabric.device().grid_width()).map(move |x| TileId::new(x, y)))
        .find(|t| fabric.tile_kind(*t) == Some(TileKind::Clel))
        .expect("xcvu3p model must contain SLICEL tiles");

    // Take LUT A (index 0) and FF AQ (index 0).
    let bels = fabric.bels_at(clb_tile);
    let lut = bels
        .iter()
        .find(|b| b.kind == BelKind::Lut6 && b.index == 0)
        .expect("CLB tile must have LUT A");
    let ff = bels
        .iter()
        .find(|b| b.kind == BelKind::FdRe && b.index == 0)
        .expect("CLB tile must have FF AQ");

    // UG574 names.
    assert_eq!(lut.name(), Some("A6LUT".to_string()));
    assert_eq!(ff.name(), Some("AQ".to_string()));

    // Record the placement.
    let placements = [
        Placement {
            bel_tile: lut.tile,
            bel_kind: lut.kind,
            bel_index: lut.index,
            cell: "u_lut0".to_string(),
        },
        Placement {
            bel_tile: ff.tile,
            bel_kind: ff.kind,
            bel_index: ff.index,
            cell: "u_ff0".to_string(),
        },
    ];

    // Verify: two distinct BELs, same tile, correct kinds.
    assert_eq!(placements.len(), 2);
    assert_ne!(
        (placements[0].bel_kind, placements[0].bel_index),
        (placements[1].bel_kind, placements[1].bel_index),
        "LUT and FF must occupy distinct BELs"
    );
    assert_eq!(placements[0].bel_tile, clb_tile);
    assert_eq!(placements[1].bel_tile, clb_tile);

    // Verify the BELs are still reported at the tile (no aliasing).
    let requery = fabric.bels_at(clb_tile);
    assert!(
        requery
            .iter()
            .any(|b| b.kind == BelKind::Lut6 && b.index == 0)
    );
    assert!(
        requery
            .iter()
            .any(|b| b.kind == BelKind::FdRe && b.index == 0)
    );

    println!(
        "placed {} onto {} {} at {}",
        placements[0].cell,
        placements[0].bel_kind.unisim(),
        lut.name().unwrap(),
        clb_tile
    );
    println!(
        "placed {} onto {} {} at {}",
        placements[1].cell,
        placements[1].bel_kind.unisim(),
        ff.name().unwrap(),
        clb_tile
    );
}

#[test]
fn carry8_available_in_same_clb() {
    // The pack-place agent will need CARRY8 for arithmetic. Verify it is
    // present in the same tile as the LUT/FF above.
    let fabric = FabricWithPlaceholders::xcvu3p();
    let clb_tile = (0..fabric.device().grid_height())
        .flat_map(|y| (0..fabric.device().grid_width()).map(move |x| TileId::new(x, y)))
        .find(|t| fabric.tile_kind(*t) == Some(TileKind::Clel))
        .expect("xcvu3p model must contain SLICEL tiles");
    let carry = fabric
        .bels_at(clb_tile)
        .iter()
        .find(|b| b.kind == BelKind::Carry8)
        .expect("CLB tile must have one CARRY8 per UG574");
    assert_eq!(carry.name(), Some("CARRY8".to_string()));
}
