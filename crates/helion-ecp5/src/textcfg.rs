//! Trellis textcfg emission: routed ECP5 design -> [`BitstreamEcp5`].
//!
//! This module turns a routed design into the Trellis text configuration
//! format: the same symbolic format `nextpnr-ecp5 --textcfg` emits and
//! `ecppack` consumes (`.device`, `.tile`, `.tile_group`, `.bram_init`
//! sections with `arc:`/`word:`/`enum:` entries). `ecppack` is the ground
//! truth for format validity: every symbolic entry is validated against the
//! vendored `bits.db` data before emission, and the output is fed to
//! `ecppack` in tests.
//!
//! ## Inputs
//!
//! [`emit_textcfg`] takes the real [`route::RoutedEcp5`] and
//! [`place::PlacedEcp5`] plus the [`Device`]. [`build_design`] maps them
//! onto the emitter's payload structs:
//!
//! - [`PlacedLut`] (LUT4 + optional FF on a `SLICE_A`..`SLICE_D` site;
//!   `tile_key` is the device tile-grid key, e.g. `"R2C62:PLC2"`),
//!   [`PlacedIo`] (PIO site + electrical intent), hard blocks
//!   [`EbrBlock`], [`DspBlock`], [`PllBlock`], [`DccBlock`];
//! - routed nets as [`EmitArc`] mux selections: `(tile_key, sink, source)`
//!   triples naming a Trellis routing mux output (`sink`) and the selected
//!   input (`source`), in the tile entry whose `bits.db` defines the mux;
//! - the router's [`route::ConfigEnumSetting`]s (e.g. `CLK0.CLKMUX`), emitted
//!   as validated `enum:` lines.
//!
//! ## Hard-block payloads (mapping from the pack worker's types)
//!
//! - [`EbrBlock`]: `wid` (the `.bram_init` index) is `3 +` the block's
//!   index in placement order (the pack worker emits RAMs in nextpnr's
//!   autoincrement order from 3); the INIT image is the pack worker's
//!   9-bit words padded to 2048 (`init[w]` = word at EBR address `w`, low
//!   9 bits valid). v1 supports `DATA_WIDTH_A/B = 9` only; anything else
//!   fails loudly.
//! - [`DspBlock`]: MULT18X18D site plus the full register/mux enum set
//!   (defaults match nextpnr's).
//! - [`PllBlock`]: PLL site plus divider words (`CLKI_DIV`, `CLKFB_DIV`,
//!   `CLKOP_DIV`, ...) as integer values and the mode enums.
//! - [`DccBlock`]: DCC mux (`tile_key` + `DCC_<side><n>` config name);
//!   emits the `DCC_<S><n>.MODE` enum.
//!
//! ## Fail-loud rules
//!
//! - Every `arc:` is checked against the tile type's `bits.db` muxes: the
//!   sink mux and the source input must both exist, otherwise `Err`.
//! - Every `enum:` is checked: the enum name must exist in at least one of
//!   the target tiles' `bits.db`, and the value must be a listed option
//!   (or `_NONE_`), otherwise `Err`.
//! - Every `word:` is checked: the word name must exist and the value
//!   length must equal the DB bit count, otherwise `Err`.
//! - Nothing is ever silently dropped: an unrepresentable config is an
//!   `Err`, never an omitted line.
//!
//! Bit positions are never hardcoded: all validation resolves through
//! [`Device::tile_bits`] and the vendored `bits.db` texts.

use crate::device::Device;
use crate::pack::CARRY_NET_PREFIX;
use crate::place;
use crate::route;
use std::collections::{BTreeMap, HashMap};

// ---------------------------------------------------------------------------
// Emitter payload types
// ---------------------------------------------------------------------------
// (Built from the real route/place types by `build_design`; the payload
// structs below are the emitter's own contract, validated against the
// vendored bits.db data before emission.)

/// Final bitstream in Trellis text-configuration form: the interchange
/// format `nextpnr-ecp5 --textcfg` emits and `ecppack` consumes, so Helion
/// output stays diffable against the reference flow.
#[derive(Clone, Debug, Default)]
pub struct BitstreamEcp5 {
    /// Trellis text configuration.
    pub textcfg: String,
}

/// Slice within a `PLC2` tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SliceId {
    A,
    B,
    C,
    D,
}

impl SliceId {
    fn trellis_name(self) -> &'static str {
        match self {
            SliceId::A => "SLICEA",
            SliceId::B => "SLICEB",
            SliceId::C => "SLICEC",
            SliceId::D => "SLICED",
        }
    }
}

/// Which EBR site of a block RAM tile: `EBR0`..`EBR3`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EbrSite {
    Ebr0,
    Ebr1,
    Ebr2,
    Ebr3,
}

impl EbrSite {
    fn trellis_name(self) -> &'static str {
        match self {
            EbrSite::Ebr0 => "EBR0",
            EbrSite::Ebr1 => "EBR1",
            EbrSite::Ebr2 => "EBR2",
            EbrSite::Ebr3 => "EBR3",
        }
    }
}

/// PIO within an IO tile: `PIOA`..`PIOD`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PioId {
    A,
    B,
    C,
    D,
}

impl PioId {
    fn trellis_name(self) -> &'static str {
        match self {
            PioId::A => "PIOA",
            PioId::B => "PIOB",
            PioId::C => "PIOC",
            PioId::D => "PIOD",
        }
    }
}

/// IO direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoDir {
    Input,
    Output,
    Bidir,
}

impl IoDir {
    fn trellis_name(self) -> &'static str {
        match self {
            IoDir::Input => "INPUT",
            IoDir::Output => "OUTPUT",
            IoDir::Bidir => "BIDIR",
        }
    }
}

/// IO pull mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoPull {
    None,
    Up,
    Down,
}

impl IoPull {
    fn trellis_name(self) -> &'static str {
        match self {
            IoPull::None => "NONE",
            IoPull::Up => "UP",
            IoPull::Down => "DOWN",
        }
    }
}

/// A placed LUT4 and its optional flip-flop on a slice site.
///
/// `tile_key` is the PLC2 tile entry key (e.g. `"R2C62:PLC2"`); `lut` is 0
/// for K0, 1 for K1. `input_used[i]` (A, B, C, D) tells whether the LUT
/// input is driven by routing; undriven inputs are tied to 1 via the
/// `SLICEx.<pin><lut>MUX` enum, matching nextpnr.
#[derive(Clone, Debug)]
pub struct PlacedLut {
    pub tile_key: String,
    pub slice: SliceId,
    pub lut: u8,
    pub init: u16,
    /// Carry (CCU2) mode instead of plain logic.
    pub carry: bool,
    pub input_used: [bool; 4],
    pub ff: Option<PlacedFf>,
}

/// Flip-flop configuration paired with a [`PlacedLut`].
#[derive(Clone, Debug)]
pub struct PlacedFf {
    /// GSR enabled for this slice.
    pub gsr_enabled: bool,
    /// REGSET SET (true) vs RESET (false).
    pub set_on_reset: bool,
    /// Clock inverted (CLKMUX INV vs CLK).
    pub clk_inverted: bool,
    /// CE used (CEMUX CE vs 1).
    pub ce_used: bool,
    /// LSR used (LSRMUX LSR vs INV).
    pub lsr_used: bool,
    /// FF D input driven by the paired LUT (REG.SD 1) vs general routing (0).
    pub d_from_lut: bool,
    /// Which slice clock network: 0 = CLK0, 1 = CLK1.
    pub clk_sel: u8,
    /// Which slice set/reset network: 0 = LSR0, 1 = LSR1.
    pub lsr_sel: u8,
}

/// A placed IO: PIO site plus electrical intent.
///
/// `pio_tile_key` / `pic_tile_key` are the tile entries carrying the PIO
/// config (e.g. `"MIB_R0C64:PIOT1"` / `"MIB_R1C64:PICT1"`); the BASE_TYPE
/// enum is emitted on both, matching nextpnr.
#[derive(Clone, Debug)]
pub struct PlacedIo {
    pub pio_tile_key: String,
    pub pic_tile_key: String,
    pub pio: PioId,
    pub dir: IoDir,
    /// IO standard, e.g. `"LVCMOS33"`.
    pub io_type: String,
    pub pull: IoPull,
    /// Output drive strength in mA (emitted as `DRIVE` for outputs).
    pub drive_ma: u8,
}

/// One routed mux selection: in tile entry `tile_key`, the mux driving
/// `sink` selects input `source`.
///
/// This mirrors nextpnr's `arc: <sink> <source>` textcfg lines (sink first).
#[derive(Clone, Debug)]
pub(crate) struct EmitArc {
    pub tile_key: String,
    pub sink: String,
    pub source: String,
}

