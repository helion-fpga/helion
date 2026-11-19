//! UltraGrid adapter: [`DeviceFile`] (vendored BitFiltrator `xcku3p.json`)
//! as a placer grid.
//!
//! Builds the slice-site list from the device file's own column tables,
//! not from the logical `UltraScaleDevice` model (known G2: invented
//! grid). For every SLR and FAR row, every logical CLB column `x`
//! contributes 60 slices (`NUM_CLB_PER_COLUMN`, the same 60 the
//! `BitLocator` uses), at Y = row * 60 + y_ofst. Slice kind comes from
//! the file's `clb_tileTypes`: CLEM / CLEM_R are Memory, CLEL_L /
//! CLEL_R are Logic. Any other tile type fails loud at construction.
//!
//! Provisional, stated plainly: the site X coordinate is the logical
//! CLB column index from the file, and the tile label is the file's tile
//! type plus coordinates (`CLEL_R_X0Y0`). That matches the golden blinky
//! FASM (`SLICE_X0Y0` = row major 0, CLB column 0, tile type CLEL_R),
//! but the logical-column-to-Vivado-tile-X mapping is not verified by
//! any public source. Ground truth waits on the Q6 Vivado PinPlanning
//! probe (GAP-B2). The physical FAR column major for each CLB column is
//! kept in `clb_columns` so the bitgen step can recover it.
//!
//! File truth for the vendored `xcku3p.json`: 4 FAR rows, 113 CLB
//! columns per row, 452 CLB columns total (244 Logic, 208 Memory),
//! 27,120 slice sites, exactly the file's `num_slices`. The file's
//! `num_slices` contradicts AMD's 20,340 by one clock row; this grid
//! uses file truth and says so (see PACKPLACE_INVENTORY.md, G-F2a).
//!
//! Hard-block columns are exposed for the future pack step (G-F2b):
//! 40 BRAM content columns and 76 DSP columns in the file. URAM column
//! majors are NOT in the device file schema, so no URAM column listing
//! exists here.

use crate::bitloc::NUM_CLB_PER_COLUMN;
use crate::device_file::{DeviceFile, DeviceFileError};
use crate::pack::SliceKind;
use crate::place::UltraGrid;

/// One hard-block or CLB column from the device file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceColumn {
    /// SLR name, e.g. `SLR0`.
    pub slr: String,
    /// FAR row index inside the SLR.
    pub row: u32,
    /// Logical column index within the row (0-based, file order).
    pub logical_x: u32,
    /// Physical column major (FAR column) from the file.
    pub col_major: u32,
}

/// Placer grid built from a BitFiltrator device file.
#[derive(Clone, Debug)]
pub struct DeviceFileGrid {
    sites: Vec<(String, String, i32, i32, SliceKind)>,
    clb_columns: Vec<DeviceColumn>,
    bram_columns: Vec<DeviceColumn>,
    dsp_columns: Vec<DeviceColumn>,
    grid_w: i32,
    grid_h: i32,
}

impl DeviceFileGrid {
    /// Build the grid from a loaded device file. Fails loud on any
    /// tile type the grid does not know, or on any missing table.
    pub fn from_device_file(file: &DeviceFile) -> Result<Self, DeviceFileError> {
        let mut sites: Vec<(String, String, i32, i32, SliceKind)> = Vec::new();
        let mut clb_columns: Vec<DeviceColumn> = Vec::new();
        let mut bram_columns: Vec<DeviceColumn> = Vec::new();
        let mut dsp_columns: Vec<DeviceColumn> = Vec::new();
        let mut max_logical_x: u32 = 0;
        let mut max_row: u32 = 0;

        for slr in file.slr_names() {
            for row in file.row_majors(slr)? {
                let n_clb = file.num_clb_columns(slr, row)?;
                for x in 0..n_clb as u32 {
                    let tile_type = file.clb_tile_type(slr, row, x)?;
                    let kind = match tile_type {
                        "CLEL_L" | "CLEL_R" => SliceKind::Logic,
                        "CLEM" | "CLEM_R" => SliceKind::Memory,
                        other => {
                            return Err(DeviceFileError::UnknownTileType(other.to_string()));
                        }
                    };
                    let col_major = file.clb_col_major(slr, row, x)?;
                    clb_columns.push(DeviceColumn {
                        slr: slr.to_string(),
                        row,
                        logical_x: x,
                        col_major,
                    });
                    for y_ofst in 0..NUM_CLB_PER_COLUMN {
                        let y = row * NUM_CLB_PER_COLUMN + y_ofst;
                        sites.push((
                            format!("SLICE_X{x}Y{y}"),
                            format!("{tile_type}_X{x}Y{y}"),
                            x as i32,
                            y as i32,
                            kind,
                        ));
                    }
                    if x > max_logical_x {
                        max_logical_x = x;
                    }
                }
                for x in 0..file.num_bram_columns(slr, row)? as u32 {
                    bram_columns.push(DeviceColumn {
                        slr: slr.to_string(),
                        row,
                        logical_x: x,
                        col_major: file.bram_content_col_major(slr, row, x)?,
                    });
                }
                for x in 0..file.num_dsp_columns(slr, row)? as u32 {
                    dsp_columns.push(DeviceColumn {
                        slr: slr.to_string(),
                        row,
                        logical_x: x,
                        col_major: file.dsp_col_major(slr, row, x)?,
                    });
                }
                if row > max_row {
                    max_row = row;
                }
            }
        }

        // Sort for a stable deterministic order: Y first, then X.
        sites.sort_by_key(|(_, _, x, y, _)| (*y, *x));

        Ok(Self {
            sites,
            clb_columns,
            bram_columns,
            dsp_columns,
            grid_w: max_logical_x as i32 + 1,
            grid_h: (max_row + 1) as i32 * NUM_CLB_PER_COLUMN as i32,
        })
    }

