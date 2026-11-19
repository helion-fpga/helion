//! Router-to-PIP translator: routed wire-ID paths to INT PIP FASM lines.
//!
//! This is the WireDb seam from INTERFACES.md: the router emits wire-ID
//! paths (`UltraRouted { routes: Vec<Vec<WireId>> }`) and the int-pip
//! emitter consumes named PIPs (`IntPip { tile, src, dst }`). The
//! translator maps each consecutive wire pair in a route to an `IntPip`
//! via `wire_info`, in physical signal-flow order (driver to sink, the
//! order `route_ultra` produces).
//!
//! Rules, all fail-loud:
//! - Any wire `wire_info` cannot name stops translation with
//!   [`RouteToPipError::UnnamedWire`]. Mock-routed designs can never
//!   feed int-pip emission: the mock has grid positions only, no names,
//!   and that is expected, not a bug.
//! - A consecutive pair that is not an edge in the graph stops
//!   translation with [`RouteToPipError::NotConnected`], naming both
//!   wires. In particular a multi-sink net whose route is a flattened
//!   union of per-sink paths (see `route_ultra`) is not an ordered
//!   path, so its flattened list fails here rather than emitting PIPs
//!   for jumps that do not exist. Single-sink routes are ordered paths
//!   and translate cleanly.
//! - A consecutive pair in different tiles is a span-wire continuation
//!   (BEG in one tile to END in the tile LEN steps away), not a PIP:
//!   it emits nothing and is counted in `continuations`. A cross-tile
//!   pair that does not fit the continuation naming rule fails loud
//!   instead of being silently skipped.
//! - A same-tile pair that is an edge in both directions is one of the
//!   4 bidirectional PIPs (no directional PIP pair exists in both
//!   directions in the INT tile: verified over all 3,774 PIPs). The
//!   caller supplies the traversal direction through `bidir`: the
//!   concrete [`crate::wire_db::IntTileWireDb`] resolves it from its
//!   PIP table, and [`translate_route`] without a resolver fails loud
//!   with [`RouteToPipError::AmbiguousBidirectionalPip`] rather than
//!   guessing FWD or REV.

use crate::fabric::{WireDb, WireId, WireInfo};
use crate::int_pip::{BidirDir, IntPip};
use crate::router::UltraRouted;
use std::fmt;

/// A translated route: the INT PIPs it programs, in route order, plus
/// the number of span-wire continuations crossed (which program no PIP).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranslatedRoute {
    pub pips: Vec<IntPip>,
    pub continuations: usize,
}

/// Translation failure. Carries the exact wire or PIP that stopped the
/// translation, and the design that needed it. Never a bare message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteToPipError {
    /// A wire in the route has no name in this WireDb (placeholder,
    /// mock, or an out-of-range ID). Translation refused.
    UnnamedWire { wire: WireId, design: String },
    /// Two consecutive wires in the route are not connected by any
    /// graph edge. The route is not an ordered path, or it jumps
    /// between wires that do not touch.
    NotConnected {
        src: String,
        dst: String,
        design: String,
    },
    /// A cross-tile pair does not fit the span-wire continuation rule
    /// (BEG in one tile to its END partner LEN steps away). It is
    /// neither a PIP nor a continuation.
    NotAContinuation {
        src: String,
        dst: String,
        design: String,
    },
    /// A same-tile pair is an edge in both directions (a bidirectional
    /// PIP) and no direction resolver was supplied, so FWD vs REV
    /// cannot be decided. Use [`crate::wire_db::IntTileWireDb::translate_route`]
    /// or pass a resolver to [`translate_route_with`].
    AmbiguousBidirectionalPip {
        tile: String,
        src: String,
        dst: String,
        design: String,
    },
}

impl fmt::Display for RouteToPipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnnamedWire { wire, design } => write!(
                f,
                "wire {} in design '{}' has no name in this WireDb; \
                 mock-routed designs cannot feed int-pip emission",
                wire, design
            ),
            Self::NotConnected { src, dst, design } => write!(
                f,
                "route in design '{}' jumps from wire '{}' to '{}' with no edge between them",
                design, src, dst
            ),
            Self::NotAContinuation { src, dst, design } => write!(
                f,
                "route in design '{}' crosses tiles from '{}' to '{}', \
                 which is neither a PIP nor a span-wire continuation",
                design, src, dst
            ),
            Self::AmbiguousBidirectionalPip {
                tile,
                src,
                dst,
                design,
            } => write!(
                f,
                "bidirectional PIP {}.PIP.{}.{} in design '{}' needs a \
                 direction resolver (FWD vs REV); none was supplied",
                tile, dst, src, design
            ),
        }
    }
}

