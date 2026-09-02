//! Helion architecture database (HAD) and FeatureMap packer.
//! CAD queries this crate instead of hardcoding a part name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Device {
    pub part: String,
    pub sku: String,
    pub part_id: u32,
    pub idcode: u32,
    pub had_version: u32,
    pub interior_cols: u32,
    pub interior_rows: u32,
    pub n_ble: u32,
    pub clb_x0: u32,
    pub clb_y0: u32,
    pub w: u32,
    pub clb_minors: u32,
    pub frame_bits: u32,
    pub n_dsp: u32,
    pub dsp_x: u32,
    pub dsp_y0: u32,
    pub n_bram: u32,
    pub bram_x: u32,
    pub bram_y0: u32,
    featuremap: FeatureMap,
}

#[derive(Clone, Debug)]
pub struct FeatureMap {
    /// relative feature (e.g. `BLE0.LUT.INIT[0]`) -> absolute bit in the CLB major
    bits: BTreeMap<String, u32>,
    pub n_minors: u32,
    pub frame_bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Far {
    pub block_type: u8,
    pub die: u8,
    pub major: u16,
    pub minor: u8,
}

impl Far {
    pub const CLB_IO_CLK: u8 = 0;
    pub const DSP: u8 = 2;
    pub const BRAM: u8 = 3;
    pub const IOB: u8 = 5;

    pub fn encode(self) -> u32 {
        ((self.block_type as u32) << 28)
            | ((self.die as u32) << 24)
            | ((self.major as u32) << 8)
            | (self.minor as u32)
    }

