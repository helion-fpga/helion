//! Virtual I/O (VIO) Core & JTAG-to-AXI4-Lite Master (`create_hw_axi_txn` — UG908 parity).
//!
//! Provides:
//! - [`VioCore`]: live virtual input stimulus injection (`PROBE_OUT*`) driving fabric BLEs/IOBs,
//!   and virtual output activity detectors (`PROBE_IN*`) with sticky rising/falling edge flags,
//!   min/max tracking, and transition counters multiplexed over [`BscanDebugHub`].
//! - [`JtagAxiMaster`] & [`create_hw_axi_txn`]: JTAG-to-AXI4-Lite read/write burst transactions
//!   with 32-bit address decoding, read-only protection, optional fabric BRAM aperture backing,
//!   and AXI4-Lite `OKAY` / `EXOKAY` / `SLVERR` / `DECERR` responses.

use crate::bscan::{BscanDebugHub, hub_opcode};
use crate::probe_site;
use helion_fabric::Fabric;
use helion_hw::Tap;
use helion_ir::Design;
use helion_place::Placed;
use std::collections::BTreeMap;

/// Sticky edge activity detected on a [`VioProbeIn`] channel since the last clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VioActivityKind {
    None,
    RisingEdge,
    FallingEdge,
    BothEdges,
}

impl VioActivityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::RisingEdge => "RISING",
            Self::FallingEdge => "FALLING",
            Self::BothEdges => "BOTH",
        }
    }
}

/// VIO input probe (`PROBE_IN*`) monitoring one or more design nets with activity detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VioProbeIn {
    pub name: String,
    pub nets: Vec<String>,
    pub width: usize,
    pub value: u64,
    pub prev_value: Option<u64>,
    pub min_value: Option<u64>,
    pub max_value: Option<u64>,
    /// Bitmask of bits that experienced a `0 -> 1` transition since last clear.
    pub rising_mask: u64,
    /// Bitmask of bits that experienced a `1 -> 0` transition since last clear.
    pub falling_mask: u64,
    /// Total number of value transitions observed since last clear.
    pub transition_count: u64,
}

impl VioProbeIn {
    pub fn new(name: impl Into<String>, nets: Vec<String>) -> Self {
        let width = nets.len().max(1).min(64);
        Self {
            name: name.into(),
            nets,
            width,
            value: 0,
            prev_value: None,
            min_value: None,
            max_value: None,
            rising_mask: 0,
            falling_mask: 0,
            transition_count: 0,
        }
    }

    pub fn mask(&self) -> u64 {
        if self.width >= 64 {
            u64::MAX
        } else {
            (1u64 << self.width) - 1
        }
    }

    /// Record a new sample word and update sticky edge activity detectors.
    pub fn observe(&mut self, raw_val: u64) -> VioActivityKind {
        let val = raw_val & self.mask();
        if let Some(prev) = self.prev_value {
            let risen = (!prev & val) & self.mask();
            let fallen = (prev & !val) & self.mask();
            self.rising_mask |= risen;
            self.falling_mask |= fallen;
            if val != prev {
                self.transition_count = self.transition_count.saturating_add(1);
            }
        }
        self.value = val;
        self.prev_value = Some(val);
        self.min_value = Some(self.min_value.map_or(val, |m| m.min(val)));
        self.max_value = Some(self.max_value.map_or(val, |m| m.max(val)));
        self.activity()
    }

    pub fn activity(&self) -> VioActivityKind {
        match (self.rising_mask != 0, self.falling_mask != 0) {
            (false, false) => VioActivityKind::None,
            (true, false) => VioActivityKind::RisingEdge,
            (false, true) => VioActivityKind::FallingEdge,
            (true, true) => VioActivityKind::BothEdges,
        }
    }

    pub fn clear_activity(&mut self) {
        self.rising_mask = 0;
        self.falling_mask = 0;
        self.transition_count = 0;
    }
}

/// VIO output probe (`PROBE_OUT*`) driving virtual stimulus into the design.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VioProbeOut {
    pub name: String,
    pub nets: Vec<String>,
    pub width: usize,
    pub initial_value: u64,
    pub staged_value: u64,
    pub committed_value: u64,
    pub dirty: bool,
    pub commit_count: u64,
}