    /// CLB columns (one per logical column per row), in file order.
    pub fn clb_columns(&self) -> &[DeviceColumn] {
        &self.clb_columns
    }

    /// BRAM content columns, in file order.
    pub fn bram_columns(&self) -> &[DeviceColumn] {
        &self.bram_columns
    }

    /// DSP columns, in file order.
    pub fn dsp_columns(&self) -> &[DeviceColumn] {
        &self.dsp_columns
    }
}

impl UltraGrid for DeviceFileGrid {
    fn slice_sites(&self) -> Vec<(String, String, i32, i32, SliceKind)> {
        self.sites.clone()
    }

    fn grid_size(&self) -> (i32, i32) {
        (self.grid_w, self.grid_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
    }

    fn load_xcku3p() -> DeviceFile {
        DeviceFile::from_path(&data_dir().join("xcku3p.json")).expect("xcku3p.json must load")
    }

    #[test]
    fn xcku3p_grid_counts_match_device_file() {
        let file = load_xcku3p();
        let grid = DeviceFileGrid::from_device_file(&file).unwrap();
        let sites = grid.slice_sites();
        // File truth: 452 CLB columns x 60 slices = 27,120 sites,
        // exactly the file's num_slices.
        assert_eq!(sites.len(), 27_120);
        assert_eq!(sites.len(), file.num_slices() as usize);
        let memory = sites
            .iter()
            .filter(|(_, _, _, _, k)| *k == SliceKind::Memory)
            .count();
        let logic = sites
            .iter()
            .filter(|(_, _, _, _, k)| *k == SliceKind::Logic)
            .count();
        // 208 CLEM/CLEM_R columns x 60, 244 CLEL columns x 60.
        assert_eq!(memory, 208 * 60);
        assert_eq!(logic, 244 * 60);
        assert_eq!(grid.clb_columns().len(), 452);
        assert_eq!(grid.bram_columns().len(), 40);
        assert_eq!(grid.dsp_columns().len(), 76);
        assert_eq!(grid.grid_size(), (113, 240));
    }

    #[test]
    fn xcku3p_grid_first_site_matches_golden_blinky() {
        // The golden blinky FASM places logic in SLICE_X0Y0: row major 0,
        // CLB column 0, tile type CLEL_R per the device file.
        let file = load_xcku3p();
        let grid = DeviceFileGrid::from_device_file(&file).unwrap();
        let sites = grid.slice_sites();
        let first = sites
            .iter()
            .find(|(site, _, _, _, _)| site == "SLICE_X0Y0")
            .expect("SLICE_X0Y0 must exist");
        assert_eq!(first.1, "CLEL_R_X0Y0");
        assert_eq!(first.4, SliceKind::Logic);
    }

    #[test]
    fn grid_site_order_is_deterministic() {
        let file = load_xcku3p();
        let a = DeviceFileGrid::from_device_file(&file).unwrap();
        let b = DeviceFileGrid::from_device_file(&file).unwrap();
        assert_eq!(a.slice_sites(), b.slice_sites());
        // Sites are sorted Y first, then X.
        let sites = a.slice_sites();
        let mut sorted = sites.clone();
        sorted.sort_by_key(|(_, _, x, y, _)| (*y, *x));
        assert_eq!(sites, sorted);
    }
}