    /// Inverse of [`Far::encode`] (`.hbits` decode / readback).
    pub fn decode(word: u32) -> Self {
        Self {
            block_type: (word >> 28) as u8,
            die: ((word >> 24) & 0xF) as u8,
            major: ((word >> 8) & 0xFFFF) as u16,
            minor: (word & 0xFF) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitLoc {
    pub far: Far,
    pub bit: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Site {
    pub x: u32,
    pub y: u32,
    pub kind: SiteKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SiteKind {
    Clb,
    Iob,
    Clk,
    Dsp,
    Bram,
}

impl Device {
    pub fn workspace_root() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.pop();
        p
    }

    /// Compile-time HAD tree (`$workspace/devices/helion`). Last-resort fallback
    /// so `cargo test` works; packaged binaries must not rely on this alone.
    pub fn compile_time_devices_dir() -> PathBuf {
        Self::workspace_root().join("devices/helion")
    }

    /// Candidate HAD directories, in search order:
    /// `HELION_HAD`, next to the executable, `Helion.app/Contents/Resources`,
    /// cwd, then the compile-time workspace.
    pub fn had_search_paths_from(
        had_env: Option<&Path>,
        exe: Option<&Path>,
        cwd: Option<&Path>,
        compile_time: &Path,
    ) -> Vec<PathBuf> {
        let mut v = Vec::new();
        let mut push = |p: PathBuf| {
            if !v.iter().any(|q| q == &p) {
                v.push(p);
            }
        };
        if let Some(p) = had_env {
            push(p.to_path_buf());
            push(p.join("devices/helion"));
            push(p.join("helion"));
        }
        if let Some(exe) = exe {
            if let Some(dir) = exe.parent() {
                push(dir.join("devices/helion"));
                // Helion.app/Contents/MacOS/<bin> -> Contents/Resources/devices/helion
                push(dir.join("../Resources/devices/helion"));
                // target/{debug,release}/helion -> repo devices/helion
                push(dir.join("../../devices/helion"));
                // target/<triple>/{debug,release}/helion
                push(dir.join("../../../devices/helion"));
            }
        }
        if let Some(cwd) = cwd {
            push(cwd.join("devices/helion"));
        }
        push(compile_time.to_path_buf());
        v
    }

    pub fn had_search_paths() -> Vec<PathBuf> {
        Self::had_search_paths_from(
            std::env::var_os("HELION_HAD")
                .or_else(|| std::env::var_os("HELION_HAD_DIR"))
                .or_else(|| std::env::var_os("HELION_DEVICES"))
                .map(PathBuf::from)
                .as_deref(),
            std::env::current_exe().ok().as_deref(),
            std::env::current_dir().ok().as_deref(),
            &Self::compile_time_devices_dir(),
        )
    }

    /// First candidate that contains a `parts/` directory.
    pub fn resolve_devices_dir(
        had_env: Option<&Path>,
        exe: Option<&Path>,
        cwd: Option<&Path>,
        compile_time: &Path,
    ) -> PathBuf {
        Self::had_search_paths_from(had_env, exe, cwd, compile_time)
            .into_iter()
            .find(|p| p.join("parts").is_dir())
            .unwrap_or_else(|| compile_time.to_path_buf())
    }

    /// Runtime HAD root. `HELION_HAD` wins; then exe-relative (Mac .app
    /// Resources included); then cwd; then `CARGO_MANIFEST_DIR`.
    pub fn devices_dir() -> PathBuf {
        Self::resolve_devices_dir(
            std::env::var_os("HELION_HAD")
                .or_else(|| std::env::var_os("HELION_HAD_DIR"))
                .or_else(|| std::env::var_os("HELION_DEVICES"))
                .map(PathBuf::from)
                .as_deref(),
            std::env::current_exe().ok().as_deref(),
            std::env::current_dir().ok().as_deref(),
            &Self::compile_time_devices_dir(),
        )
    }

    /// RTL examples shipped next to HAD (`$root/examples` in the repo,
    /// `Contents/Resources/examples` in Helion.app).
    pub fn examples_dir() -> PathBuf {
        Self::devices_dir()
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("examples"))
            .unwrap_or_else(|| Self::workspace_root().join("examples"))
    }

    /// Load a part by name (`HL10T-C32-1`) from HAD TOML. This is the shipped API.
    pub fn load_part(name: &str) -> Result<Self, String> {
        let path = Self::devices_dir().join("parts").join(format!("{name}.toml"));
        Self::load_part_path(&path)
    }

    pub fn load_part_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut part = String::new();
        let mut sku = String::new();
        let mut part_id = 0u32;
        let mut idcode = 0u32;
        let mut had_version = 1u32;
        let mut interior_cols = 0u32;
        let mut interior_rows = 0u32;
        let mut n_ble = 8u32;
        let mut clb_x0 = 2u32;
        let mut clb_y0 = 1u32;
        let mut w = 80u32;
        let mut n_dsp = 0u32;
        let mut dsp_x = 8u32;
        let mut dsp_y0 = 1u32;
        let mut n_bram = 0u32;
        let mut bram_x = 10u32;
        let mut bram_y0 = 1u32;
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            let v = v.trim().trim_matches('"');
            match k {
                "part" => part = v.to_string(),
                "sku" => sku = v.to_string(),
                "part_id" => part_id = parse_u32(v)?,
                "idcode" => idcode = parse_u32(v)?,
                "had_version" => had_version = parse_u32(v)?,
                "interior_cols" => interior_cols = parse_u32(v)?,
                "interior_rows" => interior_rows = parse_u32(v)?,
                "n_ble" => n_ble = parse_u32(v)?,
                "clb_x0" => clb_x0 = parse_u32(v)?,
                "clb_y0" => clb_y0 = parse_u32(v)?,
                "w" => w = parse_u32(v)?,
                "mac27" => n_dsp = parse_u32(v)?,
                "dsp_x" => dsp_x = parse_u32(v)?,
                "dsp_y0" => dsp_y0 = parse_u32(v)?,
                "bram18" => n_bram = parse_u32(v)?,
                "bram_x" => bram_x = parse_u32(v)?,
                "bram_y0" => bram_y0 = parse_u32(v)?,
                _ => {}
            }
        }
        if part.is_empty() {
            return Err("missing part".into());
        }
        if idcode == 0 {
            idcode = idcode_from_part_id(0, part_id);
        }
        let featuremap = FeatureMap::pack_clb(16, 128);
        Ok(Self {
            part,
            sku,
            part_id,
            idcode,
            had_version,
            interior_cols,
            interior_rows,
            n_ble,
            clb_x0,
            clb_y0,
            w,
            clb_minors: 16,
            frame_bits: 128,
            n_dsp,
            dsp_x,
            dsp_y0,
            n_bram,
            bram_x,
            bram_y0,
            featuremap,
        })
    }

    pub fn lut6_count(&self) -> u32 {
        self.interior_cols * self.interior_rows * self.n_ble
    }

    pub fn n_clb(&self) -> u32 {
        self.interior_cols * self.interior_rows
    }

    pub fn clb_sites(&self) -> impl Iterator<Item = Site> + '_ {
        let x0 = self.clb_x0;
        let y0 = self.clb_y0;
        let cols = self.interior_cols;
        let rows = self.interior_rows;
        (0..rows).flat_map(move |dy| {
            (0..cols).map(move |dx| Site {
                x: x0 + dx,
                y: y0 + dy,
                kind: SiteKind::Clb,
            })
        })
    }