impl VioProbeOut {
    pub fn new(name: impl Into<String>, nets: Vec<String>, initial_value: u64) -> Self {
        let width = nets.len().max(1).min(64);
        let mask = if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        let init = initial_value & mask;
        Self {
            name: name.into(),
            nets,
            width,
            initial_value: init,
            staged_value: init,
            committed_value: init,
            dirty: false,
            commit_count: 0,
        }
    }

    pub fn mask(&self) -> u64 {
        if self.width >= 64 {
            u64::MAX
        } else {
            (1u64 << self.width) - 1
        }
    }
}

/// Virtual Input/Output (VIO) runtime controller attached to a [`BscanDebugHub`] slot.
#[derive(Clone, Debug)]
pub struct VioCore {
    pub core_name: String,
    pub slot_id: u8,
    pub probes_in: Vec<VioProbeIn>,
    pub probes_out: Vec<VioProbeOut>,
}

impl VioCore {
    pub fn new(core_name: impl Into<String>, slot_id: u8) -> Self {
        Self {
            core_name: core_name.into(),
            slot_id,
            probes_in: Vec::new(),
            probes_out: Vec::new(),
        }
    }

    pub fn add_probe_in(&mut self, name: impl Into<String>, nets: Vec<String>) {
        self.probes_in.push(VioProbeIn::new(name, nets));
    }

    pub fn add_probe_out(
        &mut self,
        name: impl Into<String>,
        nets: Vec<String>,
        initial_value: u64,
    ) {
        self.probes_out
            .push(VioProbeOut::new(name, nets, initial_value));
    }

    pub fn probe_in(&self, name: &str) -> Option<&VioProbeIn> {
        self.probes_in
            .iter()
            .find(|p| p.name == name || p.nets.iter().any(|n| n == name))
    }

    pub fn probe_out(&self, name: &str) -> Option<&VioProbeOut> {
        self.probes_out
            .iter()
            .find(|p| p.name == name || p.nets.iter().any(|n| n == name))
    }

    /// Stage a new virtual stimulus value on `probe_out` `name`.
    pub fn set_probe_out(&mut self, name: &str, value: u64) -> Result<(), String> {
        let p = self
            .probes_out
            .iter_mut()
            .find(|p| p.name == name || p.nets.iter().any(|n| n == name))
            .ok_or_else(|| format!("vio {}: unknown probe_out '{name}'", self.core_name))?;
        p.staged_value = value & p.mask();
        p.dirty = true;
        Ok(())
    }

    /// Invert all bits of `probe_out` `name` and mark it dirty for commit.
    pub fn toggle_probe_out(&mut self, name: &str) -> Result<u64, String> {
        let p = self
            .probes_out
            .iter_mut()
            .find(|p| p.name == name || p.nets.iter().any(|n| n == name))
            .ok_or_else(|| format!("vio {}: unknown probe_out '{name}'", self.core_name))?;
        p.staged_value = (!p.staged_value) & p.mask();
        p.dirty = true;
        Ok(p.staged_value)
    }

    /// Commit all staged `probe_out` values over the [`BscanDebugHub`].
    /// Returns the number of `PROBE_WRITE` JTAG DR frames sent.
    pub fn commit_outputs(&mut self, hub: &mut BscanDebugHub) -> Result<usize, String> {
        let mut frames = 0usize;
        for (ch_idx, p) in self.probes_out.iter_mut().enumerate() {
            if p.dirty || p.commit_count == 0 {
                let payload = ((ch_idx as u64 & 0xFF) << 40) | (p.staged_value & 0x00FF_FFFF_FFFF);
                hub.transact(self.slot_id, hub_opcode::PROBE_WRITE, payload)?;
                p.committed_value = p.staged_value;
                p.dirty = false;
                p.commit_count += 1;
                frames += 1;
            }
        }
        Ok(frames)
    }

