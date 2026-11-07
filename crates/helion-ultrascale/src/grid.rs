//! UltraGrid adapter: [`UltraScaleDevice`] as a placer grid.
//!
//! Translates device-model tiles into the placer's slice-site list.
//! Site names follow Vivado site naming (`SLICE_X{x}Y{y}`). Tile names are
//! PROVISIONAL (`CLBLL_L_X{x}Y{y}` for SLICEL, `CLBLM_L_X{x}Y{y}` for
//! SLICEM): the logical grid has no left/right half distinction, so `_L`
//! is used uniformly until the device-file worker lands real tile names.
//! The device-file worker owns tile naming; this adapter only maps what
//! the device model already knows.

use crate::device::UltraScaleDevice;
use crate::pack::SliceKind;
use crate::place::UltraGrid;
use crate::TileKind;

impl UltraGrid for UltraScaleDevice {
    fn slice_sites(&self) -> Vec<(String, String, i32, i32, SliceKind)> {
        let mut tiles: Vec<_> = self
            .tiles()
            .filter(|(_, k)| matches!(k, TileKind::Clel | TileKind::Clem))
            .collect();
        // HashMap iteration is not deterministic; sort for a stable order.
        tiles.sort_by_key(|(id, _)| (id.y, id.x));
        tiles
            .into_iter()
            .map(|(id, kind)| {
                let (site, tile, slice_kind) = match kind {
                    TileKind::Clel => (
                        format!("SLICE_X{}Y{}", id.x, id.y),
                        format!("CLBLL_L_X{}Y{}", id.x, id.y),
                        SliceKind::Logic,
                    ),
                    TileKind::Clem => (
                        format!("SLICE_X{}Y{}", id.x, id.y),
                        format!("CLBLM_L_X{}Y{}", id.x, id.y),
                        SliceKind::Memory,
                    ),
                    _ => unreachable!("filtered to CLB tiles above"),
                };
                (site, tile, id.x as i32, id.y as i32, slice_kind)
            })
            .collect()
    }

    fn grid_size(&self) -> (i32, i32) {
        (self.grid_width() as i32, self.grid_height() as i32)
    }
}