    pub fn dsp_sites(&self) -> impl Iterator<Item = Site> + '_ {
        let n = self.n_dsp;
        let x = self.dsp_x;
        let y0 = self.dsp_y0;
        (0..n).map(move |i| Site {
            x,
            y: y0 + i * 2,
            kind: SiteKind::Dsp,
        })
    }

    pub fn bram_sites(&self) -> impl Iterator<Item = Site> + '_ {
        let n = self.n_bram;
        let x = self.bram_x;
        let y0 = self.bram_y0;
        (0..n).map(move |i| Site {
            x,
            y: y0 + i * 2,
            kind: SiteKind::Bram,
        })
    }

    pub fn iob_sites(&self) -> impl Iterator<Item = Site> + '_ {
        // Bottom IO under each CLB column: (clb_x, clb_y0-1)
        let x0 = self.clb_x0;
        let cols = self.interior_cols;
        let y = self.clb_y0.saturating_sub(1);
        (0..cols).map(move |dx| Site {
            x: x0 + dx,
            y,
            kind: SiteKind::Iob,
        })
    }

    pub fn clb_major(&self, x: u32, y: u32) -> Option<u16> {
        if x < self.clb_x0 || y < self.clb_y0 {
            return None;
        }
        let dx = x - self.clb_x0;
        let dy = y - self.clb_y0;
        if dx >= self.interior_cols || dy >= self.interior_rows {
            return None;
        }
        Some((dy * self.interior_cols + dx) as u16)
    }

    pub fn iob_major(&self, x: u32, y: u32) -> Option<u16> {
        if y != self.clb_y0.saturating_sub(1) {
            return None;
        }
        if x < self.clb_x0 {
            return None;
        }
        let dx = x - self.clb_x0;
        if dx >= self.interior_cols {
            return None;
        }
        Some(dx as u16)
    }

    /// HAD I/O bank: 8 consecutive IOB sites (Fig. 53 / UG893 I/O Planning).
    pub const IOB_BANK_PINS: u32 = 8;
    /// Default I/O standard for a 1.8 V HAD bank (gold STA pad delay).
    pub const DEFAULT_IOSTANDARD: &'static str = "LVCMOS18";
    /// Default LVCMOS drive (mA). Unset / 12 keeps gold STA pad delay.
    pub const DEFAULT_DRIVE_MA: u32 = 12;
    /// Default slew. Unset / SLOW keeps gold STA pad delay.
    pub const DEFAULT_SLEW: &'static str = "SLOW";
    /// Default pull. Unset / NONE keeps gold STA pad delay.
    pub const DEFAULT_PULLTYPE: &'static str = "NONE";
    /// Default differential termination. Unset / FALSE keeps gold STA pad delay.
    pub const DEFAULT_DIFF_TERM: &'static str = "FALSE";
    /// Default input termination. Unset / NONE keeps gold STA pad delay.
    pub const DEFAULT_IN_TERM: &'static str = "NONE";

    pub fn iob_bank(&self, x: u32, y: u32) -> Option<u32> {
        self.iob_major(x, y)
            .map(|m| m as u32 / Self::IOB_BANK_PINS)
    }

    /// Bank VCCO in millivolts for a Helion I/O standard, or `None` if not in HAD.
    pub fn iostandard_vcco_mv(std: &str) -> Option<u32> {
        match std.trim().to_ascii_uppercase().as_str() {
            "LVCMOS12" => Some(1200),
            "LVCMOS15" | "SSTL15" | "SSTL15_I" => Some(1500),
            "LVCMOS18" | "HSTL_I" | "HSTL_I_18" => Some(1800),
            "LVCMOS25" => Some(2500),
            "LVCMOS33" => Some(3300),
            _ => None,
        }
    }

    pub fn legal_iostandard(std: &str) -> bool {
        Self::iostandard_vcco_mv(std).is_some()
    }

    /// HAD-legal drive strengths (mA).
    pub fn legal_drive(ma: u32) -> bool {
        matches!(ma, 2 | 4 | 6 | 8 | 12 | 16 | 24)
    }

    pub fn parse_drive(s: &str) -> Option<u32> {
        let t = s
            .trim()
            .trim_end_matches(|c: char| c.eq_ignore_ascii_case(&'m') || c.eq_ignore_ascii_case(&'a'))
            .trim();
        t.parse().ok().filter(|&ma| Self::legal_drive(ma))
    }

    pub fn legal_slew(s: &str) -> bool {
        matches!(s.trim().to_ascii_uppercase().as_str(), "SLOW" | "FAST")
    }

    pub fn legal_pulltype(s: &str) -> bool {
        matches!(
            s.trim().to_ascii_uppercase().as_str(),
            "NONE" | "PULLUP" | "PULLDOWN" | "KEEPER"
        )
    }

    /// UG893 I/O Ports `DIFF_TERM`: TRUE | FALSE (also 1 | 0).
    pub fn parse_diff_term(s: &str) -> Option<&'static str> {
        match s.trim().to_ascii_uppercase().as_str() {
            "TRUE" | "1" => Some("TRUE"),
            "FALSE" | "0" => Some("FALSE"),
            _ => None,
        }
    }

    pub fn legal_diff_term(s: &str) -> bool {
        Self::parse_diff_term(s).is_some()
    }

    /// UG893 I/O Ports `IN_TERM`: NONE | UNTUNED_SPLIT_{40,50,60}.
    pub fn parse_in_term(s: &str) -> Option<&'static str> {
        match s.trim().to_ascii_uppercase().as_str() {
            "NONE" => Some("NONE"),
            "UNTUNED_SPLIT_40" => Some("UNTUNED_SPLIT_40"),
            "UNTUNED_SPLIT_50" => Some("UNTUNED_SPLIT_50"),
            "UNTUNED_SPLIT_60" => Some("UNTUNED_SPLIT_60"),
            _ => None,
        }
    }

    pub fn legal_in_term(s: &str) -> bool {
        Self::parse_in_term(s).is_some()
    }

    /// SSTL/HSTL banks carry on-die / differential termination. LVCMOS does not.
    pub fn had_odt_iostandard(std: Option<&str>) -> bool {
        let std = std
            .map(|s| s.trim().to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| Self::DEFAULT_IOSTANDARD.to_string());
        matches!(
            std.as_str(),
            "SSTL15" | "SSTL15_I" | "HSTL_I" | "HSTL_I_18"
        )
    }

    /// `DIFF_TERM TRUE` is HAD-legal only on SSTL/HSTL; FALSE is always legal.
    pub fn diff_term_legal_for_iostandard(std: Option<&str>, term: &str) -> bool {
        match Self::parse_diff_term(term) {
            None => false,
            Some("FALSE") => true,
            Some("TRUE") => Self::had_odt_iostandard(std),
            _ => false,
        }
    }

    /// `IN_TERM` other than NONE is HAD-legal only on SSTL/HSTL.
    pub fn in_term_legal_for_iostandard(std: Option<&str>, term: &str) -> bool {
        match Self::parse_in_term(term) {
            None => false,
            Some("NONE") => true,
            Some(_) => Self::had_odt_iostandard(std),
        }
    }

    /// Drive vs IOSTANDARD (unset standard is the HAD default LVCMOS18).
    pub fn drive_legal_for_iostandard(std: Option<&str>, ma: u32) -> bool {
        if !Self::legal_drive(ma) {
            return false;
        }
        let std = std
            .map(|s| s.trim().to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| Self::DEFAULT_IOSTANDARD.to_string());
        match std.as_str() {
            "LVCMOS12" => matches!(ma, 2 | 4 | 6 | 8),
            "LVCMOS15" | "SSTL15" | "SSTL15_I" => matches!(ma, 2 | 4 | 6 | 8 | 12 | 16),
            "LVCMOS18" | "HSTL_I" | "HSTL_I_18" => matches!(ma, 2 | 4 | 6 | 8 | 12 | 16),
            "LVCMOS25" => matches!(ma, 4 | 8 | 12 | 16 | 24),
            "LVCMOS33" => matches!(ma, 4 | 8 | 12 | 16),
            _ => Self::legal_drive(ma),
        }
    }

    /// IOB word bits above USED/BLE/Y. Defaults encode as 0 so gold bitgen holds.
    /// [18:16] DRIVE, [19] SLEW, [21:20] PULLTYPE, [22] DIFF_TERM, [24:23] IN_TERM.
    pub fn iob_electrical_bits(
        drive: Option<&str>,
        slew: Option<&str>,
        pull: Option<&str>,
        diff_term: Option<&str>,
        in_term: Option<&str>,
    ) -> u128 {
        let mut w = 0u128;
        if let Some(ma) = drive.and_then(Self::parse_drive) {
            if ma != Self::DEFAULT_DRIVE_MA {
                let code: u128 = match ma {
                    2 => 1,
                    4 => 2,
                    6 => 3,
                    8 => 4,
                    16 => 6,
                    24 => 7,
                    _ => 0,
                };
                w |= code << 16;
            }
        }
        if slew
            .map(|s| s.trim().eq_ignore_ascii_case("FAST"))
            .unwrap_or(false)
        {
            w |= 1u128 << 19;
        }
        let pcode = match pull.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
            Some("PULLUP") => 1u128,
            Some("PULLDOWN") => 2,
            Some("KEEPER") => 3,
            _ => 0,
        };
        w |= pcode << 20;
        if diff_term
            .and_then(Self::parse_diff_term)
            .map(|s| s == "TRUE")
            .unwrap_or(false)
        {
            w |= 1u128 << 22;
        }
        let icode = match in_term.and_then(Self::parse_in_term) {
            Some("UNTUNED_SPLIT_40") => 1u128,
            Some("UNTUNED_SPLIT_50") => 2,
            Some("UNTUNED_SPLIT_60") => 3,
            _ => 0,
        };
        w |= icode << 23;
        w
    }

    pub fn featuremap(&self) -> &FeatureMap {
        &self.featuremap
    }

    /// HAD die/device report (queried by doctor / Tcl, not hardcoded in CAD).
    pub fn report_die(&self) -> String {
        format!(
            "die 0 part={} sku={} cols={} rows={} LUT6={} BRAM18={} DSP={} idcode={:#010x} sites_clb={} sites_iob={}",
            self.part,
            self.sku,
            self.interior_cols,
            self.interior_rows,
            self.lut6_count(),
            self.n_bram,
            self.n_dsp,
            self.idcode,
            self.clb_sites().count(),
            self.iob_sites().count()
        )
    }

    /// HAD FeatureMap text report: where the shipped packer puts each feature.
    /// Queried by `helion doctor` and the Tcl `report_featuremap`; the CAD never
    /// hardcodes these offsets.
    pub fn report_featuremap(&self) -> String {
        let fm = &self.featuremap;
        let mut s = format!(
            "featuremap part={} minors={} frame_bits={} features={} sites_clb={} sites_iob={} sites_bram={} sites_dsp={}\n",
            self.part,
            fm.n_minors,
            fm.frame_bits,
            fm.bits.len(),
            self.clb_sites().count(),
            self.iob_sites().count(),
            self.bram_sites().count(),
            self.dsp_sites().count()
        );
        for feature in [
            "BLE0.LUT.INIT[0]",
            "BLE0.LUT.INIT[63]",
            "BLE7.LUT.INIT[0]",
            "BLE0.LUT.FRACTURE",
            "BLE0.FF.USED",
            "BLE0.FF.INIT",
            "IMUX[0][0]",
            "IMUX[63][4]",
        ] {
            match fm.minor_bit(feature) {
                Some((minor, bit)) => {
                    s.push_str(&format!("  {feature} minor {minor} bit {bit}\n"));
                }
                None => s.push_str(&format!("  {feature} MISSING\n")),
            }
        }
        s
    }

    pub fn locate_clb(&self, x: u32, y: u32, feature: &str) -> Result<BitLoc, String> {
        let major = self
            .clb_major(x, y)
            .ok_or_else(|| format!("not a CLB site CLB_X{x}Y{y}"))?;
        let abs = self
            .featuremap
            .abs_bit(feature)
            .ok_or_else(|| format!("unknown feature {feature}"))?;
        let minor = (abs / self.frame_bits) as u8;
        let bit = (abs % self.frame_bits) as u8;
        Ok(BitLoc {
            far: Far {
                block_type: Far::CLB_IO_CLK,
                die: 0,
                major,
                minor,
            },
            bit,
        })
    }

    /// Parse `CLB_X2Y1.BLE0.LUT.INIT[0]` into a bit location via the shipped FeatureMap.
    pub fn locate(&self, full: &str) -> Result<BitLoc, String> {
        let (site, rest) = full
            .split_once('.')
            .ok_or_else(|| format!("bad feature {full}"))?;
        if let Some(xy) = site.strip_prefix("CLB_X") {
            let (xs, ys) = xy
                .split_once('Y')
                .ok_or_else(|| format!("bad CLB site {site}"))?;
            let x: u32 = xs.parse().map_err(|_| "bad x".to_string())?;
            let y: u32 = ys.parse().map_err(|_| "bad y".to_string())?;
            self.locate_clb(x, y, rest)
        } else {
            Err(format!("unsupported site {site}"))
        }
    }
}

