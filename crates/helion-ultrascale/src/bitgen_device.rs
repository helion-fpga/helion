//! `BitgenDevice` implementation for the real device file.
//!
//! This closes the interfaces session 2 item 4 FLAG: the emitter
//! (`bitgen.rs`) can now run on a real `DeviceFile` instead of only
//! the provisional `StubDevice`. FRAMING green on a real device
//! depends on this impl.
//!
//! Ground rules, tile by tile:
//!
//! - SLICE tiles (`SLICE_X<x>Y<y>`, y absolute slice Y): ground
//!   truth from the device file. SLR from the Y range, row major
//!   from Y and 60 CLBs per column, column major and tile type
//!   (`CLEL_L`, `CLEM`, `CLEL_R`, `CLEM_R`) from the file, exactly
//!   as `BitLocator` resolves BELs. Base FAR minor is 0: the
//!   segbits DB frame equals the `BitLocator` minor for every CLB
//!   bit checked (FF INIT: DB frame 12, locator minor 12 for AFF
//!   and BFF; LUT INIT: DB frames 11..8, locator minors 11..8 for
//!   A6LUT; CLEL DB frames span 0..15, the 16 minors of a CLB
//!   column). `tile_base_far` is therefore
//!   `Far(ClbIoClk, row_major, col_major, 0)`.
//! - INT tiles (`INT_X<x>Y<r>`, r the FAR row): PROVISIONAL rule,
//!   the same rule the diff harness reference uses
//!   (`harness/reference.py`, `INT_COL_OFFSET_FROM_CLB = -1`,
//!   xczu3eg-derived): INT column major = CLB column major - 1.
//!   The device file confirms the rule where the golden exercises
//!   it: INT_X0Y0 resolves to major 4, which carries exactly 76
//!   minors, the INT segbits frame span (vendored
//!   `segbits_int.db`, frames 0..75). The rule is validated, not
//!   assumed: an INT tile resolves only when its rule-derived
//!   column carries at least 76 minors; otherwise `tile_base_far`
//!   and `tile_type` return `None` (fail loud). On xcku3p the
//!   rule is invalid for 61 of 113 CLB columns per row (the second
//!   CLB of each pair, whose left neighbour is another CLB
//!   column, not an INT column); those INT tiles do not resolve.
//!   The xcku3p tilegrid is the arbiter: when it lands, this rule
//!   is replaced by ground truth. Until then, INT placement is
//!   absolute-provisional, exactly as the harness flags it.
//! - RAMB18 tiles (`RAMB18_X<x>Y<y>`, y absolute 18K Y): ground
//!   truth from the device file (BRAM content column major, 24
//!   18K blocks per column), tile type `BRAM` (the device file's
//!   own tile-type string), base FAR
//!   `Far(BramContent, row_major, col_major, 0)`. BRAM parity
//!   shares the content column majors in the file (the parity
//!   table equals the content table), so no separate parity
//!   enumeration is needed. RAMB36 tiles have no resolver
//!   (`BitLocator` rejects them too) and do not resolve here.
//! - HPIO, HDIO, CMT, DSP, and every other tile kind: NOT
//!   resolved (`None`). Their column majors are not in the device
//!   file and no public tilegrid exists for xcku3p/xcvu3p, so any
//!   base FAR would be invented. This is a named blocker, not a
//!   silent skip: the golden's HPIO lines cannot emit until the
//!   tilegrid lands (the harness resolves IO bits tile-relatively,
//!   so the IO_CONFIG block itself is unaffected).
//!
//! `tile_names` enumerates exactly the tiles that resolve (all
//! SLICE tiles, the resolving INT tiles, all RAMB18 tiles),
//! sorted, deduplicated. `config_order_fars` enumerates every
//! real frame in the file (standard columns plus BRAM content
//! columns), ascending FAR integer, which is the config order
//! the demo reconstruction uses (`sorted(frames)` in
//! `make_synth_xcku3p_bit.py`); its length equals
//! `DeviceFile::total_frames` (41,460 for xcku3p), asserted in
//! the integration tests. Both lists are computed lazily on
//! first use (`OnceLock` caches on `DeviceFile`) so loading a
//! device file for the locator or the demo stays cheap.
//!
//! Carried, not fixed here (recorded, not hidden):
//!
//! - Critic session 7 FLAG F1: the last-of-row FAR 2-zero-padding
//!   path is untested (0 of 261 FARs in the demo validation
//!   exercised it). This impl does not change frame emission, so
//!   it does not fix F1; the generator must not be trusted for
//!   full-device bitstreams until F1 is exercised.
//! - The xcvu3p frame-total gap: the file's tables sum to 73,035
//!   frames against UG570 Table 1's 71,820 (overshoot 1,215).
//!   Flagged provisional in the file, never tuned away; this impl
//!   enumerates the file's own frames.
//! - The device file's `num_slices` (27,120) contradicts its own
//!   `num_luts` (162,720, 6 per slice, not 8) and the BRAM/DSP
//!   tables imply more sites than the headers claim. This impl
//!   enumerates tiles from the column tables, never from the
//!   header counts, so those anomalies do not affect it.
//!
//! No em dashes anywhere.

