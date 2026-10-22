//! Mock WireDb: a grid graph for algorithm development.
//!
//! Each tile has one wire; each wire fans out to its 4 Manhattan neighbors.
//! BEL pins map to the wire at their site. This is NOT a realistic UltraScale
//! model; it exists so the PathFinder core can be developed and tested before
//! the real WireDb lands.

use super::{UltraSite, WireDb, WireId};
use std::collections::HashMap;

pub struct MockWireDb {
    cols: i32,
    rows: i32,
    tracks: i32,
    fanout: Vec<Vec<WireId>>,
    pin_wires: HashMap<(i32, i32, String, String), WireId>,
}

impl MockWireDb {
    pub fn new(cols: i32, rows: i32) -> Self {
        Self::with_tracks(cols, rows, 1)
    }

    /// Multiple parallel wires per tile, like real FPGA routing tracks.
    /// Track 0 connects to track 0 of neighbors, etc., plus inter-track
    /// vias at each tile (track i -> track (i+1)%tracks).
    pub fn with_tracks(cols: i32, rows: i32, tracks: i32) -> Self {
        let n = (cols * rows * tracks) as usize;
        let mut fanout = vec![Vec::new(); n];
        let wire_id = |x: i32, y: i32, t: i32| (y * cols * tracks + x * tracks + t) as WireId;
        for y in 0..rows {
            for x in 0..cols {
                for t in 0..tracks {
                    let w = wire_id(x, y, t);
                    // Manhattan neighbors on same track.
                    for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let nx = x + dx;
                        let ny = y + dy;
                        if nx >= 0 && nx < cols && ny >= 0 && ny < rows {
                            fanout[w as usize].push(wire_id(nx, ny, t));
                        }
                    }
                    // Inter-track via.
                    fanout[w as usize].push(wire_id(x, y, (t + 1) % tracks));
                }
            }
        }
        Self {
            cols,
            rows,
            tracks,
            fanout,
            pin_wires: HashMap::new(),
        }
    }

    fn wire_at(&self, x: i32, y: i32) -> Option<WireId> {
        if x >= 0 && x < self.cols && y >= 0 && y < self.rows {
            Some((y * self.cols * self.tracks + x * self.tracks) as WireId)
        } else {
            None
        }
    }

    /// Register a BEL pin at a site. Maps to the site's wire.
    pub fn add_pin(&mut self, site: UltraSite, bel: &str, pin: &str) {
        if let Some(w) = self.wire_at(site.x, site.y) {
            self.pin_wires
                .insert((site.x, site.y, bel.to_string(), pin.to_string()), w);
        }
    }
}

impl WireDb for MockWireDb {
    fn wire_count(&self) -> usize {
        (self.cols * self.rows * self.tracks) as usize
    }

    fn fanout(&self, wire: WireId) -> &[WireId] {
        &self.fanout[wire as usize]
    }

    fn wire_xy(&self, wire: WireId) -> (i32, i32) {
        let w = wire as i32;
        let t = self.tracks;
        ((w / t) % self.cols, (w / t) / self.cols)
    }

    fn wire_for_bel_pin(&self, site: UltraSite, bel: &str, pin: &str) -> Option<WireId> {
        self.pin_wires
            .get(&(site.x, site.y, bel.to_string(), pin.to_string()))
            .copied()
    }
}
