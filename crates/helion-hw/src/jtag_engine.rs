//! Native JTAG engine over FTDI MPSSE.
//!
//! [`JtagEngine`] drives an IEEE 1149.1 scan chain through any
//! [`MpssePort`] — the real USB FTDI ([`crate::UsbFtdiPort`], feature
//! `usb-native`) or the in-process [`crate::MockFtdi`] that interprets the
//! same MPSSE bytes and clocks simulated TAPs. It keeps a software mirror of
//! the 16-state TAP controller, emits shortest TMS paths, shifts arbitrary
//! IR/DR lengths through a multi-device chain (non-target devices in BYPASS),
//! scans the chain for IDCODEs and the total IR length, and sets TCK.
//!
//! Every shift is encoded by [`crate::mpsse::MpsseCmd`]; reads are decoded
//! from the returned bytes only — the engine never fills in TDO itself.

use crate::mpsse::{self, MpsseCmd, pack_bits, unpack_bits};
use crate::{IR_BYPASS, IR_CFG_W, IR_IDCODE, IR_STAT, Tap};
use helion_bits::Bitstream;
use helion_fabric::Stat;

/// USB-level FTDI channel the engine talks to.
pub trait MpssePort: Send {
    /// One-line description (`FT2232H bus 1 addr 4`, `mock FTDI · 2-device chain`).
    fn describe(&self) -> String;
    /// `SIO_RESET` (wValue 0).
    fn usb_reset(&mut self) -> Result<(), String>;
    /// `SIO_SET_LATENCY_TIMER` in ms.
    fn set_latency_timer(&mut self, ms: u8) -> Result<(), String>;
    /// `SIO_SET_BITMODE` (`mask | mode << 8`).
    fn set_bitmode(&mut self, mask: u8, mode: u8) -> Result<(), String>;
    /// Purge RX and TX buffers.
    fn purge(&mut self) -> Result<(), String>;
    /// Bulk-OUT MPSSE bytes.
    fn write(&mut self, data: &[u8]) -> Result<(), String>;
    /// Bulk-IN exactly `n` payload bytes (modem-status bytes stripped).
    fn read(&mut self, n: usize) -> Result<Vec<u8>, String>;
    /// The simulated board behind a mock port (`None` on a real cable).
    fn board(&mut self) -> Option<&mut Tap> {
        None
    }
    /// `true` for the in-process mock.
    fn is_mock(&self) -> bool {
        false
    }
}

/// IEEE 1149.1 TAP controller states (all 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JtagState {
    Reset,
    Idle,
    SelectDr,
    CaptureDr,
    ShiftDr,
    Exit1Dr,
    PauseDr,
    Exit2Dr,
    UpdateDr,
    SelectIr,
    CaptureIr,
    ShiftIr,
    Exit1Ir,
    PauseIr,
    Exit2Ir,
    UpdateIr,
}

impl JtagState {
    pub const ALL: [JtagState; 16] = [
        JtagState::Reset,
        JtagState::Idle,
        JtagState::SelectDr,
        JtagState::CaptureDr,
        JtagState::ShiftDr,
        JtagState::Exit1Dr,
        JtagState::PauseDr,
        JtagState::Exit2Dr,
        JtagState::UpdateDr,
        JtagState::SelectIr,
        JtagState::CaptureIr,
        JtagState::ShiftIr,
        JtagState::Exit1Ir,
        JtagState::PauseIr,
        JtagState::Exit2Ir,
        JtagState::UpdateIr,
    ];

    /// Next state on one TCK with `tms`.
    pub fn next(self, tms: bool) -> JtagState {
        use JtagState::*;
        match (self, tms) {
            (Reset, true) => Reset,
            (Reset, false) => Idle,
            (Idle, true) => SelectDr,
            (Idle, false) => Idle,
            (SelectDr, true) => SelectIr,
            (SelectDr, false) => CaptureDr,
            (CaptureDr, true) => Exit1Dr,
            (CaptureDr, false) => ShiftDr,
            (ShiftDr, true) => Exit1Dr,
            (ShiftDr, false) => ShiftDr,
            (Exit1Dr, true) => UpdateDr,
            (Exit1Dr, false) => PauseDr,
            (PauseDr, true) => Exit2Dr,
            (PauseDr, false) => PauseDr,
            (Exit2Dr, true) => UpdateDr,
            (Exit2Dr, false) => ShiftDr,
            (UpdateDr, true) => SelectDr,
            (UpdateDr, false) => Idle,
            (SelectIr, true) => Reset,
            (SelectIr, false) => CaptureIr,
            (CaptureIr, true) => Exit1Ir,
            (CaptureIr, false) => ShiftIr,
            (ShiftIr, true) => Exit1Ir,
            (ShiftIr, false) => ShiftIr,
            (Exit1Ir, true) => UpdateIr,
            (Exit1Ir, false) => PauseIr,
            (PauseIr, true) => Exit2Ir,
            (PauseIr, false) => PauseIr,
            (Exit2Ir, true) => UpdateIr,
            (Exit2Ir, false) => ShiftIr,
            (UpdateIr, true) => SelectDr,
            (UpdateIr, false) => Idle,
        }
    }