use crate::bitgen::BitgenDevice;
use crate::device_file::{
    DeviceFile, Far, FarBlockType, NUM_18K_BRAM_PER_COLUMN, NUM_CLB_PER_COLUMN,
};

/// INT segbits frame span: the vendored `segbits_int.db` carries
/// frames 0..75 (3778 lines). An INT tile's column must carry at
/// least this many minors for the tile to resolve; see the module
/// docs for why the provisional rule is validated against the
/// device file's own minors table.
pub const INT_FRAME_SPAN: u32 = 76;

/// Parse `SLICE_X<x>Y<y>` into (x, y). Returns `None` on any
/// malformed name instead of panicking.
fn parse_slice_tile(tile: &str) -> Option<(u32, u32)> {
    let rest = tile.strip_prefix("SLICE_X")?;
    let (x_text, y_text) = rest.split_once('Y')?;
    let x: u32 = x_text.parse().ok()?;
    let y: u32 = y_text.parse().ok()?;
    Some((x, y))
}

/// Parse `INT_X<x>Y<r>` into (x, r). The Y is the FAR row (one INT
/// tile per clock-region row), not an absolute slice Y.
fn parse_int_tile(tile: &str) -> Option<(u32, u32)> {
    let rest = tile.strip_prefix("INT_X")?;
    let (x_text, r_text) = rest.split_once('Y')?;
    let x: u32 = x_text.parse().ok()?;
    let r: u32 = r_text.parse().ok()?;
    Some((x, r))
}

/// Parse `RAMB18_X<x>Y<y>` into (x, y). Only 18K BRAM tiles are
/// supported, as in `BitLocator`.
fn parse_bram_tile(tile: &str) -> Option<(u32, u32)> {
    let rest = tile.strip_prefix("RAMB18_X")?;
    let (x_text, y_text) = rest.split_once('Y')?;
    let x: u32 = x_text.parse().ok()?;
    let y: u32 = y_text.parse().ok()?;
    Some((x, y))
}

/// Which SLR holds a BEL at absolute Y, given the number of that BEL
/// kind per column. Mirrors `BitLocator::slr_name`: the SLR whose
/// clock-region row range contains Y. Returns the SLR name and the
/// row major inside it.
fn slr_row_for_y(device: &DeviceFile, y: u32, per_column: u32) -> Option<(&str, u32)> {
    for slr in device.slr_names() {
        let min_row = device.min_clock_region_row_idx(slr).ok()?;
        let max_row = device.max_clock_region_row_idx(slr).ok()?;
        let min_y = min_row * per_column;
        let max_y = (max_row + 1) * per_column - 1;
        if min_y <= y && y <= max_y {
            return Some((slr, y / per_column - min_row));
        }
    }
    None
}