impl std::error::Error for RouteToPipError {}

/// Span-wire naming: `<DIR><LEN>[_E|_W]_(BEG|END)<n>`. Returns the
/// partner wire name and the tile delta from this wire's tile to the
/// partner's tile, or `None` when the name is not a span wire. This is
/// the translator-side check that a cross-tile pair is a genuine
/// continuation; the graph-side builder lives in `wire_db`.
fn continuation_partner(name: &str) -> Option<(String, (i32, i32))> {
    let (head, kind) = match name.find("_BEG") {
        Some(i) => (&name[..i], Some((i, true))),
        None => match name.find("_END") {
            Some(i) => (&name[..i], Some((i, false))),
            None => (name, None),
        },
    };
    let (i, is_beg) = kind?;
    let idx = &name[i + 4..];
    if idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (dir_len, side) = match head.rsplit_once('_') {
        Some((d, s)) if s == "E" || s == "W" => (d, s),
        _ => (head, ""),
    };
    let ((dx, dy), len) = match dir_len {
        "NN1" => ((0, 1), 1),
        "NN2" => ((0, 1), 2),
        "NN4" => ((0, 1), 4),
        "NN12" => ((0, 1), 12),
        "SS1" => ((0, -1), 1),
        "SS2" => ((0, -1), 2),
        "SS4" => ((0, -1), 4),
        "SS12" => ((0, -1), 12),
        "EE1" => ((1, 0), 1),
        "EE2" => ((1, 0), 2),
        "EE4" => ((1, 0), 4),
        "EE12" => ((1, 0), 12),
        "WW1" => ((-1, 0), 1),
        "WW2" => ((-1, 0), 2),
        "WW4" => ((-1, 0), 4),
        "WW12" => ((-1, 0), 12),
        _ => return None,
    };
    let base = format!(
        "{}{}{}",
        &name[..2],
        len,
        if side.is_empty() {
            String::new()
        } else {
            format!("_{}", side)
        }
    );
    let partner = if is_beg {
        format!("{}_END{}", base, idx)
    } else {
        format!("{}_BEG{}", base, idx)
    };
    let delta = if is_beg {
        (dx * len, dy * len)
    } else {
        (-dx * len, -dy * len)
    };
    Some((partner, delta))
}

fn named<'a>(db: &'a dyn WireDb, wire: WireId, design: &str) -> Result<WireInfo<'a>, RouteToPipError> {
    db.wire_info(wire)
        .ok_or_else(|| RouteToPipError::UnnamedWire {
            wire,
            design: design.to_string(),
        })
}

