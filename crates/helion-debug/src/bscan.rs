//! JTAG BSCAN (`BSCANE2`) Debug Hub Multiplexing (`xsdbm` / UG908 parity).
//!
//! Routes IEEE 1149.1 `USER1`..`USER4` DR scans across up to 16 attached debug
//! cores (ILA, VIO, JTAG-to-AXI4-Lite Master, System Monitor bridge) using
//! 64-bit hub-framed DR words.

use helion_hw::{IR_USR1, Tap};

/// Maximum number of debug cores attached to a single BSCAN Debug Hub.
pub const MAX_BSCAN_SLOTS: usize = 16;

/// 4-bit framing nibble in bits `[63:60]` of a BSCAN hub DR word.
pub const BSCAN_HUB_MAGIC: u8 = 0xA;

/// Broadcast / hub-control slot index (`0xF`).
pub const BSCAN_HUB_BROADCAST_SLOT: u8 = 0xF;

/// Hub opcodes encoded in bits `[55:48]` of [`BscanFrame`].
pub mod hub_opcode {
    pub const NOP: u8 = 0x00;
    pub const SELECT_CORE: u8 = 0x01;
    pub const READ_STATUS: u8 = 0x02;
    pub const WRITE_CTRL: u8 = 0x03;
    pub const PROBE_WRITE: u8 = 0x04;
    pub const PROBE_READ: u8 = 0x05;
    pub const AXI_TXN: u8 = 0x06;
    pub const ARM_TRIGGER: u8 = 0x07;
    pub const UPLOAD_BRAM: u8 = 0x08;
    pub const DISCOVER: u8 = 0x09;
}

/// JTAG `BSCANE2` user instruction chain (`USER1`..`USER4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BscanUserChain {
    User1 = 1,
    User2 = 2,
    User3 = 3,
    User4 = 4,
}

impl BscanUserChain {
    pub fn from_index(idx: u8) -> Result<Self, String> {
        match idx {
            1 => Ok(Self::User1),
            2 => Ok(Self::User2),
            3 => Ok(Self::User3),
            4 => Ok(Self::User4),
            other => Err(format!(
                "bscan: invalid JTAG_CHAIN {other} (expected 1..=4)"
            )),
        }
    }

    pub fn index(self) -> u8 {
        self as u8
    }

    /// Corresponding 6-bit Helion TAP IR opcode (`USER1` = `0b000010`, etc.).
    pub fn tap_ir(self) -> u8 {
        match self {
            Self::User1 => IR_USR1,
            Self::User2 => 0b000011 ^ 0b100000, // distinct user2 opcode
            Self::User3 => 0b100010,
            Self::User4 => 0b100011,
        }
    }
}

/// Type of debug core attached to a [`BscanDebugHub`] port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugCoreKind {
    Ila,
    Vio,
    JtagAxi,
    SysmonBridge,
}

impl DebugCoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ila => "ila",
            Self::Vio => "vio",
            Self::JtagAxi => "jtag_axi",
            Self::SysmonBridge => "sysmon_bridge",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ila" | "h_ila" | "labtools_ila" => Ok(Self::Ila),
            "vio" | "h_vio" | "labtools_vio" => Ok(Self::Vio),
            "jtag_axi" | "axi" | "h_jtag_axi" | "labtools_jtag_axi" => Ok(Self::JtagAxi),
            "sysmon" | "sysmon_bridge" | "xadc" => Ok(Self::SysmonBridge),
            other => Err(format!("bscan: unknown debug core kind '{other}'")),
        }
    }

    pub fn type_code(self) -> u8 {
        match self {
            Self::Ila => 1,
            Self::Vio => 2,
            Self::JtagAxi => 3,
            Self::SysmonBridge => 4,
        }
    }
}

/// 64-bit framed JTAG DR word multiplexed by the BSCAN Debug Hub.
///
/// Layout:
/// - `[63:60]`: 4-bit `magic` (`0xA`)
/// - `[59:56]`: 4-bit `slot_id` (`0..15`, or `0xF` hub control)
/// - `[55:48]`: 8-bit `opcode`
/// - `[47:0]`:  48-bit `payload`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BscanFrame {
    pub magic: u8,
    pub slot_id: u8,
    pub opcode: u8,
    pub payload: u64,
}

impl BscanFrame {
    pub const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