    /// Shortest TMS sequence from `self` to `to` (BFS over the 16 states).
    /// `Reset` is always reached with five ones, which works from anywhere.
    pub fn path_to(self, to: JtagState) -> Vec<bool> {
        if to == JtagState::Reset {
            return vec![true; 5];
        }
        if self == to {
            return Vec::new();
        }
        let mut prev: std::collections::HashMap<JtagState, (JtagState, bool)> =
            std::collections::HashMap::new();
        let mut q = std::collections::VecDeque::from([self]);
        while let Some(s) = q.pop_front() {
            for tms in [false, true] {
                let n = s.next(tms);
                if n != self && !prev.contains_key(&n) {
                    prev.insert(n, (s, tms));
                    if n == to {
                        let mut path = Vec::new();
                        let mut cur = to;
                        while cur != self {
                            let (p, t) = prev[&cur];
                            path.push(t);
                            cur = p;
                        }
                        path.reverse();
                        return path;
                    }
                    q.push_back(n);
                }
            }
        }
        unreachable!("TAP graph is strongly connected")
    }
}

/// One device on the scan chain (index 0 is nearest TDO).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainDevice {
    /// `None` for a device that powers up in BYPASS (no IDCODE).
    pub idcode: Option<u32>,
    pub ir_len: u8,
    pub name: String,
}

/// IR length for IDCODEs the engine knows (Helion parts and a few common
/// chain neighbours), keyed on the IDCODE without its version nibble.
pub fn known_ir_len(idcode: u32) -> Option<(u8, &'static str)> {
    let id = idcode & 0x0FFF_FFFF;
    if crate::HAD_KNOWN_BOARDS
        .iter()
        .any(|b| b.idcode & 0x0FFF_FFFF == id)
    {
        return Some((Tap::IR_LEN, "Helion HL10T"));
    }
    match id {
        0x0362_D093 => Some((6, "xc7a35t")),
        0x0362_C093 => Some((6, "xc7a50t")),
        0x0363_1093 => Some((6, "xc7a100t")),
        0x0372_7093 => Some((6, "xc7z020 (PL)")),
        0x0BA0_0477 => Some((4, "ARM DAP")),
        0x0111_1043 => Some((8, "LFE5U-25F")),
        _ => None,
    }
}

/// Traffic counters (shown in the UI and asserted in tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JtagStats {
    pub ir_scans: usize,
    pub dr_scans: usize,
    pub bytes_out: usize,
    pub bytes_in: usize,
    pub flushes: usize,
}

/// Native JTAG engine.
pub struct JtagEngine<P: MpssePort> {
    port: P,
    state: JtagState,
    chain: Vec<ChainDevice>,
    target: usize,
    divisor: u16,
    /// Out-only commands not yet written.
    queue: MpsseCmd,
    pub stats: JtagStats,
}

impl<P: MpssePort> std::fmt::Debug for JtagEngine<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JtagEngine")
            .field("port", &self.port.describe())
            .field("state", &self.state)
            .field("chain", &self.chain)
            .field("target", &self.target)
            .field("tck_hz", &self.tck_hz())
            .finish()
    }
}

/// Flush out-only traffic once this much is queued.
const QUEUE_FLUSH_BYTES: usize = 32 * 1024;