    /// Drive committed `probe_out` values directly onto the live [`Fabric`] BLEs / IOBs.
    pub fn apply_stimulus_to_fabric(
        &self,
        placed: &Placed,
        design: &Design,
        fab: &mut Fabric,
    ) -> Result<usize, String> {
        let mut driven_bits = 0usize;
        for p in &self.probes_out {
            for (bit_idx, net_name) in p.nets.iter().enumerate() {
                let bit_val = ((p.committed_value >> bit_idx) & 1) == 1;
                let (site, ble) = probe_site(placed, design, net_name)?;
                fab.set_ble_q(site.x, site.y, u32::from(ble), bit_val);
                driven_bits += 1;
            }
        }
        fab.refresh_comb();
        Ok(driven_bits)
    }

    /// Directly observe a `probe_in` channel value (and log a BSCAN `PROBE_READ`).
    pub fn sample_probe_in(
        &mut self,
        name: &str,
        value: u64,
        hub: &mut BscanDebugHub,
    ) -> Result<VioActivityKind, String> {
        let p = self
            .probes_in
            .iter_mut()
            .find(|p| p.name == name || p.nets.iter().any(|n| n == name))
            .ok_or_else(|| format!("vio {}: unknown probe_in '{name}'", self.core_name))?;
        let act = p.observe(value);
        hub.transact(self.slot_id, hub_opcode::PROBE_READ, p.value)?;
        Ok(act)
    }

    /// Sample all `probe_in` channels from the live [`Fabric`] and update activity detectors.
    pub fn sample_from_fabric(
        &mut self,
        placed: &Placed,
        design: &Design,
        fab: &Fabric,
        hub: &mut BscanDebugHub,
    ) -> Result<(), String> {
        for p in &mut self.probes_in {
            let mut word = 0u64;
            for (bit_idx, net_name) in p.nets.iter().enumerate().take(64) {
                let (site, ble) = probe_site(placed, design, net_name)?;
                if fab.ble_out(site.x, site.y, u32::from(ble)) {
                    word |= 1u64 << bit_idx;
                }
            }
            p.observe(word);
            hub.transact(self.slot_id, hub_opcode::PROBE_READ, p.value)?;
        }
        Ok(())
    }

    pub fn clear_all_activity(&mut self) {
        for p in &mut self.probes_in {
            p.clear_activity();
        }
    }
}

/// Direction of a JTAG-to-AXI4-Lite transaction (`create_hw_axi_txn -type read|write`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxiTxnKind {
    Read,
    Write,
}

impl AxiTxnKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" | "rd" | "r" => Ok(Self::Read),
            "write" | "wr" | "w" => Ok(Self::Write),
            other => Err(format!(
                "create_hw_axi_txn: unknown -type '{other}' (expected read|write)"
            )),
        }
    }
}

/// AXI4-Lite 2-bit response code (`RRESP` / `BRESP`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AxiResp {
    /// `2'b00` — Normal access success.
    Okay = 0b00,
    /// `2'b01` — Exclusive access okay.
    ExOkay = 0b01,
    /// `2'b10` — Slave error (e.g. write to read-only register).
    SlvErr = 0b10,
    /// `2'b11` — Decode error (no slave mapped at target address).
    DecErr = 0b11,
}

impl AxiResp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Okay => "OKAY",
            Self::ExOkay => "EXOKAY",
            Self::SlvErr => "SLVERR",
            Self::DecErr => "DECERR",
        }
    }
}

/// Hardware AXI transaction descriptor created by [`create_hw_axi_txn`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwAxiTxn {
    pub name: String,
    pub addr: u32,
    pub kind: AxiTxnKind,
    /// Burst length in 32-bit words (`1..=256`).
    pub len: usize,
    pub write_data: Vec<u32>,
    pub read_data: Vec<u32>,
    pub resp: Option<AxiResp>,
    pub completed: bool,
    pub dr_scans: usize,
}