    pub fn new(slot_id: u8, opcode: u8, payload: u64) -> Self {
        Self {
            magic: BSCAN_HUB_MAGIC,
            slot_id: slot_id & 0x0F,
            opcode,
            payload: payload & Self::PAYLOAD_MASK,
        }
    }

    pub fn encode(self) -> u64 {
        ((u64::from(self.magic & 0x0F)) << 60)
            | ((u64::from(self.slot_id & 0x0F)) << 56)
            | ((u64::from(self.opcode)) << 48)
            | (self.payload & Self::PAYLOAD_MASK)
    }

    pub fn decode(word: u64) -> Result<Self, String> {
        let magic = ((word >> 60) & 0x0F) as u8;
        if magic != BSCAN_HUB_MAGIC {
            return Err(format!(
                "bscan: invalid frame magic {:#x} in word {:#018x} (expected {:#x})",
                magic, word, BSCAN_HUB_MAGIC
            ));
        }
        Ok(Self {
            magic,
            slot_id: ((word >> 56) & 0x0F) as u8,
            opcode: ((word >> 48) & 0xFF) as u8,
            payload: word & Self::PAYLOAD_MASK,
        })
    }
}

/// Metadata and state for one slot on the [`BscanDebugHub`].
#[derive(Clone, Debug)]
pub struct BscanCoreSlot {
    pub slot_id: u8,
    pub core_name: String,
    pub kind: DebugCoreKind,
    pub uuid: u128,
    pub probe_count: usize,
    pub ctrl_reg: u64,
    pub status_reg: u64,
    pub last_payload: u64,
    pub rx_count: usize,
}

impl BscanCoreSlot {
    pub fn uuid_hex(&self) -> String {
        format!("{:032X}", self.uuid)
    }
}

/// Deterministic 128-bit UUID from hub name, slot index, and core name.
pub fn deterministic_core_uuid(hub_name: &str, slot_id: u8, core_name: &str) -> u128 {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x8422_2325_cbf2_9ce4 ^ u64::from(slot_id);
    for b in hub_name.bytes().chain([b':']).chain(core_name.bytes()) {
        h1 ^= u64::from(b);
        h1 = h1.wrapping_mul(0x0000_0100_0000_01b3);
        h2 ^= u64::from(b.rotate_left(3));
        h2 = h2.wrapping_mul(0x0000_0100_0000_01b3);
    }
    ((h1 as u128) << 64) | (h2 as u128)
}

/// JTAG BSCAN Debug Hub (`xsdbm_dbg_hub`) multiplexing up to 15 user debug cores
/// (`slot_id` 0..14; `0xF` reserved for hub broadcast/discovery).
#[derive(Clone, Debug)]
pub struct BscanDebugHub {
    pub hub_name: String,
    pub chain: BscanUserChain,
    pub active_slot: Option<u8>,
    pub slots: Vec<BscanCoreSlot>,
    pub total_dr_scans: usize,
}

impl BscanDebugHub {
    pub fn new(hub_name: impl Into<String>, chain: BscanUserChain) -> Self {
        Self {
            hub_name: hub_name.into(),
            chain,
            active_slot: None,
            slots: Vec::new(),
            total_dr_scans: 0,
        }
    }

    /// Attach a debug core to the next free BSCAN hub slot (`0..15`).
    pub fn register_core(
        &mut self,
        core_name: impl Into<String>,
        kind: DebugCoreKind,
        probe_count: usize,
    ) -> Result<u8, String> {
        let core_name = core_name.into();
        if let Some(existing) = self.slots.iter().find(|s| s.core_name == core_name) {
            return Ok(existing.slot_id);
        }
        if self.slots.len() >= MAX_BSCAN_SLOTS - 1 {
            return Err(format!(
                "bscan: hub {} full ({} slots occupied)",
                self.hub_name,
                self.slots.len()
            ));
        }
        let slot_id = self.slots.len() as u8;
        let uuid = deterministic_core_uuid(&self.hub_name, slot_id, &core_name);
        let status_reg =
            ((u64::from(kind.type_code())) << 40) | ((probe_count as u64 & 0xFFFF) << 16) | 1;
        self.slots.push(BscanCoreSlot {
            slot_id,
            core_name,
            kind,
            uuid,
            probe_count,
            ctrl_reg: 0,
            status_reg,
            last_payload: 0,
            rx_count: 0,
        });
        if self.active_slot.is_none() {
            self.active_slot = Some(slot_id);
        }
        Ok(slot_id)
    }