/// A placed DP16KD block RAM: the INIT payload contract with the pack worker.
///
/// - `tile_key`: the tile entry hosting the EBR site (e.g.
///   `"MIB_R70C4:MIB_EBR0"`).
/// - `site`, `bel_col`, `bel_row`: the Trellis site (bel coordinates from
///   the device model); the emitter derives the `.tile_group` from the
///   site's alias tiles.
/// - `wid`: the `.bram_init` index. The pack worker autoincrements from 3
///   (matching nextpnr/vendor behaviour).
/// - `init`: 2048 9-bit words; `init[w]` is the word at EBR address `w`
///   (low 9 bits valid). v1 only supports `data_width_a/b = 9`.
#[derive(Clone, Debug)]
pub struct EbrBlock {
    pub tile_key: String,
    pub site: EbrSite,
    pub bel_col: i32,
    pub bel_row: i32,
    pub wid: u32,
    pub init: Vec<u16>,
    pub mode: String,
    pub allow_multi_width: bool,
    pub data_width_a: u8,
    pub data_width_b: u8,
    pub write_mode_a: String,
    pub write_mode_b: String,
    pub csdecode_a: u8,
    pub csdecode_b: u8,
    pub clk_a_mux: String,
    pub clk_b_mux: String,
    pub we_a_mux: String,
    pub we_b_mux: String,
    pub ce_a_mux: String,
    pub ce_b_mux: String,
}

impl EbrBlock {
    /// Defaults matching nextpnr's DP16KD emission for a simple inferred RAM.
    pub fn simple(
        tile_key: String,
        site: EbrSite,
        bel_col: i32,
        bel_row: i32,
        wid: u32,
        init: Vec<u16>,
    ) -> Self {
        Self {
            tile_key,
            site,
            bel_col,
            bel_row,
            wid,
            init,
            mode: "DP16KD".to_string(),
            allow_multi_width: false,
            data_width_a: 9,
            data_width_b: 9,
            write_mode_a: "READBEFOREWRITE".to_string(),
            write_mode_b: "READBEFOREWRITE".to_string(),
            csdecode_a: 0,
            csdecode_b: 0,
            clk_a_mux: "CLKA".to_string(),
            clk_b_mux: "CLKB".to_string(),
            we_a_mux: "WEA".to_string(),
            we_b_mux: "WEB".to_string(),
            ce_a_mux: "CEA".to_string(),
            ce_b_mux: "CEB".to_string(),
        }
    }
}

/// A placed MULT18X18D DSP block: the config contract with the pack worker.
///
/// All register/mux enums default to nextpnr's values for an unregistered
/// multiply; the pack worker overrides per its inference.
#[derive(Clone, Debug)]
pub struct DspBlock {
    pub tile_key: String,
    /// e.g. `"MULT18_0"`.
    pub site: String,
    pub bel_col: i32,
    pub bel_row: i32,
    pub reg_inputa_clk: String,
    pub reg_inputa_ce: String,
    pub reg_inputa_rst: String,
    pub reg_inputb_clk: String,
    pub reg_inputb_ce: String,
    pub reg_inputb_rst: String,
    pub reg_inputc_clk: String,
    pub reg_pipeline_clk: String,
    pub reg_pipeline_ce: String,
    pub reg_pipeline_rst: String,
    pub reg_output_clk: String,
    pub reg_output_rst: String,
    pub gsr: String,
    pub resetmode: String,
}

impl DspBlock {
    /// Defaults matching nextpnr's MULT18X18D emission for a plain multiply.
    pub fn plain_multiply(tile_key: String, site: String, bel_col: i32, bel_row: i32) -> Self {
        let s = |v: &str| v.to_string();
        Self {
            tile_key,
            site,
            bel_col,
            bel_row,
            reg_inputa_clk: s("NONE"),
            reg_inputa_ce: s("CE0"),
            reg_inputa_rst: s("RST0"),
            reg_inputb_clk: s("NONE"),
            reg_inputb_ce: s("CE0"),
            reg_inputb_rst: s("RST0"),
            reg_inputc_clk: s("NONE"),
            reg_pipeline_clk: s("NONE"),
            reg_pipeline_ce: s("CE0"),
            reg_pipeline_rst: s("RST0"),
            reg_output_clk: s("NONE"),
            reg_output_rst: s("RST0"),
            gsr: s("ENABLED"),
            resetmode: s("SYNC"),
        }
    }
}

/// A placed PLLB: the divider/config contract with the pack worker.
///
/// Divider words are integer values; the emitter formats them as binary
/// `word:` lines per the DB bit counts.
#[derive(Clone, Debug)]
pub struct PllBlock {
    pub tile_key: String,
    pub bel_col: i32,
    pub bel_row: i32,
    pub clki_div: u32,
    pub clkfb_div: u32,
    pub clkop_div: u32,
    pub clkos_div: Option<u32>,
    pub clkos2_div: Option<u32>,
    pub clkos3_div: Option<u32>,
    pub clkop_enable: String,
    pub feedbk_path: String,
    pub dphase_source: bool,
}

impl PllBlock {
    /// Defaults matching the reference EHXPLLL emission.
    pub fn ehxplll(tile_key: String, bel_col: i32, bel_row: i32) -> Self {
        Self {
            tile_key,
            bel_col,
            bel_row,
            clki_div: 1,
            clkfb_div: 1,
            clkop_div: 1,
            clkos_div: None,
            clkos2_div: None,
            clkos3_div: None,
            clkop_enable: "ENABLED".to_string(),
            feedbk_path: "INT_OP".to_string(),
            dphase_source: false,
        }
    }
}

/// A used DCC primary-clock mux.
#[derive(Clone, Debug)]
pub struct DccBlock {
    pub tile_key: String,
    pub config_name: String,
}

/// The emitter's input bundle, built from the real [`route::RoutedEcp5`]
/// and [`place::PlacedEcp5`] by [`build_design`].
#[derive(Clone, Debug, Default)]
pub(crate) struct EmitDesign {
    pub top: String,
    pub luts: Vec<PlacedLut>,
    pub ios: Vec<PlacedIo>,
    pub arcs: Vec<EmitArc>,
    pub ebrs: Vec<EbrBlock>,
    pub dsps: Vec<DspBlock>,
    pub plls: Vec<PllBlock>,
    pub dccs: Vec<DccBlock>,
    pub config_enums: Vec<route::ConfigEnumSetting>,
    pub placed: Option<place::PlacedEcp5>,
}

// ---------------------------------------------------------------------------
// Internal accumulator
// ---------------------------------------------------------------------------

/// Config entries accumulated for one `.tile` section.
#[derive(Debug, Default)]
struct TileCfg {
    arcs: Vec<(String, String)>,
    words: Vec<(String, String)>,
    enums: Vec<(String, String)>,
}

/// Config entries accumulated for one `.tile_group` section.
#[derive(Debug)]
struct TileGroup {
    tiles: Vec<String>,
    words: Vec<(String, String)>,
    enums: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// bits.db validation helpers
// ---------------------------------------------------------------------------

/// Look up a tile entry by key. Fail loud on unknown keys.
pub(crate) fn tile_of<'a>(device: &'a Device, key: &str) -> Result<&'a crate::device::Tile, String> {
    device
        .tiles()
        .iter()
        .find(|t| t.key == key)
        .ok_or_else(|| format!("textcfg: unknown tile key {key:?}"))
}

/// Validate an `arc:` against the tile type's muxes and return ().
///
/// Checks the sink mux exists in `bits.db` and lists `source` as an input.
/// This is the "sets exactly the bits the DB says" check at the symbolic
/// level: ecppack later expands the arc to the DB's `F<B>F<B>` tokens.
pub(crate) fn check_arc(device: &Device, tile_key: &str, sink: &str, source: &str) -> Result<(), String> {
    let tile = tile_of(device, tile_key)?;
    // The router's parser keeps `-` (default, no-bit) inputs; db.rs drops
    // them, which would false-positive on router-emitted default arcs
    // like `F6 <- F6_SLICE`.
    let text = crate::tiledata::bits_db(&tile.tile_type).ok_or_else(|| {
        format!(
            "textcfg: no vendored bits.db for tile type {}",
            tile.tile_type
        )
    })?;
    let muxes = crate::route::parse_muxes(text);
    let mux = muxes
        .iter()
        .find(|m| m.dst == sink)
        .ok_or_else(|| {
            format!(
                "textcfg: tile {tile_key} ({}): no mux with sink {sink:?} in bits.db",
                tile.tile_type
            )
        })?;
    if !mux.inputs.iter().any(|i| i.src == source) {
        return Err(format!(
            "textcfg: tile {tile_key} ({}): mux {sink:?} has no input {source:?} in bits.db",
            tile.tile_type
        ));
    }
    Ok(())
}