/// Vivado Hardware Manager `create_hw_axi_txn` equivalent.
///
/// Validates 32-bit word alignment (`addr & 0x3 == 0`), burst length (`1..=256`),
/// and write payload length.
pub fn create_hw_axi_txn(
    name: impl Into<String>,
    addr: u32,
    kind: AxiTxnKind,
    len: usize,
    write_data: Vec<u32>,
) -> Result<HwAxiTxn, String> {
    let name = name.into();
    if name.trim().is_empty() {
        return Err("create_hw_axi_txn: transaction name cannot be empty".into());
    }
    if addr & 0x3 != 0 {
        return Err(format!(
            "create_hw_axi_txn: address {addr:#010x} is not 32-bit word aligned"
        ));
    }
    if len == 0 || len > 256 {
        return Err(format!(
            "create_hw_axi_txn: burst length {len} out of valid AXI range 1..=256"
        ));
    }
    if kind == AxiTxnKind::Write && write_data.len() != len {
        return Err(format!(
            "create_hw_axi_txn: write transaction '{name}' expects {len} words, got {}",
            write_data.len()
        ));
    }
    Ok(HwAxiTxn {
        name,
        addr,
        kind,
        len,
        write_data,
        read_data: Vec::new(),
        resp: None,
        completed: false,
        dr_scans: 0,
    })
}

/// Address region mapped on the [`JtagAxiMaster`] interconnect.
#[derive(Clone, Debug)]
pub struct AxiRegion {
    pub name: String,
    pub base_addr: u32,
    pub size_bytes: u32,
    pub read_only: bool,
    /// When `Some(major)`, 32-bit word accesses at offset `4 * i` also read/write
    /// word `i` of fabric BRAM `major`.
    pub bram_aperture_major: Option<u16>,
}

impl AxiRegion {
    pub fn contains(&self, addr: u32) -> bool {
        let end = self.base_addr.saturating_add(self.size_bytes);
        addr >= self.base_addr && addr < end
    }
}

/// JTAG-to-AXI4-Lite Master core (`h_jtag_axi`) attached to a [`BscanDebugHub`].
#[derive(Clone, Debug)]
pub struct JtagAxiMaster {
    pub core_name: String,
    pub slot_id: u8,
    pub regions: Vec<AxiRegion>,
    pub mem: BTreeMap<u32, u32>,
    pub txn_count: usize,
}

impl JtagAxiMaster {
    /// Construct a JTAG-to-AXI4-Lite master with standard Helion memory map apertures:
    /// - `0x4000_0000..0x4001_0000`: `axi_gpio_csr` (64 KiB R/W)
    /// - `0x44A0_0000..0x44A1_0000`: `axi_user_ip` (64 KiB R/W)
    /// - `0x8000_0000..0x8000_1000`: `axi_rom_id` (4 KiB Read-Only, `[0x8000_0000] = 0x4845_4C31`)
    /// - `0xC000_0000..0xC001_0000`: `axi_bram_ctrl` (64 KiB R/W, linkable to fabric BRAM)
    pub fn new(core_name: impl Into<String>, slot_id: u8) -> Self {
        let mut mem = BTreeMap::new();
        mem.insert(0x8000_0000, 0x4845_4C31); // "HEL1" magic ID
        mem.insert(0x8000_0004, 0x0003_0000); // v3.0
        Self {
            core_name: core_name.into(),
            slot_id,
            regions: vec![
                AxiRegion {
                    name: "axi_gpio_csr".to_string(),
                    base_addr: 0x4000_0000,
                    size_bytes: 0x0001_0000,
                    read_only: false,
                    bram_aperture_major: None,
                },
                AxiRegion {
                    name: "axi_user_ip".to_string(),
                    base_addr: 0x44A0_0000,
                    size_bytes: 0x0001_0000,
                    read_only: false,
                    bram_aperture_major: None,
                },
                AxiRegion {
                    name: "axi_rom_id".to_string(),
                    base_addr: 0x8000_0000,
                    size_bytes: 0x0000_1000,
                    read_only: true,
                    bram_aperture_major: None,
                },
                AxiRegion {
                    name: "axi_bram_ctrl".to_string(),
                    base_addr: 0xC000_0000,
                    size_bytes: 0x0001_0000,
                    read_only: false,
                    bram_aperture_major: Some(0),
                },
            ],
            mem,
            txn_count: 0,
        }
    }

    pub fn map_region(&mut self, region: AxiRegion) {
        self.regions.push(region);
    }

    fn find_region(&self, addr: u32) -> Option<&AxiRegion> {
        self.regions.iter().find(|r| r.contains(addr))
    }