impl<P: MpssePort> JtagEngine<P> {
    /// Put the FTDI channel in MPSSE mode, synchronise, configure JTAG pins
    /// and TCK, reset the chain and scan it.
    pub fn open(mut port: P, tck_hz: u32) -> Result<Self, String> {
        port.usb_reset()?;
        port.set_latency_timer(2)?;
        port.set_bitmode(0x00, mpsse::BITMODE_RESET)?;
        port.set_bitmode(mpsse::pin::JTAG_DIR, mpsse::BITMODE_MPSSE)?;
        port.purge()?;
        // AN_135 sync: a bogus opcode must echo back as 0xFA, opcode.
        for bogus in [0xAAu8, 0xAB] {
            let mut c = MpsseCmd::new();
            c.bogus(bogus).send_immediate();
            port.write(c.bytes())?;
            let rx = port.read(2)?;
            if rx != [mpsse::op::BAD_COMMAND, bogus] {
                return Err(format!(
                    "MPSSE sync failed: sent bogus {bogus:#04x}, got {rx:02X?} \
                     (expected FA {bogus:02X}) — not an FTDI MPSSE channel?"
                ));
            }
        }
        let divisor = mpsse::divisor_for_hz(tck_hz);
        let mut c = MpsseCmd::new();
        c.jtag_init(divisor);
        port.write(c.bytes())?;
        let mut e = Self {
            port,
            state: JtagState::Reset,
            chain: Vec::new(),
            target: 0,
            divisor,
            queue: MpsseCmd::new(),
            stats: JtagStats::default(),
        };
        e.stats.bytes_out += c.bytes().len();
        e.reset()?;
        e.scan_chain()?;
        Ok(e)
    }

    pub fn port(&self) -> &P {
        &self.port
    }