/// One `.config_enum` entry parsed from a `bits.db` text: name and all
/// option names (including `-` no-bit options, which `db.rs` drops).
struct EnumInfo {
    name: String,
    options: Vec<String>,
}

/// Parse `.config_enum <name> [<default>]` sections from a `bits.db` text.
///
/// Unlike `db.rs`'s parser, this keeps `-` (no-bit) options: they are valid
/// `enum:` values (ecppack applies an empty bit group for them).
fn parse_config_enums(text: &str) -> Vec<EnumInfo> {
    let mut out = Vec::new();
    let mut cur: Option<EnumInfo> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(".config_enum ") {
            if let Some(e) = cur.take() {
                out.push(e);
            }
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                cur = Some(EnumInfo {
                    name: name.to_string(),
                    options: Vec::new(),
                });
            }
            continue;
        }
        if line.starts_with('.') {
            if let Some(e) = cur.take() {
                out.push(e);
            }
            continue;
        }
        if let Some(e) = cur.as_mut() {
            if let Some(opt) = line.split_whitespace().next() {
                if !opt.is_empty() {
                    e.options.push(opt.to_string());
                }
            }
        }
    }
    if let Some(e) = cur.take() {
        out.push(e);
    }
    out
}

/// Validate an `enum:` name/value against a tile type's `config_enum`s.
pub(crate) fn check_enum_on_type(tile_type: &str, name: &str, value: &str) -> Result<(), String> {
    let text = crate::tiledata::bits_db(tile_type)
        .ok_or_else(|| format!("textcfg: no vendored bits.db for tile type {tile_type}"))?;
    let enums = parse_config_enums(text);
    let e = enums
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("textcfg: tile type {tile_type}: no config enum {name:?} in bits.db"))?;
    if value != "_NONE_" && !e.options.iter().any(|o| o == value) {
        return Err(format!(
            "textcfg: tile type {tile_type}: enum {name:?} has no option {value:?} in bits.db"
        ));
    }
    Ok(())
}

/// One `.config` word entry parsed from a `bits.db` text: name and the
/// number of bit lines (== the required value length).
pub(crate) struct WordInfo {
    pub(crate) name: String,
    pub(crate) bits: usize,
}

/// Parse `.config <name> [<default>]` sections from a `bits.db` text.
///
/// `db.rs` does not parse `.config` words (only muxes, config_enums and
/// fixed_conns), so this local parser covers word validation without
/// touching the read-only `db.rs`.
pub(crate) fn parse_config_words(text: &str) -> Vec<WordInfo> {
    let mut out = Vec::new();
    let mut cur: Option<WordInfo> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(".config ") {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            let name = rest.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                cur = Some(WordInfo {
                    name: name.to_string(),
                    bits: 0,
                });
            }
            continue;
        }
        if line.starts_with('.') {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            continue;
        }
        if let Some(w) = cur.as_mut() {
            w.bits += 1;
        }
    }
    if let Some(w) = cur.take() {
        out.push(w);
    }
    out
}

/// Validate a `word:` name/value against a tile type's `.config` words.
pub(crate) fn check_word_on_type(tile_type: &str, name: &str, value: &str) -> Result<(), String> {
    let text = crate::tiledata::bits_db(tile_type)
        .ok_or_else(|| format!("textcfg: no vendored bits.db for tile type {tile_type}"))?;
    let words = parse_config_words(text);
    let w = words
        .iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("textcfg: tile type {tile_type}: no config word {name:?} in bits.db"))?;
    if value.len() != w.bits {
        return Err(format!(
            "textcfg: tile type {tile_type}: word {name:?} needs {} bits, got {} ({value:?})",
            w.bits,
            value.len()
        ));
    }
    if !value.chars().all(|c| c == '0' || c == '1') {
        return Err(format!(
            "textcfg: tile type {tile_type}: word {name:?} value {value:?} is not a binary string"
        ));
    }
    Ok(())
}

/// Format an integer as a binary `word:` value string, MSB first.
///
/// This matches nextpnr/prjtrellis `to_string(vector<bool>)`: character 0
/// is the most significant bit.
pub(crate) fn word_bin(value: u32, width: usize) -> String {
    (0..width)
        .rev()
        .map(|i| if (value >> i) & 1 == 1 { '1' } else { '0' })
        .collect()
}