impl FeatureMap {
    /// Normative CLB packer: all eight INIT first (512 bits), then per-BLE mode.
    /// `BLE0.LUT.FRACTURE` is absolute bit 512 = minor 4 bit 0.
    pub fn pack_clb(n_minors: u32, frame_bits: u32) -> Self {
        let mut bits = BTreeMap::new();
        let mut cursor = 0u32;
        // 1. All eight INIT, 64-aligned as a group.
        for n in 0..8u32 {
            cursor = align(cursor, 64);
            for i in 0..64u32 {
                bits.insert(format!("BLE{n}.LUT.INIT[{i}]"), cursor + i);
            }
            cursor += 64;
        }
        // 2. Per-BLE mode: FRACTURE, CARRY, DMUX[1:0], OQ_MUX  (5 bits)
        for n in 0..8u32 {
            bits.insert(format!("BLE{n}.LUT.FRACTURE"), cursor);
            bits.insert(format!("BLE{n}.LUT.CARRY"), cursor + 1);
            bits.insert(format!("BLE{n}.DMUX[0]"), cursor + 2);
            bits.insert(format!("BLE{n}.DMUX[1]"), cursor + 3);
            bits.insert(format!("BLE{n}.OQ_MUX"), cursor + 4);
            cursor += 5;
        }
        // 3. FF: USED, INIT, SRVAL, SYNC, CLKINV
        for n in 0..8u32 {
            bits.insert(format!("BLE{n}.FF.USED"), cursor);
            bits.insert(format!("BLE{n}.FF.INIT"), cursor + 1);
            bits.insert(format!("BLE{n}.FF.SRVAL"), cursor + 2);
            bits.insert(format!("BLE{n}.FF.SYNC"), cursor + 3);
            bits.insert(format!("BLE{n}.FF.CLKINV"), cursor + 4);
            cursor += 5;
        }
        // 4. IMUX 64×5, then pad to n_minors * frame_bits
        for m in 0..64u32 {
            for b in 0..5u32 {
                bits.insert(format!("IMUX[{m}][{b}]"), cursor + b);
            }
            cursor += 5;
        }
        let _ = cursor;
        Self {
            bits,
            n_minors,
            frame_bits,
        }
    }