    /// Execute `txn` through the [`BscanDebugHub`] (`run_hw_axi`).
    pub fn run_hw_axi(
        &mut self,
        txn: &mut HwAxiTxn,
        hub: &mut BscanDebugHub,
    ) -> Result<AxiResp, String> {
        self.run_hw_axi_inner(txn, hub, None)
    }

    /// Execute `txn` through the [`BscanDebugHub`] and a live [`Tap`] fabric (updating
    /// `IR_USR1` DR scans and any BRAM-backed AXI aperture).
    pub fn run_hw_axi_on_tap(
        &mut self,
        txn: &mut HwAxiTxn,
        hub: &mut BscanDebugHub,
        tap: &mut Tap,
    ) -> Result<AxiResp, String> {
        self.run_hw_axi_inner(txn, hub, Some(tap))
    }

    fn run_hw_axi_inner(
        &mut self,
        txn: &mut HwAxiTxn,
        hub: &mut BscanDebugHub,
        mut tap: Option<&mut Tap>,
    ) -> Result<AxiResp, String> {
        txn.read_data.clear();
        txn.dr_scans = 0;
        let mut overall_resp = AxiResp::Okay;

        for beat in 0..txn.len {
            let beat_addr = txn.addr.wrapping_add((beat as u32) * 4);
            // 1. Address command phase DR scan on BSCAN hub.
            let cmd_payload = ((u64::from(txn.kind == AxiTxnKind::Write)) << 47)
                | (u64::from(beat_addr) << 8)
                | (beat as u64 & 0xFF);
            if let Some(t) = tap.as_deref_mut() {
                hub.transact_via_tap(t, self.slot_id, hub_opcode::AXI_TXN, cmd_payload)?;
            } else {
                hub.transact(self.slot_id, hub_opcode::AXI_TXN, cmd_payload)?;
            }
            txn.dr_scans += 1;

            let Some(region) = self.find_region(beat_addr).cloned() else {
                overall_resp = AxiResp::DecErr;
                if txn.kind == AxiTxnKind::Read {
                    txn.read_data.push(0xDEAD_BEEF);
                }
                break;
            };

            match txn.kind {
                AxiTxnKind::Write => {
                    let word = txn.write_data[beat];
                    // 2. Write data phase DR scan on BSCAN hub.
                    if let Some(t) = tap.as_deref_mut() {
                        hub.transact_via_tap(
                            t,
                            self.slot_id,
                            hub_opcode::AXI_TXN,
                            u64::from(word),
                        )?;
                    } else {
                        hub.transact(self.slot_id, hub_opcode::AXI_TXN, u64::from(word))?;
                    }
                    txn.dr_scans += 1;

                    if region.read_only {
                        overall_resp = AxiResp::SlvErr;
                        break;
                    }
                    self.mem.insert(beat_addr, word);
                    if let (Some(major), Some(t)) = (region.bram_aperture_major, tap.as_deref_mut())
                    {
                        let word_idx = ((beat_addr - region.base_addr) / 4) as usize;
                        if word_idx < 255 {
                            t.fabric_mut()
                                .bram_write_word(major, word_idx, u64::from(word));
                        }
                    }
                }
                AxiTxnKind::Read => {
                    let mut val = self.mem.get(&beat_addr).copied().unwrap_or(0);
                    if let (Some(major), Some(t)) = (region.bram_aperture_major, tap.as_deref_mut())
                    {
                        let word_idx = ((beat_addr - region.base_addr) / 4) as usize;
                        if word_idx < 255 && !self.mem.contains_key(&beat_addr) {
                            val = t.fabric().bram_read_word(major, word_idx) as u32;
                        }
                    }
                    // 2. Read data phase DR scan on BSCAN hub.
                    if let Some(t) = tap.as_deref_mut() {
                        hub.transact_via_tap(
                            t,
                            self.slot_id,
                            hub_opcode::AXI_TXN,
                            u64::from(val),
                        )?;
                    } else {
                        hub.transact(self.slot_id, hub_opcode::AXI_TXN, u64::from(val))?;
                    }
                    txn.dr_scans += 1;
                    txn.read_data.push(val);
                }
            }
        }

        self.txn_count += 1;
        txn.resp = Some(overall_resp);
        txn.completed = true;
        Ok(overall_resp)
    }
}