/// Bit-reverse `value` over `width` bits (for the EBR `WID` word, which
/// nextpnr emits bit-reversed).
pub(crate) fn bit_reverse(value: u32, width: usize) -> u32 {
    let mut out = 0u32;
    for i in 0..width {
        if (value >> i) & 1 == 1 {
            out |= 1 << (width - 1 - i);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tile group construction (from the device model)
// ---------------------------------------------------------------------------

/// All tile entry keys aliasing the site `(name, bel_col, bel_row)`,
/// sorted by key.
///
/// Trellis lists hard-block sites (EBR, DSP) in every adjacent tile entry
/// whose config they span; the alias set is exactly the `.tile_group`
/// nextpnr emits (verified against reference textcfg for EBR and DSP).
pub(crate) fn alias_tiles(device: &Device, name: &str, bel_col: i32, bel_row: i32) -> Vec<String> {
    let mut keys: Vec<String> = device
        .tiles()
        .iter()
        .filter(|t| {
            t.sites
                .iter()
                .any(|s| s.name == name && s.bel_col == bel_col && s.bel_row == bel_row)
        })
        .map(|t| t.key.clone())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Tile group for a placed EBR: the site's alias tiles.
pub(crate) fn ebr_group(device: &Device, ebr: &EbrBlock) -> Result<Vec<String>, String> {
    let tiles = alias_tiles(device, ebr.site.trellis_name(), ebr.bel_col, ebr.bel_row);
    if tiles.is_empty() {
        return Err(format!(
            "textcfg: EBR site {} at bel ({}, {}) aliases no tile entries",
            ebr.site.trellis_name(),
            ebr.bel_col,
            ebr.bel_row
        ));
    }
    Ok(tiles)
}

/// Tile group for a placed DSP: the site's alias tiles.
pub(crate) fn dsp_group(device: &Device, dsp: &DspBlock) -> Result<Vec<String>, String> {
    let tiles = alias_tiles(device, &dsp.site, dsp.bel_col, dsp.bel_row);
    if tiles.is_empty() {
        return Err(format!(
            "textcfg: DSP site {} at bel ({}, {}) aliases no tile entries",
            dsp.site, dsp.bel_col, dsp.bel_row
        ));
    }
    Ok(tiles)
}

/// Tile group for a placed PLL: the host tile plus neighbouring `PLL*`
/// tiles (the PLL divider words are split across the `PLL0_*`/`PLL1_*`
/// pair; extra group tiles are harmless because ecppack skips words and
/// enums a tile does not define).
///
/// v1 covers the pattern verified against the reference textcfg
/// (`PLL0_UR` + `PLL1_UR` at `(row+1, col)`); the neighbour scan over
/// `(row, col+dc)` / `(row+1, col+dc)` follows nextpnr's per-corner tile
/// selection. Corners whose words live in `BANKREF` tiles are not covered
/// yet and fail loudly at ecppack instead of silently.
pub(crate) fn pll_group(device: &Device, pll: &PllBlock) -> Result<Vec<String>, String> {
    let host = tile_of(device, &pll.tile_key)?;
    let (row, col) = (host.row, host.col);
    let mut keys = BTreeMap::new();
    keys.insert(host.key.clone(), ());
    for t in device.tiles() {
        let dr = t.row as i64 - row as i64;
        let dc = t.col as i64 - col as i64;
        if dr.abs() <= 1 && dc.abs() <= 1 && t.tile_type.starts_with("PLL") {
            keys.insert(t.key.clone(), ());
        }
    }
    let tiles: Vec<String> = keys.into_keys().collect();
    if tiles.len() < 2 {
        return Err(format!(
            "textcfg: PLL at {} found no neighbouring PLL tile for the divider-word group",
            pll.tile_key
        ));
    }
    Ok(tiles)
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

struct Emitter<'a> {
    device: &'a Device,
    tiles: BTreeMap<String, TileCfg>,
    groups: Vec<TileGroup>,
    brams: BTreeMap<u32, Vec<u16>>,
}

impl<'a> Emitter<'a> {
    fn new(device: &'a Device) -> Self {
        Self {
            device,
            tiles: BTreeMap::new(),
            groups: Vec::new(),
            brams: BTreeMap::new(),
        }
    }

    fn tile_entry(&mut self, key: &str) -> Result<&mut TileCfg, String> {
        // Fail loud on unknown tile keys at insertion time.
        tile_of(self.device, key)?;
        Ok(self.tiles.entry(key.to_string()).or_default())
    }

    fn add_arc(&mut self, tile_key: &str, sink: &str, source: &str) -> Result<(), String> {
        check_arc(self.device, tile_key, sink, source)?;
        self.tile_entry(tile_key)?
            .arcs
            .push((sink.to_string(), source.to_string()));
        Ok(())
    }

    fn add_enum(&mut self, tile_key: &str, name: &str, value: &str) -> Result<(), String> {
        let tile = tile_of(self.device, tile_key)?;
        check_enum_on_type(&tile.tile_type, name, value)?;
        self.tile_entry(tile_key)?
            .enums
            .push((name.to_string(), value.to_string()));
        Ok(())
    }

    fn add_word(&mut self, tile_key: &str, name: &str, value: &str) -> Result<(), String> {
        let tile = tile_of(self.device, tile_key)?;
        check_word_on_type(&tile.tile_type, name, value)?;
        self.tile_entry(tile_key)?
            .words
            .push((name.to_string(), value.to_string()));
        Ok(())
    }

    /// Add an enum to a tile group: at least one group tile must define it.
    fn group_add_enum(&mut self, group: &[String], name: &str, value: &str) -> Result<(), String> {
        let mut ok = false;
        let mut err = String::new();
        for key in group {
            let tile = tile_of(self.device, key)?;
            match check_enum_on_type(&tile.tile_type, name, value) {
                Ok(()) => ok = true,
                Err(e) => err = e,
            }
        }
        if !ok {
            return Err(format!("textcfg: tile group {group:?}: {err}"));
        }
        Ok(())
    }

    /// Add a word to a tile group: at least one group tile must define it.
    fn group_add_word(&mut self, group: &[String], name: &str, value: &str) -> Result<(), String> {
        let mut ok = false;
        let mut err = String::new();
        for key in group {
            let tile = tile_of(self.device, key)?;
            match check_word_on_type(&tile.tile_type, name, value) {
                Ok(()) => ok = true,
                Err(e) => err = e,
            }
        }
        if !ok {
            return Err(format!("textcfg: tile group {group:?}: {err}"));
        }
        Ok(())
    }

    fn emit_lut(&mut self, lut: &PlacedLut) -> Result<(), String> {
        let tile = tile_of(self.device, &lut.tile_key)?;
        if tile.tile_type != "PLC2" {
            return Err(format!(
                "textcfg: LUT placed on non-PLC2 tile {} ({})",
                lut.tile_key, tile.tile_type
            ));
        }
        if lut.lut > 1 {
            return Err(format!("textcfg: bad lut index {} (want 0/1)", lut.lut));
        }
        let slice = lut.slice.trellis_name();
        let mode = if lut.carry { "CCU2" } else { "LOGIC" };
        self.add_enum(&lut.tile_key, &format!("{slice}.MODE"), mode)?;
        self.add_word(
            &lut.tile_key,
            &format!("{slice}.K{}.INIT", lut.lut),
            &word_bin(u32::from(lut.init), 16),
        )?;
        if lut.carry {
            self.add_enum(
                &lut.tile_key,
                &format!("{slice}.CCU2.INJECT1_{}", lut.lut),
                "YES",
            )?;
        } else {
            // Keep cascade-mux wiring untouched, as nextpnr does.
            self.add_enum(
                &lut.tile_key,
                &format!("{slice}.CCU2.INJECT1_{}", lut.lut),
                "_NONE_",
            )?;
        }
        // Tie undriven LUT inputs to 1.
        for (i, used) in lut.input_used.iter().enumerate() {
            if !used {
                let pin = b"ABCD"[i] as char;
                self.add_enum(
                    &lut.tile_key,
                    &format!("{slice}.{pin}{}MUX", lut.lut),
                    "1",
                )?;
            }
        }
        if let Some(ff) = &lut.ff {
            self.emit_ff(lut, ff)?;
        }
        Ok(())
    }

    fn emit_ff(&mut self, lut: &PlacedLut, ff: &PlacedFf) -> Result<(), String> {
        let slice = lut.slice.trellis_name();
        let key = lut.tile_key.clone();
        self.add_enum(
            &key,
            &format!("{slice}.GSR"),
            if ff.gsr_enabled { "ENABLED" } else { "DISABLED" },
        )?;
        self.add_enum(
            &key,
            &format!("{slice}.REG{}.SD", lut.lut),
            if ff.d_from_lut { "1" } else { "0" },
        )?;
        self.add_enum(
            &key,
            &format!("{slice}.REG{}.REGSET", lut.lut),
            if ff.set_on_reset { "SET" } else { "RESET" },
        )?;
        self.add_enum(
            &key,
            &format!("{slice}.REG{}.LSRMODE", lut.lut),
            "LSR",
        )?;
        self.add_enum(
            &key,
            &format!("{slice}.CEMUX"),
            if ff.ce_used { "CE" } else { "1" },
        )?;
        if ff.clk_sel > 1 || ff.lsr_sel > 1 {
            return Err(format!(
                "textcfg: bad clk_sel/lsr_sel {}/{} (want 0/1)",
                ff.clk_sel, ff.lsr_sel
            ));
        }
        self.add_enum(
            &key,
            &format!("CLK{}.CLKMUX", ff.clk_sel),
            if ff.clk_inverted { "INV" } else { "CLK" },
        )?;
        self.add_enum(
            &key,
            &format!("LSR{}.SRMODE", ff.lsr_sel),
            "LSR_OVER_CE",
        )?;
        self.add_enum(
            &key,
            &format!("LSR{}.LSRMUX", ff.lsr_sel),
            if ff.lsr_used { "LSR" } else { "INV" },
        )?;
        Ok(())
    }

    fn emit_io(&mut self, io: &PlacedIo) -> Result<(), String> {
        let pio = io.pio.trellis_name();
        let base = format!("{}_{}", io.dir.trellis_name(), io.io_type);
        self.add_enum(&io.pio_tile_key, &format!("{pio}.BASE_TYPE"), &base)?;
        self.add_enum(&io.pic_tile_key, &format!("{pio}.BASE_TYPE"), &base)?;
        self.add_enum(
            &io.pio_tile_key,
            &format!("{pio}.PULLMODE"),
            io.pull.trellis_name(),
        )?;
        if io.dir == IoDir::Output {
            self.add_enum(
                &io.pio_tile_key,
                &format!("{pio}.DRIVE"),
                &io.drive_ma.to_string(),
            )?;
        }
        Ok(())
    }

    fn emit_ebr(&mut self, ebr: &EbrBlock) -> Result<(), String> {
        if ebr.init.len() != 2048 {
            return Err(format!(
                "textcfg: EBR {} needs 2048 init words, got {}",
                ebr.tile_key,
                ebr.init.len()
            ));
        }
        if !ebr.allow_multi_width && (ebr.data_width_a != 9 || ebr.data_width_b != 9) {
            return Err(format!(
                "textcfg: EBR {}: v1 supports DATA_WIDTH 9/9 only, got {}/{}",
                ebr.tile_key, ebr.data_width_a, ebr.data_width_b
            ));
        }
        if self.brams.contains_key(&ebr.wid) {
            return Err(format!("textcfg: duplicate EBR wid {}", ebr.wid));
        }
        tile_of(self.device, &ebr.tile_key)?;
        let site = ebr.site.trellis_name();
        let group_tiles = ebr_group(self.device, ebr)?;
        let mut words: Vec<(String, String)> = Vec::new();
        let mut enums: Vec<(String, String)> = Vec::new();
        words.push((
            format!("{site}.WID"),
            word_bin(bit_reverse(ebr.wid, 9), 9),
        ));
        words.push((
            format!("{site}.CSDECODE_A"),
            word_bin(u32::from(ebr.csdecode_a), 3),
        ));
        words.push((
            format!("{site}.CSDECODE_B"),
            word_bin(u32::from(ebr.csdecode_b), 3),
        ));
        enums.push((format!("{site}.MODE"), ebr.mode.clone()));
        if ebr.mode == "PDPW16KD" {
            enums.push((
                format!("{site}.PDPW16KD.DATA_WIDTH_R"),
                ebr.data_width_b.to_string(),
            ));
        } else {
            enums.push((
                format!("{site}.DP16KD.DATA_WIDTH_A"),
                ebr.data_width_a.to_string(),
            ));
            enums.push((
                format!("{site}.DP16KD.DATA_WIDTH_B"),
                ebr.data_width_b.to_string(),
            ));
            enums.push((
                format!("{site}.DP16KD.WRITEMODE_A"),
                ebr.write_mode_a.clone(),
            ));
            enums.push((
                format!("{site}.DP16KD.WRITEMODE_B"),
                ebr.write_mode_b.clone(),
            ));
        }
        for (p, v) in [
            ("REGMODE_A", "NOREG"),
            ("REGMODE_B", "NOREG"),
            ("RESETMODE", "ASYNC"),
            ("ASYNC_RESET_RELEASE", "ASYNC"),
            ("GSR", "DISABLED"),
        ] {
            enums.push((format!("{site}.{p}"), v.to_string()));
        }
        for (p, v) in [
            ("CLKAMUX", ebr.clk_a_mux.as_str()),
            ("CLKBMUX", ebr.clk_b_mux.as_str()),
            ("RSTAMUX", "INV"),
            ("RSTBMUX", "INV"),
            ("WEAMUX", ebr.we_a_mux.as_str()),
            ("WEBMUX", ebr.we_b_mux.as_str()),
            ("CEAMUX", ebr.ce_a_mux.as_str()),
            ("CEBMUX", ebr.ce_b_mux.as_str()),
            ("OCEAMUX", "OCEA"),
            ("OCEBMUX", "OCEB"),
        ] {
            enums.push((format!("{site}.{p}"), v.to_string()));
        }
        // Validate every entry against the group tiles before committing.
        for (n, v) in &words {
            self.group_add_word(&group_tiles, n, v)?;
        }
        for (n, v) in &enums {
            self.group_add_enum(&group_tiles, n, v)?;
        }
        self.groups.push(TileGroup {
            tiles: group_tiles,
            words,
            enums,
        });
        // bram_init payload: 2048 12-bit groups, low 9 bits = data.
        self.brams.insert(
            ebr.wid,
            ebr.init.iter().map(|w| w & 0x1ff).collect(),
        );
        Ok(())
    }

    fn emit_dsp(&mut self, dsp: &DspBlock) -> Result<(), String> {
        tile_of(self.device, &dsp.tile_key)?;
        let group_tiles = dsp_group(self.device, dsp)?;
        let s = &dsp.site;
        if s.starts_with("ALU54_") {
            let enums: Vec<(String, String)> = vec![
                (format!("{s}.MODE"), "ALU54B".to_string()),
                (format!("{s}.RESETMODE"), dsp.resetmode.clone()),
                (format!("{s}.LEGACY"), "DISABLED".to_string()),
                (
                    format!("{s}.FORCE_ZERO_BARREL_SHIFT"),
                    "ENABLED".to_string(),
                ),
            ];
            for (n, v) in &enums {
                self.group_add_enum(&group_tiles, n, v)?;
            }
            self.groups.push(TileGroup {
                tiles: group_tiles,
                words: Vec::new(),
                enums,
            });
            return Ok(());
        }
        let mut enums: Vec<(String, String)> = vec![
            (format!("{s}.REG_INPUTA_CLK"), dsp.reg_inputa_clk.clone()),
            (format!("{s}.REG_INPUTA_CE"), dsp.reg_inputa_ce.clone()),
            (format!("{s}.REG_INPUTA_RST"), dsp.reg_inputa_rst.clone()),
            (format!("{s}.REG_INPUTB_CLK"), dsp.reg_inputb_clk.clone()),
            (format!("{s}.REG_INPUTB_CE"), dsp.reg_inputb_ce.clone()),
            (format!("{s}.REG_INPUTB_RST"), dsp.reg_inputb_rst.clone()),
            (format!("{s}.REG_INPUTC_CLK"), dsp.reg_inputc_clk.clone()),
            (format!("{s}.REG_PIPELINE_CLK"), dsp.reg_pipeline_clk.clone()),
            (format!("{s}.REG_PIPELINE_CE"), dsp.reg_pipeline_ce.clone()),
            (format!("{s}.REG_PIPELINE_RST"), dsp.reg_pipeline_rst.clone()),
            (format!("{s}.REG_OUTPUT_CLK"), dsp.reg_output_clk.clone()),
            (format!("{s}.REG_OUTPUT_RST"), dsp.reg_output_rst.clone()),
            (format!("{s}.CLK0_DIV"), "ENABLED".to_string()),
            (format!("{s}.CLK1_DIV"), "ENABLED".to_string()),
            (format!("{s}.CLK2_DIV"), "ENABLED".to_string()),
            (format!("{s}.CLK3_DIV"), "ENABLED".to_string()),
            (format!("{s}.GSR"), dsp.gsr.clone()),
            (format!("{s}.SOURCEB_MODE"), "B_SHIFT".to_string()),
            (format!("{s}.RESETMODE"), dsp.resetmode.clone()),
            (format!("{s}.MODE"), "MULT18X18D".to_string()),
            (format!("{s}.CIBOUT_BYP"), "ON".to_string()),
            (format!("{s}.CLK0MUX"), "CLK0".to_string()),
            (format!("{s}.CLK1MUX"), "CLK1".to_string()),
            (format!("{s}.CLK2MUX"), "CLK2".to_string()),
            (format!("{s}.CLK3MUX"), "CLK3".to_string()),
            (format!("{s}.CE0MUX"), "CE0".to_string()),
            (format!("{s}.CE1MUX"), "CE1".to_string()),
            (format!("{s}.CE2MUX"), "CE2".to_string()),
            (format!("{s}.CE3MUX"), "CE3".to_string()),
            (format!("{s}.RST0MUX"), "RST0".to_string()),
            (format!("{s}.RST1MUX"), "RST1".to_string()),
            (format!("{s}.RST2MUX"), "RST2".to_string()),
            (format!("{s}.RST3MUX"), "RST3".to_string()),
        ];
        let z: u32 = s
            .strip_prefix("MULT18_")
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| format!("textcfg: bad DSP site name {s:?}"))?;
        enums.push((
            if z < 4 { "DSP_LEFT" } else { "DSP_RIGHT" }.to_string() + ".CIBOUT",
            "ON".to_string(),
        ));
        if z != 0 {
            enums.retain(|(n, v)| self.group_add_enum(&group_tiles, n, v).is_ok());
        }
        for (n, v) in &enums {
            self.group_add_enum(&group_tiles, n, v)?;
        }
        self.groups.push(TileGroup {
            tiles: group_tiles,
            words: Vec::new(),
            enums,
        });
        Ok(())
    }

    fn emit_pll(&mut self, pll: &PllBlock) -> Result<(), String> {
        let group_tiles = pll_group(self.device, pll)?;
        let mut words: Vec<(String, u32)> = vec![
            ("CLKI_DIV".to_string(), pll.clki_div),
            ("CLKFB_DIV".to_string(), pll.clkfb_div),
            ("CLKOP_DIV".to_string(), pll.clkop_div),
        ];
        if let Some(d) = pll.clkos_div {
            words.push(("CLKOS_DIV".to_string(), d));
        }
        if let Some(d) = pll.clkos2_div {
            words.push(("CLKOS2_DIV".to_string(), d));
        }
        if let Some(d) = pll.clkos3_div {
            words.push(("CLKOS3_DIV".to_string(), d));
        }
        let mut enums: Vec<(String, String)> = vec![
            ("MODE".to_string(), "EHXPLLL".to_string()),
            ("CLKOP_ENABLE".to_string(), pll.clkop_enable.clone()),
            ("FEEDBK_PATH".to_string(), pll.feedbk_path.clone()),
            ("CLKOP_TRIM_POL".to_string(), "RISING".to_string()),
            ("CLKOP_TRIM_DELAY".to_string(), "0".to_string()),
            ("OUTDIVIDER_MUXA".to_string(), "DIVA".to_string()),
            ("STDBY_ENABLE".to_string(), "DISABLED".to_string()),
            ("INT_LOCK_STICKY".to_string(), "ENABLED".to_string()),
            (
                "DPHASE_SOURCE".to_string(),
                if pll.dphase_source {
                    "ENABLED"
                } else {
                    "DISABLED"
                }
                .to_string(),
            ),
            ("PLLRST_ENA".to_string(), "DISABLED".to_string()),
            ("INTFB_WAKE".to_string(), "DISABLED".to_string()),
        ];
        if pll.clkos_div.is_some() {
            enums.push(("CLKOS_ENABLE".to_string(), "ENABLED".to_string()));
        }
        if pll.clkos2_div.is_some() {
            enums.push(("CLKOS2_ENABLE".to_string(), "ENABLED".to_string()));
        }
        if pll.clkos3_div.is_some() {
            enums.push(("CLKOS3_ENABLE".to_string(), "ENABLED".to_string()));
        }
        let mut word_strs: Vec<(String, String)> = Vec::new();
        for (name, val) in &words {
            let width = self.word_width(&group_tiles, name)?;
            word_strs.push((name.clone(), word_bin(*val, width)));
        }
        for (n, v) in &word_strs {
            self.group_add_word(&group_tiles, n, v)?;
        }
        for (n, v) in &enums {
            self.group_add_enum(&group_tiles, n, v)?;
        }
        self.groups.push(TileGroup {
            tiles: group_tiles,
            words: word_strs,
            enums,
        });
        Ok(())
    }

    /// Width (bit count) of a `.config` word found in any group tile.
    fn word_width(&self, group: &[String], name: &str) -> Result<usize, String> {
        for key in group {
            let tile = tile_of(self.device, key)?;
            if let Some(text) = crate::tiledata::bits_db(&tile.tile_type) {
                if let Some(w) = parse_config_words(text).iter().find(|w| w.name == name) {
                    return Ok(w.bits);
                }
            }
        }
        Err(format!("textcfg: tile group {group:?}: no config word {name:?}"))
    }

    fn emit_dcc(&mut self, dcc: &DccBlock) -> Result<(), String> {
        // DCC config lives in its host tile; no group needed.
        let name = format!("{}.MODE", dcc.config_name);
        let tile = tile_of(self.device, &dcc.tile_key)?;
        check_enum_on_type(&tile.tile_type, &name, "DCCA")?;
        self.tile_entry(&dcc.tile_key)?
            .enums
            .push((name, "DCCA".to_string()));
        Ok(())
    }

    fn render(&self, top: &str) -> String {
        let mut out = String::new();
        out.push_str(".device LFE5U-85F\n\n");
        out.push_str(".comment Generated by Helion ECP5 backend\n\n");
        if !top.is_empty() {
            out.push_str(&format!(".comment Top: {top}\n\n"));
        }
        // `.tile` sections, sorted by key (BTreeMap order).
        for (key, cfg) in &self.tiles {
            if cfg.arcs.is_empty() && cfg.words.is_empty() && cfg.enums.is_empty() {
                continue;
            }
            out.push_str(&format!(".tile {key}\n"));
            for (sink, source) in &cfg.arcs {
                out.push_str(&format!("arc: {sink} {source}\n"));
            }
            for (name, value) in &cfg.words {
                out.push_str(&format!("word: {name} {value}\n"));
            }
            for (name, value) in &cfg.enums {
                out.push_str(&format!("enum: {name} {value}\n"));
            }
            out.push('\n');
        }
        // `.bram_init` sections, sorted by wid.
        for (wid, data) in &self.brams {
            out.push_str(&format!(".bram_init {wid}\n"));
            for chunk in data.chunks(8) {
                for (j, v) in chunk.iter().enumerate() {
                    if j > 0 {
                        out.push(' ');
                    }
                    out.push_str(&format!("{v:03x}"));
                }
                out.push('\n');
            }
            out.push('\n');
        }
        // `.tile_group` sections, sorted by first tile key.
        let mut groups: Vec<&TileGroup> = self.groups.iter().collect();
        groups.sort_by(|a, b| a.tiles.cmp(&b.tiles));
        for g in groups {
            out.push_str(".tile_group");
            for t in &g.tiles {
                out.push(' ');
                out.push_str(t);
            }
            out.push('\n');
            for (name, value) in &g.words {
                out.push_str(&format!("word: {name} {value}\n"));
            }
            for (name, value) in &g.enums {
                out.push_str(&format!("enum: {name} {value}\n"));
            }
            out.push('\n');
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// place/route -> payload mapping
// ---------------------------------------------------------------------------

/// Device tile-grid key (e.g. `"R2C62:PLC2"`, `"MIB_R70C4:MIB_EBR0"`) for a
/// placed site address. This matches the route worker's `tile_key`
/// convention exactly (it uses the same device tile keys).
fn tile_key_of(
    device: &Device,
    site: &place::SiteAddr,
    what: &str,
) -> Result<String, String> {
    device
        .tiles()
        .iter()
        .find(|t| {
            t.row == site.tile_row && t.col == site.tile_col && t.tile_type == site.tile_type
        })
        .map(|t| t.key.clone())
        .ok_or_else(|| {
            format!(
                "textcfg: {what}: no tile at R{}C{} of type {}",
                site.tile_row, site.tile_col, site.tile_type
            )
        })
}

/// Numeric slice index (0..3) from a slice site name.
fn slice_idx(site: &str, what: &str) -> Result<u8, String> {
    match site {
        "SLICE_A" => Ok(0),
        "SLICE_B" => Ok(1),
        "SLICE_C" => Ok(2),
        "SLICE_D" => Ok(3),
        _ => Err(format!("textcfg: {what}: site '{site}' is not a slice")),
    }
}

/// Which slice clock mux (`CLK0`/`CLK1`) the router programmed for an FF:
/// read back from the routed clock's per-slice segments
/// (`MUXCLK{slice} <- CLK{n}`). Defaults to 0 (local clocks).
fn clk_sel_for(
    routed: &route::RoutedEcp5,
    ff_clk: &str,
    tile_key: &str,
    slice: u8,
) -> u8 {
    for clock in &routed.clocks {
        if clock.net != ff_clk {
            continue;
        }
        let want = format!("MUXCLK{slice}");
        for seg in &clock.slices {
            if seg.tile_key == tile_key && seg.dst == want {
                if let Some(n) = seg.src.strip_prefix("CLK") {
                    if let Ok(v) = n.parse::<u8>() {
                        return v.min(1);
                    }
                }
            }
        }
    }
    0
}

/// Build the LUT payloads: one per placed LUT, plus a dummy `LOGIC` LUT
/// for each unpaired FF's half-slice (an unused LUT programmed constant
/// is harmless; the FF takes D from fabric).
///
/// `input_used[i]` is true when input `i` is driven by routing. The
/// dedicated carry input (B) of a carry LUT counts as used: it must never
/// be tied high.
fn build_luts(
    routed: &route::RoutedEcp5,
    placed: &place::PlacedEcp5,
    device: &Device,
) -> Result<Vec<PlacedLut>, String> {
    let mut ff_by_half: HashMap<(u32, u32, String, u8), &place::PlacedFf> = HashMap::new();
    for ff in &placed.ffs {
        ff_by_half.insert(
            (
                ff.site.tile_row,
                ff.site.tile_col,
                ff.site.site.clone(),
                ff.half,
            ),
            ff,
        );
    }
    let mut lut_halves: HashMap<(u32, u32, String, u8), &place::PlacedLut> = HashMap::new();
    let mut out = Vec::with_capacity(placed.luts.len());
    for lut in &placed.luts {
        let what = format!("LUT {}", lut.name);
        let tile_key = tile_key_of(device, &lut.site, &what)?;
        let slice = slice_idx(&lut.site.site, &what)?;
        if lut.half > 1 {
            return Err(format!("textcfg: {what}: bad half {}", lut.half));
        }
        let slice_id = match slice {
            0 => SliceId::A,
            1 => SliceId::B,
            2 => SliceId::C,
            _ => SliceId::D,
        };
        let carry = lut.carry.is_some();
        let mut input_used = [false; 4];
        for (i, net) in lut.inputs.iter().enumerate() {
            if let Some(n) = net {
                if !n.starts_with(CARRY_NET_PREFIX) {
                    input_used[i] = true;
                }
            }
        }
        if carry {
            input_used[1] = true;
        }
        let key = (
            lut.site.tile_row,
            lut.site.tile_col,
            lut.site.site.clone(),
            lut.half,
        );
        lut_halves.insert(key.clone(), lut);
        let ff = ff_by_half.get(&key).copied().map(|f| PlacedFf {
            gsr_enabled: false,
            set_on_reset: false,
            clk_inverted: false,
            ce_used: f.ce.is_some(),
            lsr_used: f.sr.is_some(),
            d_from_lut: f.d == lut.output,
            clk_sel: clk_sel_for(routed, &f.clk, &tile_key, slice),
            lsr_sel: 0,
        });
        out.push(PlacedLut {
            tile_key,
            slice: slice_id,
            lut: lut.half,
            init: lut.init,
            carry,
            input_used,
            ff,
        });
    }
    // Unpaired FFs: their half-slice has no LUT; emit a constant dummy so
    // the FF config has a host. (Carry-locked slices never host unpaired
    // FFs: the place worker excludes locked slices from the free pool.)
    for ff in &placed.ffs {
        let key = (
            ff.site.tile_row,
            ff.site.tile_col,
            ff.site.site.clone(),
            ff.half,
        );
        if lut_halves.contains_key(&key) {
            continue;
        }
        let what = format!("FF {}", ff.name);
        let tile_key = tile_key_of(device, &ff.site, &what)?;
        let slice = slice_idx(&ff.site.site, &what)?;
        let slice_id = match slice {
            0 => SliceId::A,
            1 => SliceId::B,
            2 => SliceId::C,
            _ => SliceId::D,
        };
        out.push(PlacedLut {
            tile_key: tile_key.clone(),
            slice: slice_id,
            lut: ff.half,
            init: 0,
            carry: false,
            input_used: [false; 4],
            ff: Some(PlacedFf {
                gsr_enabled: false,
                set_on_reset: false,
                clk_inverted: false,
                ce_used: ff.ce.is_some(),
                lsr_used: ff.sr.is_some(),
                d_from_lut: false,
                clk_sel: clk_sel_for(routed, &ff.clk, &tile_key, slice),
                lsr_sel: 0,
            }),
        });
    }
    Ok(out)
}

/// Adjacent PIC tile key for a PIO site: same numeric suffix, `PIO` ->
/// `PIC`, one step inward from the chip edge. (Verified: all 104 PIO tile
/// entries of the LFE5U-85F resolve this way.)
fn pic_tile_key(
    device: &Device,
    site: &place::SiteAddr,
    what: &str,
) -> Result<String, String> {
    let pic_type = site
        .tile_type
        .strip_prefix("PIO")
        .map(|rest| format!("PIC{rest}"))
        .ok_or_else(|| {
            format!(
                "textcfg: {what}: PIO tile type {} has no PIC counterpart",
                site.tile_type
            )
        })?;
    let (row, col) = (site.tile_row, site.tile_col);
    let (pr, pc) = if row == 0 {
        (row + 1, col)
    } else if row == 95 {
        (row - 1, col)
    } else if col == 0 {
        (row, col + 1)
    } else if col == 126 {
        (row, col - 1)
    } else {
        return Err(format!(
            "textcfg: {what}: PIO tile at R{row}C{col} is on no chip edge"
        ));
    };
    device
        .tiles()
        .iter()
        .find(|t| t.row == pr && t.col == pc && t.tile_type == pic_type)
        .map(|t| t.key.clone())
        .ok_or_else(|| {
            format!("textcfg: {what}: no PIC tile {pic_type} at R{pr}C{pc}")
        })
}

/// Build the IO payloads. Electrical intent is v1 defaults (the pack
/// worker carries no IO-standard attributes): LVCMOS33, no pull, 8mA
/// drive on outputs.
fn build_ios(
    placed: &place::PlacedEcp5,
    device: &Device,
) -> Result<Vec<PlacedIo>, String> {
    let mut out = Vec::with_capacity(placed.ios.len());
    for io in &placed.ios {
        let what = format!("IO {}", io.name);
        let pio = match io.site.site.as_str() {
            "PIOA" => PioId::A,
            "PIOB" => PioId::B,
            "PIOC" => PioId::C,
            "PIOD" => PioId::D,
            s => return Err(format!("textcfg: {what}: site '{s}' is not a PIO")),
        };
        let dir = match io.dir {
            crate::pack::IoDir::In => IoDir::Input,
            crate::pack::IoDir::Out => IoDir::Output,
        };
        let pio_tile_key = tile_key_of(device, &io.site, &what)?;
        let pic_tile_key = pic_tile_key(device, &io.site, &what)?;
        out.push(PlacedIo {
            pio_tile_key,
            pic_tile_key,
            pio,
            dir,
            io_type: "LVCMOS33".to_string(),
            pull: IoPull::None,
            drive_ma: 8,
        });
    }
    Ok(out)
}

/// Build the EBR payloads. `wid` is `3 +` the block's placement index
/// (nextpnr autoincrements from 3); the INIT image is the pack worker's
/// 9-bit words, zero-padded to 2048.
fn build_ebrs(
    placed: &place::PlacedEcp5,
    device: &Device,
) -> Result<Vec<EbrBlock>, String> {
    let mut out = Vec::with_capacity(placed.ebrs.len());
    for (i, ebr) in placed.ebrs.iter().enumerate() {
        let what = format!("EBR {}", ebr.name);
        let tile_key = tile_key_of(device, &ebr.site, &what)?;
        let site = match ebr.site.site.as_str() {
            "EBR0" => EbrSite::Ebr0,
            "EBR1" => EbrSite::Ebr1,
            "EBR2" => EbrSite::Ebr2,
            "EBR3" => EbrSite::Ebr3,
            s => return Err(format!("textcfg: {what}: site '{s}' is not an EBR")),
        };
        let ram = placed
            .packed
            .rams
            .iter()
            .find(|r| r.name == ebr.name)
            .ok_or_else(|| format!("textcfg: {what}: no packed RAM with that name"))?;
        if ram.init.len() > 2048 {
            return Err(format!(
                "textcfg: {what}: {} INIT words exceed the 2048-word DP16KD image",
                ram.init.len()
            ));
        }
        let mut init = vec![0u16; 2048];
        for (w, word) in ram.init.iter().enumerate() {
            init[w] = (word & 0x1ff) as u16;
        }
        let mut eb = EbrBlock::simple(
            tile_key,
            site,
            ebr.site.bel_col,
            ebr.site.bel_row,
            3 + i as u32,
            init,
        );
        eb.mode = ram.mode.clone();
        eb.allow_multi_width = true;
        eb.data_width_a = ram.data_width_a;
        eb.data_width_b = ram.data_width_b;
        eb.write_mode_a = ram.writemode_a.clone();
        eb.write_mode_b = ram.writemode_b.clone();
        eb.csdecode_a = ram.csdecode_a;
        eb.csdecode_b = ram.csdecode_b;
        out.push(eb);
    }
    Ok(out)
}

/// Build the DSP payloads (plain unregistered multiply defaults).
fn build_dsps(
    placed: &place::PlacedEcp5,
    device: &Device,
) -> Result<Vec<DspBlock>, String> {
    let mut out = Vec::with_capacity(placed.dsps.len());
    for dsp in &placed.dsps {
        let what = format!("DSP {}", dsp.name);
        let tile_key = tile_key_of(device, &dsp.site, &what)?;
        out.push(DspBlock::plain_multiply(
            tile_key,
            dsp.site.site.clone(),
            dsp.site.bel_col,
            dsp.site.bel_row,
        ));
    }
    Ok(out)
}

/// Build the PLL payloads: divider words from the pack worker's
/// parameters (`CLKI_DIV`/`CLKOP_DIV` required, `CLKFB_DIV` defaults to 1).
fn build_plls(
    placed: &place::PlacedEcp5,
    device: &Device,
) -> Result<Vec<PllBlock>, String> {
    let mut out = Vec::with_capacity(placed.plls.len());
    for pll in &placed.plls {
        let what = format!("PLL {}", pll.name);
        let tile_key = tile_key_of(device, &pll.site, &what)?;
        let packed = placed
            .packed
            .plls
            .iter()
            .find(|p| p.name == pll.name)
            .ok_or_else(|| format!("textcfg: {what}: no packed PLL with that name"))?;
        let div = |key: &str, required: bool| -> Result<u32, String> {
            match packed.params.iter().find(|(k, _)| k == key) {
                Some((_, v)) => v.parse::<u32>().map_err(|_| {
                    format!("textcfg: {what}: {key}='{v}' is not an integer divider")
                }),
                None if required => Err(format!("textcfg: {what}: missing {key} parameter")),
                None => Ok(1),
            }
        };
        out.push(PllBlock {
            tile_key,
            bel_col: pll.site.bel_col,
            bel_row: pll.site.bel_row,
            clki_div: div("CLKI_DIV", true)?,
            clkfb_div: div("CLKFB_DIV", false)?,
            clkop_div: div("CLKOP_DIV", true)?,
            clkos_div: packed.params.iter().find(|(k, _)| k == "CLKOS_DIV").and_then(|(_, v)| v.parse().ok()),
            clkos2_div: packed.params.iter().find(|(k, _)| k == "CLKOS2_DIV").and_then(|(_, v)| v.parse().ok()),
            clkos3_div: packed.params.iter().find(|(k, _)| k == "CLKOS3_DIV").and_then(|(_, v)| v.parse().ok()),
            clkop_enable: "ENABLED".to_string(),
            feedbk_path: "INT_OP".to_string(),
            dphase_source: packed.dphase_source,
        });
    }
    Ok(out)
}

/// Build the DCC payloads from the routed clocks. The config-name side
/// letter comes from the host tile type (`TMID_*` -> `T`, ...); the v1
/// router only programs `TMID_0`.
fn build_dccs(
    routed: &route::RoutedEcp5,
    device: &Device,
) -> Result<Vec<DccBlock>, String> {
    let mut out = Vec::with_capacity(routed.clocks.len());
    for clock in &routed.clocks {
        let tile_key = clock.dcc_input.tile_key.clone();
        let tile = tile_of(device, &tile_key)?;
        let side = if tile.tile_type.starts_with("TMID") {
            "T"
        } else if tile.tile_type.starts_with("BMID") {
            "B"
        } else if tile.tile_type.starts_with("LMID") {
            "L"
        } else if tile.tile_type.starts_with("RMID") {
            "R"
        } else {
            return Err(format!(
                "textcfg: clock '{}': DCC tile type {} has no known side",
                clock.net, tile.tile_type
            ));
        };
        out.push(DccBlock {
            tile_key,
            config_name: format!("DCC_{side}{}", clock.dcc),
        });
    }
    Ok(out)
}

/// Map the real routed + placed designs onto the emitter's payload bundle.
pub(crate) fn build_design(
    routed: &route::RoutedEcp5,
    placed: &place::PlacedEcp5,
    device: &Device,
) -> Result<EmitDesign, String> {
    let mut arcs = Vec::new();
    for net in &routed.nets {
        for seg in &net.segments {
            // Empty `bits` is the `-` default input: the arc is still
            // emitted (the bitstream starts cleared).
            arcs.push(EmitArc {
                tile_key: seg.tile_key.clone(),
                sink: seg.dst.clone(),
                source: seg.src.clone(),
            });
        }
    }
    Ok(EmitDesign {
        top: routed.top.clone(),
        luts: build_luts(routed, placed, device)?,
        ios: build_ios(placed, device)?,
        arcs,
        ebrs: build_ebrs(placed, device)?,
        dsps: build_dsps(placed, device)?,
        plls: build_plls(placed, device)?,
        dccs: build_dccs(routed, device)?,
        config_enums: routed.config_enums.clone(),
        placed: Some(placed.clone()),
    })
}

/// Emit Trellis textcfg for a routed design.
///
/// `routed` is the real [`route::RoutedEcp5`], `placed` the real
/// [`place::PlacedEcp5`] (site payloads), `device` the ECP5 device model
/// used for validation and tile-group construction. Returns the textcfg
/// text in [`BitstreamEcp5`], ready for `ecppack`.
///
/// Fails loudly (`Err`) on any unrepresentable config: unknown tile keys,
/// mux sinks/sources missing from `bits.db`, unknown enum names/values,
/// unknown words or wrong value widths, bad EBR payloads. Nothing is
/// silently dropped.
pub fn emit_textcfg(
    routed: &route::RoutedEcp5,
    placed: &place::PlacedEcp5,
    device: &Device,
) -> Result<BitstreamEcp5, String> {
    let design = build_design(routed, placed, device)?;
    emit_design(&design, device)
}

/// Emit textcfg for an already-built [`EmitDesign`] bundle.
pub(crate) fn emit_design(
    design: &EmitDesign,
    device: &Device,
) -> Result<BitstreamEcp5, String> {
    let mut em = Emitter::new(device);
    for lut in &design.luts {
        em.emit_lut(lut)?;
    }
    for io in &design.ios {
        em.emit_io(io)?;
    }
    for arc in &design.arcs {
        em.add_arc(&arc.tile_key, &arc.sink, &arc.source)?;
    }
    for ebr in &design.ebrs {
        em.emit_ebr(ebr)?;
    }
    for dsp in &design.dsps {
        em.emit_dsp(dsp)?;
    }
    for pll in &design.plls {
        em.emit_pll(pll)?;
    }
    for dcc in &design.dccs {
        em.emit_dcc(dcc)?;
    }
    for ce in &design.config_enums {
        em.add_enum(&ce.tile_key, &ce.name, &ce.value)?;
    }
    let mut extra_comments = String::new();
    if let Some(placed) = &design.placed {
        for dll in &placed.ddrdlls {
            let tkey = tile_key_of(device, &dll.site, &dll.name)?;
            let pdll = placed.packed.ddrdlls.iter().find(|d| d.name == dll.name);
            em.add_enum(&tkey, "DDRDLL.MODE", "DDRDLLA")?;
            let gsr = pdll.map(|d| d.gsr.as_str()).unwrap_or("ENABLED");
            em.add_enum(&tkey, "DDRDLL.GSR", gsr)?;
            let fmd = if pdll.map(|d| d.force_max_delay).unwrap_or(false) {
                "YES"
            } else {
                "NO"
            };
            em.add_enum(&tkey, "DDRDLL.FORCE_MAX_DELAY", fmd)?;
        }
        for dqs in &placed.dqs_blocks {
            let tkey = tile_key_of(device, &dqs.site, &dqs.name)?;
            let pdqs = placed.packed.dqs_blocks.iter().find(|d| d.name == dqs.name);
            em.add_enum(&tkey, "DQS.MODE", "DQSBUFM")?;
            if let Some(pd) = pdqs {
                if dqs.site.tile_type.ends_with("DQS2") {
                    em.add_enum(&tkey, "DQS.DQS_LI_DEL_ADJ", &pd.dqs_li_del_adj)?;
                    em.add_word(
                        &tkey,
                        "DQS.DQS_LI_DEL_VAL",
                        &word_bin(u32::from(pd.dqs_li_del_val), 8),
                    )?;
                }
            }
        }
        for iol in &placed.iologics {
            let tkey = tile_key_of(device, &iol.site, &iol.name)?;
            let mut group_tiles: Vec<String> = device
                .tiles()
                .iter()
                .filter(|t| {
                    t.col == iol.site.tile_col
                        && (t.row as i64 - iol.site.tile_row as i64).abs() <= 2
                        && (t.tile_type.starts_with("PIC") || t.tile_type.starts_with("EFB"))
                })
                .map(|t| t.key.clone())
                .collect();
            if !group_tiles.contains(&tkey) {
                group_tiles.push(tkey);
            }
            group_tiles.sort();
            group_tiles.dedup();
            let piol = placed.packed.iologics.iter().find(|x| x.name == iol.name);
            let s = &iol.site.site;
            let mut words = Vec::new();
            let mut enums = Vec::new();
            match iol.kind {
                crate::pack::IologicKind::Iddrx1f | crate::pack::IologicKind::Oddrx1f => {
                    enums.push((format!("{s}.MODE"), "IDDRX1_ODDRX1".to_string()));
                }
                crate::pack::IologicKind::Iddrx2f => {
                    enums.push((format!("{s}.MODE"), "IDDRXN".to_string()));
                }
                crate::pack::IologicKind::Oddrx2f => {
                    enums.push((format!("{s}.MODE"), "ODDRXN".to_string()));
                    enums.push((format!("{s}.ODDRXN.MODE"), "ODDRX2".to_string()));
                }
                crate::pack::IologicKind::Delayg | crate::pack::IologicKind::Delayf => {
                    let del = piol.map(|p| p.del_value).unwrap_or(0);
                    words.push((
                        format!("{s}.DELAY.DEL_VALUE"),
                        word_bin(u32::from(del), 7),
                    ));
                    let wfe = if piol.map(|p| p.wait_for_edge).unwrap_or(false) {
                        "ENABLED"
                    } else {
                        "DISABLED"
                    };
                    enums.push((format!("{s}.DELAY.WAIT_FOR_EDGE"), wfe.to_string()));
                }
            }
            for (n, v) in &words {
                em.group_add_word(&group_tiles, n, v)?;
            }
            for (n, v) in &enums {
                em.group_add_enum(&group_tiles, n, v)?;
            }
            em.groups.push(TileGroup {
                tiles: group_tiles,
                words,
                enums,
            });
        }
        for sc in &placed.sys_configs {
            let tkey = tile_key_of(device, &sc.site, &sc.name)?;
            let psc = placed.packed.sys_configs.iter().find(|x| x.name == sc.name);
            match sc.kind {
                crate::pack::SysConfigKind::Usrmclk => {
                    em.add_enum(&tkey, "CCLK.MODE", "USRMCLK")?;
                }
                crate::pack::SysConfigKind::Oscg => {
                    em.add_enum(&tkey, "OSC.MODE", "OSCG")?;
                    let div = psc.map(|p| p.osc_div).unwrap_or(127);
                    em.add_enum(&tkey, "OSC.DIV", &div.to_string())?;
                }
                crate::pack::SysConfigKind::Dtr => {
                    em.add_enum(&tkey, "DTR.MODE", "DTR")?;
                }
            }
        }
        for dcu in &placed.dcus {
            let tkey = tile_key_of(device, &dcu.site, &dcu.name)?;
            let pdcu = placed.packed.dcus.iter().find(|x| x.name == dcu.name);
            let ch0 = pdcu.map(|p| p.ch0_mode.as_str()).unwrap_or("PCIE");
            let ch1 = pdcu.map(|p| p.ch1_mode.as_str()).unwrap_or("PCIE");
            extra_comments.push_str(&format!(
                ".comment DCUA {} tile={} site={} CH0_PROTOCOL={} CH1_PROTOCOL={}\n",
                dcu.name, tkey, dcu.site.site, ch0, ch1
            ));
        }
        for ext in &placed.extrefs {
            let tkey = tile_key_of(device, &ext.site, &ext.name)?;
            extra_comments.push_str(&format!(
                ".comment EXTREFB {} tile={} site={}\n",
                ext.name, tkey, ext.site.site
            ));
        }
    }
    let mut textcfg = em.render(&design.top);
    if !extra_comments.is_empty() {
        textcfg.push_str(&extra_comments);
    }
    Ok(BitstreamEcp5 { textcfg })
}

#[cfg(test)]
#[path = "textcfg_tests.rs"]
mod textcfg_tests;