    pub fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }

    pub fn state(&self) -> JtagState {
        self.state
    }

    pub fn chain(&self) -> &[ChainDevice] {
        &self.chain
    }

    pub fn target(&self) -> usize {
        self.target
    }

    pub fn divisor(&self) -> u16 {
        self.divisor
    }

    pub fn tck_hz(&self) -> u32 {
        mpsse::tck_hz(self.divisor, false)
    }

    /// Change TCK (takes effect on the next shift).
    pub fn set_tck_hz(&mut self, hz: u32) -> Result<u32, String> {
        self.divisor = mpsse::divisor_for_hz(hz);
        let mut c = MpsseCmd::new();
        c.divisor(self.divisor);
        self.queue.extend(&c);
        self.flush()?;
        Ok(self.tck_hz())
    }

    /// Pick the device later IR/DR scans address.
    pub fn select_target(&mut self, idx: usize) -> Result<(), String> {
        if idx >= self.chain.len() {
            return Err(format!(
                "jtag: target {idx} out of range ({} device(s) on the chain)",
                self.chain.len()
            ));
        }
        self.target = idx;
        Ok(())
    }

    /// Override per-device IR lengths (needed for unknown multi-device chains).
    pub fn set_ir_lengths(&mut self, lens: &[u8]) -> Result<(), String> {
        if lens.len() != self.chain.len() {
            return Err(format!(
                "jtag: {} IR length(s) for {} device(s)",
                lens.len(),
                self.chain.len()
            ));
        }
        for (d, &l) in self.chain.iter_mut().zip(lens) {
            d.ir_len = l;
        }
        Ok(())
    }

    fn queue_tms(&mut self, path: &[bool], tdi: bool) {
        for chunk in path.chunks(7) {
            let mut bits = 0u8;
            for (i, &b) in chunk.iter().enumerate() {
                if b {
                    bits |= 1 << i;
                }
                self.state = self.state.next(b);
            }
            self.queue.tms(bits, chunk.len() as u8, tdi, false);
        }
    }

    /// Write queued out-only commands.
    pub fn flush(&mut self) -> Result<(), String> {
        if self.queue.is_empty() {
            return Ok(());
        }
        debug_assert_eq!(self.queue.read_len(), 0);
        let bytes = self.queue.bytes().to_vec();
        self.queue.clear();
        self.port.write(&bytes)?;
        self.stats.bytes_out += bytes.len();
        self.stats.flushes += 1;
        Ok(())
    }

    fn run(&mut self, cmd: MpsseCmd) -> Result<Vec<Vec<bool>>, String> {
        let mut all = std::mem::take(&mut self.queue);
        all.extend(&cmd);
        if cmd.read_len() == 0 {
            self.queue = all;
            if self.queue.bytes().len() >= QUEUE_FLUSH_BYTES {
                self.flush()?;
            }
            return Ok(Vec::new());
        }
        all.send_immediate();
        self.port.write(all.bytes())?;
        self.stats.bytes_out += all.bytes().len();
        self.stats.flushes += 1;
        let rx = self.port.read(all.read_len())?;
        self.stats.bytes_in += rx.len();
        all.decode(&rx)
    }

    /// Test-Logic-Reset then Run-Test/Idle.
    pub fn reset(&mut self) -> Result<(), String> {
        let mut p = vec![true; 5];
        p.push(false);
        self.queue_tms(&p, false);
        self.state = JtagState::Idle;
        self.flush()
    }

    /// Clock `n` TCKs in Run-Test/Idle.
    pub fn run_test(&mut self, n: usize) -> Result<(), String> {
        let mut p = self.state.path_to(JtagState::Idle);
        p.extend(std::iter::repeat_n(false, n));
        self.queue_tms(&p, false);
        self.flush()
    }

    /// Shift `n` bits through Shift-IR (`ir == true`) or Shift-DR from
    /// Run-Test/Idle and return to Idle via Update. Returns TDO when
    /// `capture`.
    fn scan(&mut self, ir: bool, tdi: &[bool], capture: bool) -> Result<Vec<bool>, String> {
        let n = tdi.len();
        if n == 0 {
            return Err("jtag: zero-length scan".into());
        }
        let shift = if ir {
            JtagState::ShiftIr
        } else {
            JtagState::ShiftDr
        };
        let enter = self.state.path_to(shift);
        self.queue_tms(&enter, false);
        let mut cmd = MpsseCmd::new();
        let body = n - 1;
        let whole = body / 8;
        let rem = body % 8;
        if whole > 0 {
            cmd.data_bytes(&pack_bits(&tdi[..whole * 8]), capture);
        }
        if rem > 0 {
            let b = pack_bits(&tdi[whole * 8..body])[0];
            cmd.data_bits(b, rem as u8, capture);
        }
        // Last bit with TMS=1 (→ Exit1), then Update (1), Idle (0).
        cmd.tms(0b011, 3, tdi[n - 1], capture);
        self.state = JtagState::Idle;
        if ir {
            self.stats.ir_scans += 1;
        } else {
            self.stats.dr_scans += 1;
        }
        let segs = self.run(cmd)?;
        if !capture {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(n);
        let mut it = segs.into_iter();
        if whole > 0 {
            out.extend(it.next().ok_or("jtag: missing byte read")?);
        }
        if rem > 0 {
            out.extend(it.next().ok_or("jtag: missing bit read")?);
        }
        let last = it.next().ok_or("jtag: missing TMS read")?;
        out.push(last[0]);
        Ok(out)
    }

    /// Raw whole-chain IR scan.
    pub fn scan_ir_raw(&mut self, tdi: &[bool]) -> Result<Vec<bool>, String> {
        self.scan(true, tdi, true)
    }

    /// Raw whole-chain DR scan.
    pub fn scan_dr_raw(&mut self, tdi: &[bool]) -> Result<Vec<bool>, String> {
        self.scan(false, tdi, true)
    }

    fn total_ir_len(&self) -> usize {
        self.chain.iter().map(|d| d.ir_len as usize).sum()
    }

    /// Load `ir` into the target device; every other device gets BYPASS.
    /// Returns the target's captured IR bits.
    pub fn shift_ir(&mut self, ir: u64) -> Result<u64, String> {
        if self.chain.is_empty() {
            return Err("jtag: empty chain (no devices answered the IDCODE scan)".into());
        }
        let mut tdi = Vec::with_capacity(self.total_ir_len());
        let mut off = 0;
        for (i, d) in self.chain.iter().enumerate() {
            let len = d.ir_len as usize;
            if i == self.target {
                off = tdi.len();
                tdi.extend((0..len).map(|b| (ir >> b) & 1 == 1));
            } else {
                tdi.extend(std::iter::repeat_n(true, len));
            }
        }
        let tdo = self.scan(true, &tdi, true)?;
        let len = self.chain[self.target].ir_len as usize;
        Ok(bits_to_u64(&tdo[off..off + len]))
    }

    /// Shift `nbits` of `tdi` (LSB first) through the target's current DR.
    /// Bypassed devices add one bit each. Returns the target's captured bits.
    pub fn shift_dr(&mut self, tdi: &[u8], nbits: usize, capture: bool) -> Result<Vec<u8>, String> {
        if self.chain.is_empty() {
            return Err("jtag: empty chain".into());
        }
        let n_dev = self.chain.len();
        let mut bits = Vec::with_capacity(nbits + n_dev);
        bits.extend(std::iter::repeat_n(false, self.target));
        bits.extend(unpack_bits(tdi, nbits));
        bits.extend(std::iter::repeat_n(false, n_dev - 1 - self.target));
        let tdo = self.scan(false, &bits, capture)?;
        if !capture {
            return Ok(Vec::new());
        }
        Ok(pack_bits(&tdo[self.target..self.target + nbits]))
    }

    pub fn shift_dr_u32(&mut self, v: u32) -> Result<u32, String> {
        let r = self.shift_dr(&v.to_le_bytes(), 32, true)?;
        Ok(u32::from_le_bytes([r[0], r[1], r[2], r[3]]))
    }

    pub fn shift_dr_u64(&mut self, v: u64) -> Result<u64, String> {
        let r = self.shift_dr(&v.to_le_bytes(), 64, true)?;
        let mut b = [0u8; 8];
        b.copy_from_slice(&r[..8]);
        Ok(u64::from_le_bytes(b))
    }

    /// Scan the chain after Test-Logic-Reset: every device presents IDCODE
    /// (LSB 1, 32 bits) or BYPASS (one 0 bit). Total IR length is measured
    /// by flushing the IR chain; per-device lengths come from
    /// [`known_ir_len`], or the remainder when one device is unknown.
    pub fn scan_chain(&mut self) -> Result<&[ChainDevice], String> {
        const MAX_DEV: usize = 16;
        self.reset()?;
        let n = 32 * (MAX_DEV + 1);
        let tdo = self.scan(false, &vec![true; n], true)?;
        let mut devs: Vec<(Option<u32>, Option<(u8, &'static str)>)> = Vec::new();
        let mut i = 0usize;
        while i < n {
            if devs.len() > MAX_DEV {
                return Err("jtag: more than 16 devices — TDO stuck low or chain broken".into());
            }
            if !tdo[i] {
                devs.push((None, None));
                i += 1;
                continue;
            }
            if i + 32 > n {
                break;
            }
            let id = bits_to_u64(&tdo[i..i + 32]) as u32;
            if id == 0xFFFF_FFFF {
                break;
            }
            devs.push((Some(id), known_ir_len(id)));
            i += 32;
        }
        if devs.is_empty() {
            self.chain.clear();
            return Err("jtag: no device on the chain (TDO stuck high — no target power?)".into());
        }
        if devs.iter().all(|d| d.0.is_none()) {
            self.chain.clear();
            return Err("jtag: TDO stuck low — no IDCODE on the chain (cable not connected?)".into());
        }
        let total_ir = self.measure_ir_len()?;
        let known: usize = devs.iter().filter_map(|d| d.1.map(|k| k.0 as usize)).sum();
        let unknown = devs.iter().filter(|d| d.1.is_none()).count();
        let mut chain = Vec::with_capacity(devs.len());
        for (id, k) in &devs {
            let (ir_len, name) = match k {
                Some((l, n)) => (*l, (*n).to_string()),
                None if unknown == 1 && total_ir > known => (
                    (total_ir - known) as u8,
                    id.map(|v| format!("unknown {v:#010x}"))
                        .unwrap_or_else(|| "bypass-only device".into()),
                ),
                None => (
                    0,
                    id.map(|v| format!("unknown {v:#010x}"))
                        .unwrap_or_else(|| "bypass-only device".into()),
                ),
            };
            chain.push(ChainDevice {
                idcode: *id,
                ir_len,
                name,
            });
        }
        let sum: usize = chain.iter().map(|d| d.ir_len as usize).sum();
        if sum != total_ir {
            return Err(format!(
                "jtag: chain IR length {total_ir} does not match per-device lengths ({sum}); \
                 set them explicitly with set_ir_lengths"
            ));
        }
        self.chain = chain;
        self.target = self
            .chain
            .iter()
            .position(|d| d.name == "Helion HL10T")
            .unwrap_or(0);
        self.reset()?;
        Ok(&self.chain)
    }

    /// Total IR bits on the chain: flood zeros, then ones, and count the
    /// clocks until the first one reaches TDO.
    fn measure_ir_len(&mut self) -> Result<usize, String> {
        const FLOOD: usize = 128;
        let mut tdi = vec![false; FLOOD];
        tdi.extend(std::iter::repeat_n(true, FLOOD));
        let tdo = self.scan(true, &tdi, true)?;
        let first = tdo[FLOOD..]
            .iter()
            .position(|&b| b)
            .ok_or("jtag: IR length > 128 bits or TDO stuck low")?;
        // The scan left every IR = all ones (BYPASS); back to IDCODE.
        self.reset()?;
        Ok(first)
    }

    // --- Helion TAP operations on the target device -------------------------

    pub fn read_idcode(&mut self) -> Result<u32, String> {
        self.shift_ir(u64::from(IR_IDCODE))?;
        self.shift_dr_u32(0)
    }

    pub fn read_stat_word(&mut self) -> Result<u32, String> {
        self.shift_ir(u64::from(IR_STAT))?;
        self.shift_dr_u32(0)
    }

    /// CFG_W: shift the full `.hbits` packet stream as one DR, Update-DR
    /// commits, then STAT must read back DONE.
    pub fn program(&mut self, bits: &Bitstream) -> Result<Stat, String> {
        crate::refuse_empty_bitstream(bits)?;
        self.shift_ir(u64::from(IR_CFG_W))?;
        let n = bits.packets.len() * 8;
        self.shift_dr(&bits.packets, n, false)?;
        self.flush()?;
        let word = self.read_stat_word()?;
        let stat = stat_from_word(word);
        if !stat.done {
            return Err(format!(
                "program: STAT {word:#010x} after CFG_W — DONE low (TDO readback)"
            ));
        }
        Ok(stat)
    }

    /// Put every device in BYPASS (IR all ones).
    pub fn bypass_all(&mut self) -> Result<(), String> {
        let n = self.total_ir_len();
        self.scan(true, &vec![true; n], false)?;
        self.flush()
    }

    /// USER chain `n` (1..=4) DR access on the target: select the chain,
    /// shift `nbits`, return the captured bits.
    pub fn user_dr(&mut self, chain: u8, tdi: &[u8], nbits: usize) -> Result<Vec<u8>, String> {
        let ir = crate::user_chain_ir(chain)
            .ok_or_else(|| format!("jtag: USER{chain} does not exist (1..=4)"))?;
        self.shift_ir(u64::from(ir))?;
        self.shift_dr(tdi, nbits, true)
    }

    /// Helion IR value for BYPASS (for UI tables).
    pub fn bypass_ir() -> u8 {
        IR_BYPASS
    }
}

/// Decode a Helion STAT DR word (bit 0 CRC_ERR … bit 6 EOS).
pub fn stat_from_word(word: u32) -> Stat {
    let b = |n: u8| (word >> n) & 1 == 1;
    Stat {
        crc_err: b(Stat::BIT_CRC_ERR),
        init: b(Stat::BIT_INIT),
        gts: b(Stat::BIT_GTS),
        gsr: b(Stat::BIT_GSR),
        gwe: b(Stat::BIT_GWE),
        done: b(Stat::BIT_DONE),
        eos: b(Stat::BIT_EOS),
    }
}

fn bits_to_u64(bits: &[bool]) -> u64 {
    bits.iter()
        .take(64)
        .enumerate()
        .fold(0u64, |acc, (i, &b)| acc | (u64::from(b) << i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_state_machine_is_ieee_1149_1() {
        // Five ones reach Reset from every state.
        for s in JtagState::ALL {
            let mut c = s;
            for _ in 0..5 {
                c = c.next(true);
            }
            assert_eq!(c, JtagState::Reset, "from {s:?}");
        }
        assert_eq!(JtagState::Idle.path_to(JtagState::ShiftDr), vec![true, false, false]);
        assert_eq!(
            JtagState::Idle.path_to(JtagState::ShiftIr),
            vec![true, true, false, false]
        );
        assert_eq!(JtagState::Exit1Dr.path_to(JtagState::Idle), vec![true, false]);
        assert_eq!(JtagState::Idle.path_to(JtagState::Idle), Vec::<bool>::new());
        assert_eq!(
            JtagState::PauseDr.path_to(JtagState::ShiftIr),
            vec![true, true, true, true, false, false]
        );
        // Every path lands where it claims.
        for a in JtagState::ALL {
            for b in JtagState::ALL {
                let mut s = a;
                for t in a.path_to(b) {
                    s = s.next(t);
                }
                assert_eq!(s, b, "{a:?} -> {b:?}");
            }
        }
    }

    #[test]
    fn known_ir_lengths() {
        assert_eq!(known_ir_len(0x0001_1A1F).unwrap().0, 6);
        assert_eq!(known_ir_len(0x1362_D093).unwrap().1, "xc7a35t");
        assert!(known_ir_len(0x1234_5677).is_none());
    }
}