/// INT column major for a CLB column, by the provisional rule (CLB
/// column major - 1, harness `reference.py`), validated against the
/// device file: the rule-derived column must carry at least
/// `INT_FRAME_SPAN` minors. Returns the SLR name and the INT column
/// major, or `None` when the tile does not resolve (unknown row,
/// unknown CLB column, or the rule points at a column too short to
/// hold INT frames).
fn int_col_major(device: &DeviceFile, clb_x: u32, row_major: u32) -> Option<(&str, u32)> {
    for slr in device.slr_names() {
        let rows = device.row_majors(slr).ok()?;
        if !rows.contains(&row_major) {
            continue;
        }
        let clb_major = device.clb_col_major(slr, row_major, clb_x).ok()?;
        let int_major = clb_major.checked_sub(1)?;
        let minors = device.num_minors_per_std_col_major(slr, row_major).ok()?;
        let n = *minors.get(int_major as usize)?;
        if n >= INT_FRAME_SPAN {
            return Some((slr, int_major));
        }
        return None;
    }
    None
}

/// Resolve a Vivado tile name to its segbits tile type and base FAR
/// (the FAR of the tile's frame offset 0; `BitRef` frame offsets add
/// to it). Returns `None` for any tile the device file does not
/// cover: unknown names, HPIO/HDIO/CMT/DSP tiles (no column mapping
/// in the file), RAMB36 tiles (no resolver), and INT tiles where
/// the provisional rule fails validation.
fn resolve_tile<'a>(device: &'a DeviceFile, tile: &str) -> Option<(&'a str, u32)> {
    if let Some((x, y)) = parse_slice_tile(tile) {
        let (slr, row_major) = slr_row_for_y(device, y, NUM_CLB_PER_COLUMN)?;
        let col = device.clb_col_major(slr, row_major, x).ok()?;
        let ty = device.clb_tile_type(slr, row_major, x).ok()?;
        let base = Far::new(FarBlockType::ClbIoClk, row_major, col, 0)
            .ok()?
            .to_u32();
        return Some((ty, base));
    }
    if let Some((x, row_major)) = parse_int_tile(tile) {
        let (_slr, int_major) = int_col_major(device, x, row_major)?;
        let base = Far::new(FarBlockType::ClbIoClk, row_major, int_major, 0)
            .ok()?
            .to_u32();
        return Some(("INT", base));
    }
    if let Some((x, y)) = parse_bram_tile(tile) {
        let (slr, row_major) = slr_row_for_y(device, y, NUM_18K_BRAM_PER_COLUMN)?;
        let col = device.bram_content_col_major(slr, row_major, x).ok()?;
        let base = Far::new(FarBlockType::BramContent, row_major, col, 0)
            .ok()?
            .to_u32();
        return Some(("BRAM", base));
    }
    None
}