    pub fn slot(&self, slot_id: u8) -> Option<&BscanCoreSlot> {
        self.slots.iter().find(|s| s.slot_id == slot_id)
    }

    pub fn slot_by_name(&self, name: &str) -> Option<&BscanCoreSlot> {
        self.slots.iter().find(|s| s.core_name == name)
    }

    pub fn slot_mut(&mut self, slot_id: u8) -> Option<&mut BscanCoreSlot> {
        self.slots.iter_mut().find(|s| s.slot_id == slot_id)
    }

    /// Select active debug core slot on the BSCAN hub.
    pub fn select_slot(&mut self, slot_id: u8) -> Result<BscanFrame, String> {
        if !self.slots.iter().any(|s| s.slot_id == slot_id) {
            return Err(format!(
                "bscan: slot {slot_id} not registered on hub {}",
                self.hub_name
            ));
        }
        self.active_slot = Some(slot_id);
        self.transact(slot_id, hub_opcode::SELECT_CORE, u64::from(slot_id))
    }

    /// Execute a framed BSCAN hub transaction targeting `slot_id`.
    pub fn transact(
        &mut self,
        slot_id: u8,
        opcode: u8,
        payload: u64,
    ) -> Result<BscanFrame, String> {
        let req = BscanFrame::new(slot_id, opcode, payload);
        self.total_dr_scans += 1;

        if slot_id == BSCAN_HUB_BROADCAST_SLOT {
            // Hub-level discovery: returns number of attached cores and active chain.
            let resp_payload =
                ((self.slots.len() as u64) << 16) | ((u64::from(self.chain.index())) << 8) | 0x01;
            return Ok(BscanFrame::new(
                BSCAN_HUB_BROADCAST_SLOT,
                opcode,
                resp_payload,
            ));
        }

        let hub_name = self.hub_name.clone();
        self.active_slot = Some(slot_id);
        let slot = self.slot_mut(slot_id).ok_or_else(|| {
            format!("bscan: slot {slot_id} not registered on hub {hub_name}")
        })?;
        slot.rx_count += 1;
        slot.last_payload = req.payload;
        let resp_payload = match opcode {
            hub_opcode::SELECT_CORE => slot.status_reg,
            hub_opcode::READ_STATUS => slot.status_reg,
            hub_opcode::WRITE_CTRL => {
                slot.ctrl_reg = req.payload;
                slot.status_reg |= 0x02; // armed/configured bit
                slot.ctrl_reg
            }
            hub_opcode::PROBE_WRITE => {
                slot.ctrl_reg = req.payload;
                req.payload
            }
            hub_opcode::PROBE_READ => slot.ctrl_reg,
            hub_opcode::AXI_TXN => {
                slot.status_reg ^= 0x04;
                req.payload
            }
            hub_opcode::ARM_TRIGGER => {
                slot.status_reg |= 0x08;
                slot.status_reg
            }
            hub_opcode::UPLOAD_BRAM => slot.status_reg,
            hub_opcode::DISCOVER => (slot.uuid as u64) & BscanFrame::PAYLOAD_MASK,
            _ => req.payload,
        };
        Ok(BscanFrame::new(slot_id, opcode, resp_payload))
    }

    /// Execute a framed BSCAN hub transaction over a live [`Tap`] (`IR_USR1` DR scan).
    pub fn transact_via_tap(
        &mut self,
        tap: &mut Tap,
        slot_id: u8,
        opcode: u8,
        payload: u64,
    ) -> Result<BscanFrame, String> {
        let req = BscanFrame::new(slot_id, opcode, payload);
        tap.shift_ir(IR_USR1);
        // Shift framed request word through the TAP 64-bit DR register (bit63=0 so USR1 does not treat as ptr reset).
        let _tdo = tap.shift_dr_u64(req.encode() & !(1u64 << 63));
        self.transact(slot_id, opcode, payload)
    }

    /// Enumerate all cores attached to the BSCAN hub.
    pub fn discover_cores(&mut self) -> Vec<(u8, String, DebugCoreKind, String)> {
        let _ = self.transact(BSCAN_HUB_BROADCAST_SLOT, hub_opcode::DISCOVER, 0);
        self.slots
            .iter()
            .map(|s| (s.slot_id, s.core_name.clone(), s.kind, s.uuid_hex()))
            .collect()
    }
}