/// Translate one ordered route (driver to sink, physical direction) to
/// INT PIPs, with an optional bidirectional-PIP direction resolver.
/// The resolver maps a route pair (a, b) to the traversal direction
/// relative to the tile JSON's src to dst order (`Fwd` when (a, b) is
/// in that order, `Rev` when it is the reverse), or `None` when the
/// pair is not a bidirectional PIP. Pass `None` for the resolver to
/// fail loud on any bidirectional PIP instead of guessing.
pub fn translate_route_with(
    db: &dyn WireDb,
    route: &[WireId],
    design: &str,
    bidir: Option<&dyn Fn(WireId, WireId) -> Option<BidirDir>>,
) -> Result<TranslatedRoute, RouteToPipError> {
    let mut out = TranslatedRoute::default();
    if route.len() < 2 {
        return Ok(out);
    }
    for pair in route.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let info_a = named(db, a, design)?;
        let info_b = named(db, b, design)?;
        if !db.fanout(a).contains(&b) {
            return Err(RouteToPipError::NotConnected {
                src: info_a.name.to_string(),
                dst: info_b.name.to_string(),
                design: design.to_string(),
            });
        }
        if info_a.tile == info_b.tile {
            // Same tile: a PIP. Bidirectional PIPs are edges in both
            // directions; directional PIPs are one-way.
            if db.fanout(b).contains(&a) {
                match bidir.and_then(|resolve| resolve(a, b)) {
                    Some(BidirDir::Fwd) => out.pips.push(IntPip::with_bidir(
                        info_a.tile,
                        info_a.name,
                        info_b.name,
                        BidirDir::Fwd,
                    )),
                    Some(BidirDir::Rev) => out.pips.push(IntPip::with_bidir(
                        info_a.tile,
                        info_b.name,
                        info_a.name,
                        BidirDir::Rev,
                    )),
                    None => {
                        return Err(RouteToPipError::AmbiguousBidirectionalPip {
                            tile: info_a.tile.to_string(),
                            src: info_a.name.to_string(),
                            dst: info_b.name.to_string(),
                            design: design.to_string(),
                        });
                    }
                }
            } else {
                out.pips
                    .push(IntPip::new(info_a.tile, info_a.name, info_b.name));
            }
        } else {
            // Cross-tile: must be a span-wire continuation.
            let (xa, ya) = db.wire_xy(a);
            let (xb, yb) = db.wire_xy(b);
            let is_continuation = match continuation_partner(info_a.name) {
                Some((partner, (dx, dy))) => {
                    partner == info_b.name && xb - xa == dx && yb - ya == dy
                }
                None => false,
            };
            if is_continuation {
                out.continuations += 1;
            } else {
                return Err(RouteToPipError::NotAContinuation {
                    src: format!("{}.{}", info_a.tile, info_a.name),
                    dst: format!("{}.{}", info_b.tile, info_b.name),
                    design: design.to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// Translate one ordered route (driver to sink, physical direction) to
/// INT PIPs. Bidirectional PIPs fail loud: without a direction
/// resolver FWD vs REV cannot be decided. Use
/// [`translate_route_with`] or
/// [`crate::wire_db::IntTileWireDb::translate_route`] for those.
pub fn translate_route(
    db: &dyn WireDb,
    route: &[WireId],
    design: &str,
) -> Result<TranslatedRoute, RouteToPipError> {
    translate_route_with(db, route, design, None)
}

/// Translate every net of a routed design, in net order. One entry per
/// net in `routed.routes`, in the same order.
pub fn translate_routes(
    db: &dyn WireDb,
    routed: &UltraRouted,
    design: &str,
) -> Result<Vec<TranslatedRoute>, RouteToPipError> {
    translate_routes_with(db, routed, design, None)
}

/// Like [`translate_routes`], with a bidirectional-PIP resolver.
pub fn translate_routes_with(
    db: &dyn WireDb,
    routed: &UltraRouted,
    design: &str,
    bidir: Option<&dyn Fn(WireId, WireId) -> Option<BidirDir>>,
) -> Result<Vec<TranslatedRoute>, RouteToPipError> {
    let mut out = Vec::with_capacity(routed.routes.len());
    for route in &routed.routes {
        out.push(translate_route_with(db, route, design, bidir)?);
    }
    Ok(out)
}

/// FASM text for translated PIPs: one line per PIP, in route order,
/// across all nets.
pub fn translated_to_fasm(translated: &[TranslatedRoute]) -> String {
    let mut out = String::new();
    for t in translated {
        for pip in &t.pips {
            out.push_str(&pip.fasm_line());
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::EmptyWireDb;
    use crate::router::mock::MockWireDb;
    use crate::wire_db::{IntTile, IntTileWireDb};

    const TILE_JSON: &str = include_str!("../data/tile_type_INT.json");

    fn one_tile() -> IntTileWireDb {
        IntTileWireDb::single_tile("INT_X12Y45", 12, 45, TILE_JSON).unwrap()
    }

    #[test]
    fn blinky_path_translates_to_golden_pips() {
        let db = one_tile();
        // Physical order: LUT output wire to the BOUNCE wire that
        // feeds the FF input side of the tile.
        let route = [
            db.wire_id("INT_X12Y45", "LOGIC_OUTS_E0").unwrap(),
            db.wire_id("INT_X12Y45", "INODE_E_1_FT1").unwrap(),
            db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").unwrap(),
        ];
        let t = translate_route(&db, &route, "blinky").unwrap();
        assert_eq!(t.continuations, 0);
        assert_eq!(
            t.pips.iter().map(|p| p.fasm_line()).collect::<Vec<_>>(),
            vec![
                "INT_X12Y45.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0",
                "INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1",
            ]
        );
        // The second PIP is the golden blinky PIP (modulo the tile
        // name: the golden uses INT_X0Y0, this test uses INT_X12Y45).
        assert_eq!(t.pips[1].feature(), "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1");
    }

    #[test]
    fn continuation_crosses_tiles_without_a_pip() {
        let db = IntTileWireDb::from_json(
            TILE_JSON,
            &[
                IntTile::new("INT_X12Y45", 12, 45),
                IntTile::new("INT_X12Y57", 12, 57),
            ],
        )
        .unwrap();
        // NN12_BEG7 in (12,45) continues as NN12_END7 in (12,57),
        // which drives NN12_BEG7 there through one PIP.
        let route = [
            db.wire_id("INT_X12Y45", "NN12_BEG7").unwrap(),
            db.wire_id("INT_X12Y57", "NN12_END7").unwrap(),
            db.wire_id("INT_X12Y57", "NN12_BEG7").unwrap(),
        ];
        let t = translate_route(&db, &route, "blinky").unwrap();
        assert_eq!(t.continuations, 1);
        assert_eq!(t.pips.len(), 1);
        assert_eq!(t.pips[0].fasm_line(), "INT_X12Y57.PIP.NN12_BEG7.NN12_END7");
    }

    #[test]
    fn bidir_pair_fails_loud_without_resolver_and_resolves_with_one() {
        let db = one_tile();
        let imux = db
            .wire_id("INT_X12Y45", "INT_NODE_IMUX_5_INT_OUT0")
            .unwrap();
        let bypass = db.wire_id("INT_X12Y45", "BYPASS_E8").unwrap();
        // No resolver: fail loud, do not guess FWD or REV.
        let err = translate_route(&db, &[imux, bypass], "blinky").unwrap_err();
        assert_eq!(
            err,
            RouteToPipError::AmbiguousBidirectionalPip {
                tile: "INT_X12Y45".to_string(),
                src: "INT_NODE_IMUX_5_INT_OUT0".to_string(),
                dst: "BYPASS_E8".to_string(),
                design: "blinky".to_string(),
            }
        );
        // With the concrete DB as resolver: FWD one way, REV the other,
        // both spelled with the tile-canonical src/dst order.
        let fwd = db.translate_route(&[imux, bypass], "blinky").unwrap();
        assert_eq!(fwd.pips.len(), 1);
        assert_eq!(
            fwd.pips[0].feature(),
            "PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.FWD"
        );
        let rev = db.translate_route(&[bypass, imux], "blinky").unwrap();
        assert_eq!(rev.pips.len(), 1);
        assert_eq!(
            rev.pips[0].feature(),
            "PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.REV"
        );
        // The REV PIP still carries canonical src/dst.
        assert_eq!(rev.pips[0].src, "INT_NODE_IMUX_5_INT_OUT0");
        assert_eq!(rev.pips[0].dst, "BYPASS_E8");
    }

    #[test]
    fn mock_routed_design_fails_loud_on_unnamed_wire() {
        let db = MockWireDb::new(4, 4);
        let err = translate_route(&db, &[0, 1, 2], "blinky").unwrap_err();
        assert_eq!(
            err,
            RouteToPipError::UnnamedWire {
                wire: 0,
                design: "blinky".to_string(),
            }
        );
        let empty = EmptyWireDb;
        let err = translate_route(&empty, &[0], "blinky").unwrap();
        assert!(err.pips.is_empty());
        let err = translate_route(&empty, &[0, 1], "blinky").unwrap_err();
        assert_eq!(
            err,
            RouteToPipError::UnnamedWire {
                wire: 0,
                design: "blinky".to_string(),
            }
        );
    }

    #[test]
    fn non_adjacent_pair_fails_loud_naming_both_wires() {
        let db = one_tile();
        // LOGIC_OUTS_E0 does not drive BOUNCE_E_0_FT1 directly.
        let route = [
            db.wire_id("INT_X12Y45", "LOGIC_OUTS_E0").unwrap(),
            db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").unwrap(),
        ];
        let err = translate_route(&db, &route, "blinky").unwrap_err();
        assert_eq!(
            err,
            RouteToPipError::NotConnected {
                src: "LOGIC_OUTS_E0".to_string(),
                dst: "BOUNCE_E_0_FT1".to_string(),
                design: "blinky".to_string(),
            }
        );
    }

    #[test]
    fn short_routes_translate_to_nothing() {
        let db = one_tile();
        let w = db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").unwrap();
        assert_eq!(
            translate_route(&db, &[], "blinky").unwrap(),
            TranslatedRoute::default()
        );
        assert_eq!(
            translate_route(&db, &[w], "blinky").unwrap(),
            TranslatedRoute::default()
        );
    }

    #[test]
    fn translate_routes_keeps_net_order() {
        let db = one_tile();
        let routed = UltraRouted {
            routes: vec![
                vec![
                    db.wire_id("INT_X12Y45", "LOGIC_OUTS_E0").unwrap(),
                    db.wire_id("INT_X12Y45", "INODE_E_1_FT1").unwrap(),
                    db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").unwrap(),
                ],
                vec![db.wire_id("INT_X12Y45", "GCLK_B_0_0").unwrap()],
            ],
            iters_used: 1,
            overused_nodes: 0,
        };
        let ts = translate_routes(&db, &routed, "blinky").unwrap();
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].pips.len(), 2);
        assert!(ts[1].pips.is_empty());
        assert_eq!(
            translated_to_fasm(&ts),
            "INT_X12Y45.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0\n\
             INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n"
        );
    }
}