/// All tile names in the device file that resolve, sorted and
/// deduplicated: every SLICE tile, every resolving INT tile, every
/// RAMB18 tile. Tiles that do not resolve (HPIO/HDIO/CMT/DSP,
/// RAMB36, rule-invalid INT tiles) are absent by design.
fn compute_tile_names(device: &DeviceFile) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for slr in device.slr_names() {
        let Ok(min_clk_row) = device.min_clock_region_row_idx(slr) else {
            continue;
        };
        let Ok(rows) = device.row_majors(slr) else {
            continue;
        };
        for &row_major in &rows {
            if let Ok(n_clb) = device.num_clb_columns(slr, row_major) {
                for x in 0..n_clb as u32 {
                    for y_ofst in 0..NUM_CLB_PER_COLUMN {
                        let abs_y = (min_clk_row + row_major) * NUM_CLB_PER_COLUMN + y_ofst;
                        names.push(format!("SLICE_X{x}Y{abs_y}"));
                    }
                    if int_col_major(device, x, row_major).is_some() {
                        names.push(format!("INT_X{x}Y{row_major}"));
                    }
                }
            }
            if let Ok(n_bram) = device.num_bram_columns(slr, row_major) {
                for x in 0..n_bram as u32 {
                    for y_ofst in 0..NUM_18K_BRAM_PER_COLUMN {
                        let abs_y = (min_clk_row + row_major) * NUM_18K_BRAM_PER_COLUMN + y_ofst;
                        names.push(format!("RAMB18_X{x}Y{abs_y}"));
                    }
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Every real frame in the device file, ascending FAR integer:
/// all standard (CLB_IO_CLK) columns plus all BRAM content columns,
/// per FAR row per SLR. The count equals `DeviceFile::total_frames`
/// (asserted in the integration tests); a shortfall would mean a
/// frame the emitter could touch is missing from config order.
/// `Far::new` cannot fail for these values (row <= 6, column <=
/// 234, minor <= 255 on both files on disk), so a failed
/// construction is skipped defensively and caught by the count
/// assertion instead of panicking the library.
fn compute_config_order_fars(device: &DeviceFile) -> Vec<u32> {
    let mut fars: Vec<u32> = Vec::new();
    for slr in device.slr_names() {
        let Ok(rows) = device.row_majors(slr) else {
            continue;
        };
        for &row_major in &rows {
            if let Ok(minors) = device.num_minors_per_std_col_major(slr, row_major) {
                for (col, &n) in minors.iter().enumerate() {
                    for minor in 0..n {
                        if let Ok(far) =
                            Far::new(FarBlockType::ClbIoClk, row_major, col as u32, minor)
                        {
                            fars.push(far.to_u32());
                        }
                    }
                }
            }
            if let Ok(minors) = device.num_minors_per_bram_content_col_major(slr, row_major) {
                for (col, &n) in minors.iter().enumerate() {
                    for minor in 0..n {
                        if let Ok(far) =
                            Far::new(FarBlockType::BramContent, row_major, col as u32, minor)
                        {
                            fars.push(far.to_u32());
                        }
                    }
                }
            }
        }
    }
    fars.sort_unstable();
    fars.dedup();
    fars
}

impl BitgenDevice for DeviceFile {
    /// Device name, e.g. `xcku3p` (the trait wants the short name,
    /// not the full part string `xcku3p-ffva676-1-e`).
    fn part(&self) -> &str {
        self.device()
    }

    /// The single SLR IDCODE when every SLR in the file shares one
    /// (both files on disk have one SLR); `None` when SLRs disagree
    /// or an IDCODE fails to parse, since the emitter writes one
    /// IDCODE per bitstream.
    fn idcode(&self) -> Option<u32> {
        let mut found: Option<u32> = None;
        for slr in self.slr_names() {
            let code = self.slr_idcode(slr).ok()?;
            match found {
                None => found = Some(code),
                Some(prev) if prev == code => {}
                Some(_) => return None,
            }
        }
        found
    }

    /// Segbits tile type for a Vivado tile name, or `None` when the
    /// device file does not cover the tile.
    fn tile_type(&self, tile: &str) -> Option<&str> {
        resolve_tile(self, tile).map(|(ty, _)| ty)
    }

    /// Base FAR (frame offset 0) for a Vivado tile name, or `None`
    /// when the device file does not cover the tile.
    fn tile_base_far(&self, tile: &str) -> Option<u32> {
        resolve_tile(self, tile).map(|(_, base)| base)
    }

    /// All resolving tile names in the device file, sorted.
    fn tile_names(&self) -> &[String] {
        self.tile_names_cache
            .get_or_init(|| compute_tile_names(self))
    }

    /// All real frame FARs in config order (ascending FAR integer).
    fn config_order_fars(&self) -> &[u32] {
        self.config_order_cache
            .get_or_init(|| compute_config_order_fars(self))
    }
}