    pub fn abs_bit(&self, feature: &str) -> Option<u32> {
        self.bits.get(feature).copied()
    }

    pub fn minor_bit(&self, feature: &str) -> Option<(u8, u8)> {
        let abs = self.abs_bit(feature)?;
        Some(((abs / self.frame_bits) as u8, (abs % self.frame_bits) as u8))
    }
}

pub fn idcode_from_part_id(had_version: u32, part_id: u32) -> u32 {
    let version = had_version & 0xF;
    (version << 28) | (part_id << 12) | 0xA1F
}

pub fn uwilton_sides(i: usize, w: usize) -> (usize, usize, usize) {
    let straight = i;
    let right = (i + 1) % w;
    let left = (i + w - 1) % w;
    (straight, right, left)
}

fn align(bit: u32, a: u32) -> u32 {
    if a == 0 {
        return bit;
    }
    let r = bit % a;
    if r == 0 { bit } else { bit + (a - r) }
}

fn parse_u32(v: &str) -> Result<u32, String> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else {
        v.parse().map_err(|e| format!("{v}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helion_t_had_loads() {
        let d = Device::load_part("HL10T-C32-1").expect("load Helion-T");
        assert_eq!(d.interior_cols, 32);
        assert_eq!(d.interior_rows, 32);
        assert_eq!(d.lut6_count(), 8192);
        assert_eq!(d.idcode, 0x0001_1A1F);
        assert_eq!(d.clb_sites().count(), 32 * 32);
        // CAD queries the database — sites come from HAD, not a hardcoded part match.
        let first = d.clb_sites().next().unwrap();
        assert_eq!(first.x, d.clb_x0);
        assert_eq!(first.y, d.clb_y0);
        assert_eq!(d.n_bram, 8);
        assert_eq!(d.bram_sites().count(), 8);
        assert_eq!(d.bram_sites().next().unwrap().kind, SiteKind::Bram);
    }

    #[test]
    fn runtime_had_path_is_not_only_cargo_manifest_dir() {
        let compile = Device::compile_time_devices_dir();
        assert!(
            compile.join("parts/HL10T-C32-1.toml").is_file(),
            "compile-time HAD must still exist for cargo test: {}",
            compile.display()
        );

        // HELION_HAD wins over compile-time, even when both have parts/.
        let tmp = std::env::temp_dir().join(format!(
            "helion-had-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("parts")).unwrap();
        std::fs::write(tmp.join("parts/.helion-had-marker"), b"runtime").unwrap();
        let got = Device::resolve_devices_dir(Some(&tmp), None, None, &compile);
        assert_eq!(
            got.canonicalize().unwrap(),
            tmp.canonicalize().unwrap(),
            "HELION_HAD must win over CARGO_MANIFEST_DIR"
        );
        let _ = std::fs::remove_dir_all(&tmp);

        // Mac .app Resources path is in the search list (layout, not existence).
        let exe = Path::new("/Applications/Helion.app/Contents/MacOS/helion-ide");
        let paths = Device::had_search_paths_from(None, Some(exe), None, &compile);
        assert!(
            paths.iter().any(|p| p.to_string_lossy().contains("Resources/devices/helion")),
            "search must include Helion.app Resources HAD: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p == &compile),
            "search must still include compile-time HAD: {paths:?}"
        );

        // Empty env falls back to a dir that actually has parts/.
        let fallback = Device::resolve_devices_dir(None, None, None, &compile);
        assert!(
            fallback.join("parts").is_dir(),
            "fallback HAD has no parts/: {}",
            fallback.display()
        );
        let ex = Device::examples_dir();
        assert!(
            ex.join("counter.sv").is_file(),
            "examples must be locatable next to HAD: {}",
            ex.display()
        );
    }

    #[test]
    fn idcode_formula_and_had_t() {
        // IEEE packing with version nibble 0: (part_id << 12) | 0xA1F
        assert_eq!(idcode_from_part_id(0, 1), 0x0000_1A1F);
        assert_eq!(idcode_from_part_id(0, 2), 0x0000_2A1F);
        // Helion-T HAD ships the documented 0.1 IDCODE 0x00011A1F
        let d = Device::load_part("HL10T-C32-1").unwrap();
        assert_eq!(d.idcode, 0x0001_1A1F);
    }

    #[test]
    fn featuremap_init0_and_fracture() {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        let init0 = d.locate("CLB_X2Y1.BLE0.LUT.INIT[0]").unwrap();
        assert_eq!(init0.far.minor, 0);
        assert_eq!(init0.bit, 0);
        assert_eq!(init0.far.major, 0);
        let frac = d.locate("CLB_X2Y1.BLE0.LUT.FRACTURE").unwrap();
        assert_eq!(frac.far.minor, 4);
        assert_eq!(frac.bit, 0);
    }

    #[test]
    fn iob_bank_and_iostandard_from_had() {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        assert_eq!(d.iob_bank(2, 0), Some(0));
        assert_eq!(d.iob_bank(9, 0), Some(0), "8 pins per bank: dx=7 still BANK0");
        assert_eq!(d.iob_bank(10, 0), Some(1));
        assert!(d.iob_bank(0, 0).is_none(), "x=0 is not a HAD IOB");
        assert!(Device::legal_iostandard("LVCMOS18"));
        assert!(Device::legal_iostandard("lvcmos33"));
        assert!(!Device::legal_iostandard("LVDS_25"));
        assert_eq!(Device::iostandard_vcco_mv("LVCMOS18"), Some(1800));
        assert_eq!(Device::iostandard_vcco_mv("LVCMOS33"), Some(3300));
        assert_eq!(Device::DEFAULT_IOSTANDARD, "LVCMOS18");
        assert!(Device::legal_drive(12));
        assert!(Device::legal_drive(4));
        assert!(!Device::legal_drive(5));
        assert_eq!(Device::parse_drive("12mA"), Some(12));
        assert!(Device::legal_slew("fast"));
        assert!(!Device::legal_slew("MEDIUM"));
        assert!(Device::legal_pulltype("PULLUP"));
        assert!(!Device::legal_pulltype("PULL"));
        assert!(Device::drive_legal_for_iostandard(None, 12));
        assert!(Device::drive_legal_for_iostandard(Some("LVCMOS18"), 16));
        assert!(!Device::drive_legal_for_iostandard(Some("LVCMOS18"), 24));
        assert!(Device::drive_legal_for_iostandard(Some("LVCMOS25"), 24));
        assert_eq!(Device::iob_electrical_bits(None, None, None, None, None), 0);
        assert_eq!(
            Device::iob_electrical_bits(
                Some("12"),
                Some("SLOW"),
                Some("NONE"),
                Some("FALSE"),
                Some("NONE")
            ),
            0,
            "defaults must not change the gold IOB word"
        );
        assert_ne!(Device::iob_electrical_bits(Some("4"), None, None, None, None), 0);
        assert_ne!(Device::iob_electrical_bits(None, Some("FAST"), None, None, None), 0);
        assert_ne!(Device::iob_electrical_bits(None, None, Some("PULLUP"), None, None), 0);
        assert_ne!(
            Device::iob_electrical_bits(None, None, None, Some("TRUE"), None),
            0
        );
        assert_ne!(
            Device::iob_electrical_bits(None, None, None, None, Some("UNTUNED_SPLIT_50")),
            0
        );
        assert_eq!(Device::DEFAULT_DRIVE_MA, 12);
        assert_eq!(Device::DEFAULT_SLEW, "SLOW");
        assert_eq!(Device::DEFAULT_PULLTYPE, "NONE");
        assert_eq!(Device::DEFAULT_DIFF_TERM, "FALSE");
        assert_eq!(Device::DEFAULT_IN_TERM, "NONE");
        assert!(Device::legal_diff_term("true"));
        assert!(Device::legal_diff_term("0"));
        assert!(!Device::legal_diff_term("YES"));
        assert_eq!(Device::parse_diff_term("1"), Some("TRUE"));
        assert!(Device::legal_in_term("UNTUNED_SPLIT_50"));
        assert!(!Device::legal_in_term("50"));
        assert!(Device::diff_term_legal_for_iostandard(None, "FALSE"));
        assert!(!Device::diff_term_legal_for_iostandard(None, "TRUE"));
        assert!(Device::diff_term_legal_for_iostandard(Some("SSTL15"), "TRUE"));
        assert!(Device::in_term_legal_for_iostandard(None, "NONE"));
        assert!(!Device::in_term_legal_for_iostandard(Some("LVCMOS18"), "UNTUNED_SPLIT_50"));
        assert!(Device::in_term_legal_for_iostandard(Some("HSTL_I"), "UNTUNED_SPLIT_50"));
    }

    #[test]
    fn report_die_is_from_had() {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        let r = d.report_die();
        assert!(r.contains("HL10T-C32-1"), "{r}");
        assert!(r.contains("cols=32"), "{r}");
        assert!(r.contains("LUT6=8192"), "{r}");
        assert!(r.contains("0x00011a1f") || r.contains("0x00011A1F"), "{r}");
        assert!(r.contains("BRAM18=8"), "{r}");
    }

    #[test]
    fn report_featuremap_is_from_the_packer() {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        let r = d.report_featuremap();
        assert!(r.contains("part=HL10T-C32-1"), "{r}");
        assert!(r.contains("minors=16"), "{r}");
        assert!(r.contains("frame_bits=128"), "{r}");
        assert!(r.contains("sites_clb=1024"), "{r}");
        assert!(r.contains("sites_bram=8"), "{r}");
        assert!(!r.contains("MISSING"), "every reported feature must exist: {r}");
        // The report must agree with locate(), not with a hardcoded string.
        for feature in ["BLE0.LUT.INIT[0]", "BLE0.LUT.FRACTURE", "IMUX[0][0]"] {
            let loc = d.locate(&format!("CLB_X2Y1.{feature}")).unwrap();
            assert!(
                r.contains(&format!("{feature} minor {} bit {}", loc.far.minor, loc.bit)),
                "report row for {feature} must match locate() minor {} bit {}: {r}",
                loc.far.minor,
                loc.bit
            );
        }
        // INIT[63] is the last bit of BLE0's 64-bit word.
        assert!(r.contains("BLE0.LUT.INIT[63] minor 0 bit 63"), "{r}");
        // BLE7 INIT starts a whole 64-bit group later.
        assert!(r.contains("BLE7.LUT.INIT[0] minor 3 bit 64"), "{r}");
    }

    #[test]
    fn far_encode_decode_round_trips() {
        for (block, major, minor) in [
            (Far::CLB_IO_CLK, 0u16, 0u8),
            (Far::CLB_IO_CLK, 1023, 15),
            (Far::IOB, 31, 0),
            (Far::BRAM, 7, 9),
            (Far::DSP, 3, 1),
        ] {
            let far = Far {
                block_type: block,
                die: 0,
                major,
                minor,
            };
            let back = Far::decode(far.encode());
            assert_eq!(back, far, "FAR {far:?} must survive encode/decode");
        }
        // Consecutive FARs inside one major differ by 1 (frame runs rely on it).
        let a = Far { block_type: Far::CLB_IO_CLK, die: 0, major: 5, minor: 3 };
        let b = Far { block_type: Far::CLB_IO_CLK, die: 0, major: 5, minor: 4 };
        assert_eq!(a.encode() + 1, b.encode());
    }

    #[test]
    fn uwilton_disjoint_at_zero() {
        for w in [8usize, 16, 80] {
            for i in 0..w {
                let (s, r, l) = uwilton_sides(i, w);
                let mut v = vec![s, r, l];
                v.sort_unstable();
                v.dedup();
                assert_eq!(v.len(), 3, "i={i} w={w}");
            }
        }
        let (s, r, l) = uwilton_sides(0, 8);
        assert_eq!((s, r, l), (0, 1, 7));
    }
}
