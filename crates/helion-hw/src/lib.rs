//! IEEE 1149.1 TAP + helion-prog + cable detect (sim + openFPGALoader USB).
//!
//! openFPGALoader-class usefulness for HAD: list/detect programming targets,
//! load a `.hbits` from the last implement, program over the sim cable
//! (TAP CFG_W) **or** spawn `openFPGALoader` when on PATH with a USB probe,
//! and report clear errors when nothing is attached / OFL missing.
//!
//! Optional **`usb-native`** feature enables [`rusb`] FTDI (VID 0x0403) enumeration
//! via [`native_usb`] and a real **MPSSE opcode path** via [`native_mpsse`]
//! ([`NativeFtdiMpsse`]: open device when present, encode IR/DR for CFG_W/STAT,
//! INOUT STAT TDO capture + [`parse_stat_tdo_mpsse`]; persistent FTDI session; mock roundtrip tested).
//! Without a device, native returns honest `Io` (never invents STAT). Without the
//! feature, [`NativeFtdiMpsse`] / stub return `NotImplemented` → OFL fallback.
//! [`mpsse_sim`] remains the in-process bitbang CFG_W+STAT harness (sim fabric DONE
//! only — **not** board DONE). HAD board IDs / `HELION_OFL_BOARD` live in
//! [`HAD_KNOWN_BOARDS`]. OFL verify parse stays honest (`TAP_readback=none`).
//! No UNISIM/AMD IP — HAD is Helion's story.

use helion_bits::Bitstream;
use helion_device::Device;
use helion_fabric::{Fabric, Stat};
use std::cell::Cell;

pub mod native_usb;
pub mod native_mpsse;
pub mod mpsse_sim;
pub use native_usb::{enumerate_ftdi, feature_enabled as usb_native_feature_enabled, FtdiDeviceInfo, NativeUsbScan, FTDI_VID};
pub use native_mpsse::{
    native_mpsse_status_note, try_native_mpsse_program, try_native_mpsse_program_stat,
    MpsseOpcodeBuilder, NativeFtdiMpsse, MPSSE_CLK_TMS_OUT_NEG_LSB, MPSSE_SET_CLK_DIVISOR,
};
pub use mpsse_sim::{FtdiBitbangSim, MpsseSimError};


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapState {
    TestLogicReset,
    RunTestIdle,
    SelectDr,
    CaptureDr,
    ShiftDr,
    Exit1Dr,
    UpdateDr,
    SelectIr,
    CaptureIr,
    ShiftIr,
    Exit1Ir,
    UpdateIr,
}

pub const IR_IDCODE: u8 = 0b000011;
pub const IR_STAT: u8 = 0b010010;
pub const IR_CFG_W: u8 = 0b010000;

#[derive(Clone, Debug)]
pub struct Tap {
    pub state: TapState,
    pub ir: u8,
    fabric: Fabric,
    ir_shift: u8,
    /// DR shift register (IDCODE/STAT words use low 32 bits).
    dr_shift: u64,
    shlen: u8,
    /// CFG_W payload accumulating during Shift-DR (`IR_CFG_W`), LSB-first per byte.
    cfg_buf: Vec<u8>,
    /// Bit count written into [`Self::cfg_buf`] for the current CFG_W DR scan.
    cfg_bit_count: u32,
    /// Last CFG_W Update-DR decode/program error (cleared on successful commit).
    cfg_last_err: Option<String>,
}

impl Tap {
    pub fn new(dev: &Device) -> Self {
        Self {
            state: TapState::TestLogicReset,
            ir: IR_IDCODE,
            fabric: Fabric::new(dev),
            ir_shift: 0,
            dr_shift: 0,
            shlen: 0,
            cfg_buf: Vec::new(),
            cfg_bit_count: 0,
            cfg_last_err: None,
        }
    }

    /// IEEE 1149.1 IR length for Helion TAP (matches `IR_*` 6-bit opcodes).
    pub const IR_LEN: u8 = 6;

    /// In-process fabric (sim / bitbang harness). Not a board probe.
    pub fn fabric(&self) -> &Fabric {
        &self.fabric
    }

    /// Last CFG_W Update-DR error, if any.
    pub fn cfg_last_err(&self) -> Option<&str> {
        self.cfg_last_err.as_deref()
    }

    /// One simulated TCK: sample TDO, shift TDI if in Shift-*, then apply TMS.
    ///
    /// Bit-accurate path used by [`mpsse_sim`] — independent of the high-level
    /// [`Self::shift_ir`] shortcut used by the sim cable program path.
    ///
    /// When `IR_CFG_W` is active, Shift-DR appends TDI bits into [`Self::cfg_buf`]
    /// and Update-DR commits via [`Bitstream::from_packets`] + fabric program/startup
    /// (sim fabric DONE only).
    pub fn tick(&mut self, tms: bool, tdi: bool) -> bool {
        let tdo = match self.state {
            TapState::ShiftIr | TapState::CaptureIr => (self.ir_shift & 1) != 0,
            TapState::ShiftDr | TapState::CaptureDr => (self.dr_shift & 1) != 0,
            _ => false,
        };

        match self.state {
            TapState::ShiftIr => {
                // 6-bit IR, LSB-first: shift right, insert TDI at bit 5.
                self.ir_shift =
                    ((self.ir_shift >> 1) & 0x1f) | (u8::from(tdi) << (Self::IR_LEN - 1));
                self.shlen = self.shlen.saturating_add(1);
            }
            TapState::ShiftDr => {
                if self.ir == IR_CFG_W {
                    // Variable-length CFG_W stream (full `.hbits` packets).
                    let byte_i = (self.cfg_bit_count / 8) as usize;
                    let bit_i = (self.cfg_bit_count % 8) as u8;
                    if self.cfg_buf.len() <= byte_i {
                        self.cfg_buf.resize(byte_i + 1, 0);
                    }
                    if tdi {
                        self.cfg_buf[byte_i] |= 1 << bit_i;
                    }
                    self.cfg_bit_count = self.cfg_bit_count.saturating_add(1);
                } else {
                    // 32-bit DR window for IDCODE/STAT.
                    self.dr_shift = (self.dr_shift >> 1) | ((u64::from(tdi)) << 31);
                    self.shlen = self.shlen.saturating_add(1);
                }
            }
            _ => {}
        }

        let prev = self.state;
        self.tms(tms);

        if self.state == TapState::CaptureIr && prev != TapState::CaptureIr {
            self.ir_shift = self.ir & 0x3f;
            self.shlen = 0;
        }
        if self.state == TapState::CaptureDr && prev != TapState::CaptureDr {
            self.dr_shift = match self.ir {
                IR_IDCODE => u64::from(self.fabric.idcode),
                IR_STAT => u64::from(self.fabric.stat.word()),
                IR_CFG_W => {
                    self.cfg_buf.clear();
                    self.cfg_bit_count = 0;
                    self.cfg_last_err = None;
                    0
                }
                _ => 0,
            };
            self.shlen = 0;
        }
        if self.state == TapState::UpdateIr && prev != TapState::UpdateIr {
            self.ir = self.ir_shift & 0x3f;
        }
        if self.state == TapState::UpdateDr && prev != TapState::UpdateDr {
            if self.ir == IR_CFG_W && !self.cfg_buf.is_empty() {
                match Bitstream::from_packets(&self.cfg_buf) {
                    Ok(bits) => match self.fabric.program(&bits) {
                        Ok(()) => {
                            self.fabric.finish_startup();
                            self.cfg_last_err = None;
                        }
                        Err(e) => {
                            self.cfg_last_err = Some(e);
                        }
                    },
                    Err(e) => {
                        self.cfg_last_err = Some(e);
                    }
                }
            }
        }
        tdo
    }

    /// 5× TMS=1 → Test-Logic-Reset.
    pub fn reset(&mut self) {
        self.state = TapState::TestLogicReset;
        self.ir = IR_IDCODE;
    }

    fn tms(&mut self, tms: bool) {
        use TapState::*;
        self.state = match (self.state, tms) {
            (TestLogicReset, false) => RunTestIdle,
            (TestLogicReset, true) => TestLogicReset,
            (RunTestIdle, true) => SelectDr,
            (RunTestIdle, false) => RunTestIdle,
            (SelectDr, true) => SelectIr,
            (SelectDr, false) => CaptureDr,
            (CaptureDr, false) => ShiftDr,
            (CaptureDr, true) => Exit1Dr,
            (ShiftDr, false) => ShiftDr,
            (ShiftDr, true) => Exit1Dr,
            (Exit1Dr, true) => UpdateDr,
            (Exit1Dr, false) => ShiftDr,
            (UpdateDr, false) => RunTestIdle,
            (UpdateDr, true) => SelectDr,
            (SelectIr, true) => TestLogicReset,
            (SelectIr, false) => CaptureIr,
            (CaptureIr, false) => ShiftIr,
            (CaptureIr, true) => Exit1Ir,
            (ShiftIr, false) => ShiftIr,
            (ShiftIr, true) => Exit1Ir,
            (Exit1Ir, true) => UpdateIr,
            (Exit1Ir, false) => ShiftIr,
            (UpdateIr, false) => RunTestIdle,
            (UpdateIr, true) => SelectDr,
        };
    }

    pub fn shift_ir(&mut self, val: u8) {
        self.reset();
        self.tms(false); // idle
        self.tms(true); // select-dr
        self.tms(true); // select-ir
        self.tms(false); // capture-ir
        self.tms(false); // shift-ir
        self.ir_shift = val;
        self.ir = val;
        self.tms(true); // exit1-ir
        self.tms(true); // update-ir
        self.tms(false); // idle
        let _ = self.shlen;
    }

    pub fn read_idcode(&mut self) -> u32 {
        self.shift_ir(IR_IDCODE);
        self.fabric.idcode
    }

    pub fn read_stat(&mut self) -> Stat {
        self.shift_ir(IR_STAT);
        self.fabric.stat.clone()
    }

    /// CFG_W: TAP IR then load frames into the fabric (sim cable).
    pub fn program(&mut self, bits: &Bitstream) -> Result<Stat, String> {
        self.shift_ir(IR_CFG_W);
        self.fabric.program(bits)?;
        self.fabric.finish_startup();
        Ok(self.read_stat())
    }
}

#[derive(Clone, Debug)]
pub struct SimCable {
    tap: Tap,
}

impl SimCable {
    pub fn open(dev: &Device) -> Self {
        Self { tap: Tap::new(dev) }
    }

    pub fn program(&mut self, bits: &Bitstream) -> Result<(), String> {
        self.tap.program(bits)?;
        Ok(())
    }

    pub fn program_partial(&mut self, bits: &Bitstream) -> Result<(), String> {
        self.tap.fabric.program_partial(bits)
    }

    pub fn fabric(&self) -> &Fabric {
        &self.tap.fabric
    }

    pub fn stat(&self) -> Stat {
        self.tap.fabric.stat.clone()
    }
}

pub fn hw_server_program(dev: &Device, bits: &Bitstream) -> Result<Stat, String> {
    let mut c = SimCable::open(dev);
    c.program(bits)?;
    Ok(c.stat())
}

/// helion-prog API (sim cable).
pub fn prog_sim(dev: &Device, bits: &Bitstream) -> Result<Stat, String> {
    hw_server_program(dev, bits)
}

pub fn prog_empty(dev: &Device) -> Result<Stat, String> {
    prog_sim(dev, &Bitstream::empty(dev))
}

/// Program via [`FtdiBitbangSim`] CFG_W bitbang path (sim fabric STAT/DONE only).
pub fn prog_mpsse_sim(dev: &Device, bits: &Bitstream) -> Result<Stat, String> {
    let mut bb = FtdiBitbangSim::new(dev);
    bb.open().map_err(|e| e.to_string())?;
    bb.program_bitstream(bits).map_err(|e| e.to_string())
}

/// Programming backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CableBackend {
    /// In-process TAP + fabric (helion-hw sim cable; high-level CFG_W).
    Sim,
    /// In-process FTDI bitbang harness ([`mpsse_sim`]): IR/DR + CFG_W packet shift + STAT.
    /// Sim fabric DONE only — never board/hardware DONE.
    MpsseSim,
    /// External `openFPGALoader` on PATH (USB/JTAG). Active physical path today.
    OpenFpgaLoader,
    /// Native USB path ([`HadUsbTransport`] / [`NativeFtdiMpsse`]). Persistent FTDI session when `usb-native`+device; else NotImplemented→OFL or Io.
    NativeUsb,
}

impl CableBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            CableBackend::Sim => "sim",
            CableBackend::MpsseSim => "mpsse-sim",
            CableBackend::OpenFpgaLoader => "ofl",
            CableBackend::NativeUsb => "native",
        }
    }
}

/// Error from the native USB / FTDI scaffolding (not openFPGALoader).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeUsbError {
    /// Driver / transport not built yet — callers should fall back to OFL.
    NotImplemented(&'static str),
    /// Probe / IO failure once a real driver exists.
    Io(String),
}

impl std::fmt::Display for NativeUsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeUsbError::NotImplemented(msg) => write!(f, "native USB NotImplemented: {msg}"),
            NativeUsbError::Io(msg) => write!(f, "native USB I/O: {msg}"),
        }
    }
}

/// Helion-native USB/JTAG transport (libusb/rusb + FTDI MPSSE class).
///
/// Probe **enumeration** is available with feature `usb-native` (see [`native_usb`]).
/// [`NativeFtdiMpsse`] encodes real MPSSE opcodes for CFG_W/STAT and opens FTDI when
/// present; without `usb-native` it returns [`NativeUsbError::NotImplemented`] (OFL
/// fallback). Without a device (feature on) it returns [`NativeUsbError::Io`] — never
/// invents Helion TAP STAT / DONE.
pub trait HadUsbTransport {
    fn name(&self) -> &'static str;
    fn open_probe(&mut self) -> Result<(), NativeUsbError>;
    fn program_hbits(
        &mut self,
        path: &std::path::Path,
        flash: bool,
    ) -> Result<(), NativeUsbError>;
    /// Helion TAP STAT word readback over USB/JTAG, when implemented.
    fn read_stat(&mut self) -> Result<Option<u32>, NativeUsbError>;
}

/// Compatibility alias: prefers [`NativeFtdiMpsse`] (real opcode path when `usb-native`).
///
/// Kept so older call sites / docs mentioning the stub still compile. Behavior:
/// feature off → `NotImplemented` → OFL; feature on + no device → `Io` (no STAT).
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFtdiStub;

impl HadUsbTransport for NativeFtdiStub {
    fn name(&self) -> &'static str {
        "native-ftdi-stub"
    }

    fn open_probe(&mut self) -> Result<(), NativeUsbError> {
        NativeFtdiMpsse::new().open_probe()
    }

    fn program_hbits(
        &mut self,
        path: &std::path::Path,
        flash: bool,
    ) -> Result<(), NativeUsbError> {
        let mut t = NativeFtdiMpsse::new();
        t.open_probe()?;
        t.program_hbits(path, flash)
    }

    fn read_stat(&mut self) -> Result<Option<u32>, NativeUsbError> {
        NativeFtdiMpsse::new().read_stat()
    }
}

/// Try native FTDI MPSSE program ([`NativeFtdiMpsse`]).
///
/// - `usb-native` **off**: `NotImplemented` (callers fall back to OFL).
/// - `usb-native` **on**, no FTDI: `Io` (honest — never invents STAT).
/// - device present: persistent MPSSE session; Ok only after live STAT TDO DONE=1.
pub fn try_native_usb_program(
    path: &std::path::Path,
    flash: bool,
) -> Result<(), NativeUsbError> {
    try_native_mpsse_program(path, flash)
}

/// One known Helion HAD board / part row for OFL `-b` defaults and docs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HadBoardId {
    pub part: &'static str,
    pub idcode: u32,
    /// Suggested openFPGALoader `-b` name (Helion-local until upstreamed).
    pub ofl_board: &'static str,
    /// Common FTDI VID used on bring-up cables (heuristic, not a Helion USB ID).
    pub usb_vid: u16,
    /// Common FTDI PID (FT2232H dual = 0x6010) used on bring-up cables.
    pub usb_pid: u16,
    pub note: &'static str,
}

/// Known Helion HAD part → IDCODE / suggested `HELION_OFL_BOARD` table.
pub const HAD_KNOWN_BOARDS: &[HadBoardId] = &[
    HadBoardId {
        part: "HL10T-C32-1",
        idcode: 0x0001_1A1F,
        ofl_board: "helion_hl10t",
        usb_vid: 0x0403,
        usb_pid: 0x6010,
        note: "Helion-T bring-up part; OFL board alias Helion-local (not upstream yet); OFL TAP_readback=none (never invent Helion STAT); native MPSSE=persistent FTDI session+opcodes (Io if no FTDI; no invented STAT); mpsse-sim=sim fabric STAT only",
    },
    HadBoardId {
        part: "HL10T-DSP1",
        idcode: 0x0001_1A1F,
        ofl_board: "helion_hl10t",
        usb_vid: 0x0403,
        usb_pid: 0x6010,
        note: "Same IDCODE as HL10T-C32-1; DSP/MAC27 site variant; OFL TAP_readback=none; native MPSSE=usb-native or NotImplemented→OFL",
    },
];

/// Look up a HAD board row by part name (case-insensitive).
pub fn lookup_had_board(part: &str) -> Option<&'static HadBoardId> {
    let p = part.trim();
    HAD_KNOWN_BOARDS
        .iter()
        .find(|b| b.part.eq_ignore_ascii_case(p))
}

/// Human-readable known-id table for detect / docs.
pub fn had_board_id_table_text() -> String {
    let mut out = String::from(
        "had_board_ids (HELION_OFL_BOARD defaults; Helion-local until OFL upstream)
",
    );
    out.push_str(
        "docs: set HELION_OFL_BOARD=<ofl_board> to pass -b; HELION_OFL_BOARD=none disables -b; unset → default ofl_board for known part. HELION_OFL_CABLE / HELION_OFL_EXTRA / HELION_OFL_VERIFY (flash --verify) / HELION_OFL_DRY_RUN also apply. OFL never invents Helion TAP STAT (TAP_readback=none). Native FTDI MPSSE (--cable native, feature usb-native): open when probe present; Io if none; refuses DONE without validated STAT TDO. Use --cable mpsse-sim for sim fabric CFG_W+STAT (not board DONE).
",
    );
    for b in HAD_KNOWN_BOARDS {
        out.push_str(&format!(
            "had_board part={} idcode={:#010x} ofl_board={} usb_vid={:#06x} usb_pid={:#06x} — {}
",
            b.part, b.idcode, b.ofl_board, b.usb_vid, b.usb_pid, b.note
        ));
    }
    out.push_str(&format!(
        "native_usb: feature={} enumerate=FTDI_VID_0x0403 detect-only; program=NativeFtdiMpsse (opcodes+open; NotImplemented→OFL if feature off; Io if no device); OFL TAP_readback=none (never invent Helion STAT); mpsse_sim=bitbang+CFG_W+STAT (sim fabric DONE only; not hardware DONE)\n",
        if native_usb::feature_enabled() {
            "usb-native"
        } else {
            "off (OFL path)"
        }
    ));
    out.push_str(&native_mpsse_status_note());
    out.push('\n');
    out
}

/// Resolve openFPGALoader `-b` board name.
///
/// Precedence: `HELION_OFL_BOARD` (use `none`/`-`/empty to suppress) → else known-table
/// default for `part` → else `None` (no `-b`).
pub fn resolve_ofl_board(part: Option<&str>) -> Option<String> {
    if let Ok(board) = std::env::var("HELION_OFL_BOARD") {
        let b = board.trim();
        if b.is_empty() || b == "-" || b.eq_ignore_ascii_case("none") {
            return None;
        }
        return Some(b.to_string());
    }
    part.and_then(lookup_had_board)
        .map(|b| b.ofl_board.to_string())
}

fn ofl_verify_enabled() -> bool {
    matches!(
        std::env::var("HELION_OFL_VERIFY").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

/// Whether OFL `--verify` was requested. Note: OFL verify is **SPI flash only**,
/// not Helion TAP STAT readback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OflReadbackKind {
    /// No bitstream/STAT readback available over USB for Helion TAP.
    None,
    /// `openFPGALoader --verify` requested for flash (SPI content check only).
    FlashSpiVerify,
}

impl OflReadbackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OflReadbackKind::None => "none",
            OflReadbackKind::FlashSpiVerify => "flash_spi_verify",
        }
    }
}

/// One entry from `list_cables` / `detect_boards`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CableInfo {
    pub id: String,
    pub backend: CableBackend,
    pub part_hint: String,
    pub detail: String,
}

/// Where a USB probe was discovered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbProbeSource {
    /// `openFPGALoader --scan-usb` line.
    OpenFpgaLoader,
    /// rusb FTDI VID 0x0403 enumeration (`usb-native` feature). Detect-only.
    NativeRusb,
}

impl UsbProbeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            UsbProbeSource::OpenFpgaLoader => "ofl",
            UsbProbeSource::NativeRusb => "native-rusb",
        }
    }
}

/// USB probe row from OFL `--scan-usb` and/or optional rusb FTDI enumeration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsbProbe {
    pub name: String,
    pub detail: String,
    pub source: UsbProbeSource,
}

/// Result of probing for `openFPGALoader` + optional native FTDI (rusb).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsbScan {
    pub ofl_path: Option<std::path::PathBuf>,
    /// Combined OFL + native FTDI probes (detect listing).
    pub probes: Vec<UsbProbe>,
    /// Probes from rusb FTDI VID 0x0403 only (subset / parallel view).
    pub native_probes: Vec<UsbProbe>,
    pub raw: String,
    pub note: String,
    pub native_note: String,
}

/// Result of board/cable detection (honest about missing physical HAD).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectReport {
    pub cables: Vec<CableInfo>,
    /// True when OFL and/or native rusb lists at least one USB probe (detect only — not program DONE).
    pub physical_had: bool,
    pub note: String,
    pub usb: UsbScan,
}

impl DetectReport {
    pub fn text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "detect physical_had={} cables={}\n",
            u8::from(self.physical_had),
            self.cables.len()
        ));
        out.push_str(&format!("note {}\n", self.note));
        match &self.usb.ofl_path {
            Some(p) => out.push_str(&format!("ofl path {}\n", p.display())),
            None => out.push_str("ofl path (not on PATH)\n"),
        }
                let ofl_n = self
            .usb
            .probes
            .iter()
            .filter(|p| p.source == UsbProbeSource::OpenFpgaLoader)
            .count();
        out.push_str(&format!("ofl probes {ofl_n}
"));
        out.push_str(&format!(
            "native_ftdi probes {} feature={}
",
            self.usb.native_probes.len(),
            if native_usb::feature_enabled() {
                "usb-native"
            } else {
                "off"
            }
        ));
        out.push_str(&format!("native_note {}
", self.usb.native_note));
        for p in &self.usb.probes {
            out.push_str(&format!(
                "probe {} source={} — {}
",
                p.name,
                p.source.as_str(),
                p.detail
            ));
        }
        for c in &self.cables {
            out.push_str(&format!(
                "cable {} backend={} part={} — {}\n",
                c.id,
                c.backend.as_str(),
                c.part_hint,
                c.detail
            ));
        }
        out.push_str(&had_board_id_table_text());
        out
    }
}

fn which_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// Resolve `openFPGALoader` binary: `HELION_OPENFPGALOADER` / `HELION_OFL`, else PATH.
pub fn find_openfpgaloader() -> Option<std::path::PathBuf> {
    for key in ["HELION_OPENFPGALOADER", "HELION_OFL"] {
        if let Ok(p) = std::env::var(key) {
            let t = p.trim();
            if !t.is_empty() {
                let pb = std::path::PathBuf::from(t);
                if pb.is_file() {
                    return Some(pb);
                }
                // Still return the path so callers can report a clear missing-file error.
                return Some(pb);
            }
        }
    }
    which_on_path("openFPGALoader")
}

fn ofl_dry_run() -> bool {
    matches!(
        std::env::var("HELION_OFL_DRY_RUN").as_deref(),
        Ok("1") | Ok("true") | Ok("yes") | Ok("TRUE") | Ok("YES")
    )
}

thread_local! {
    static USB_SCAN_INVOCATIONS: Cell<u32> = const { Cell::new(0) };
}

/// How many times this thread shelled `openFPGALoader --scan-usb` / native enumerate.
/// IDE paint must not bump this every frame.
pub fn usb_scan_invocations() -> u32 {
    USB_SCAN_INVOCATIONS.with(|c| c.get())
}

/// Run `openFPGALoader --scan-usb` when available, then merge optional rusb FTDI
/// (VID 0x0403) probes from [`enumerate_ftdi`]. Never fabricates probes. Native
/// listings are detect-only and must not be treated as program DONE.
pub fn scan_usb_probes() -> UsbScan {
    USB_SCAN_INVOCATIONS.with(|c| c.set(c.get().saturating_add(1)));
    let native = enumerate_ftdi();
    let native_probes: Vec<UsbProbe> = native
        .probes
        .iter()
        .map(|d| UsbProbe {
            name: d.name(),
            detail: d.detail(),
            source: UsbProbeSource::NativeRusb,
        })
        .collect();
    let native_note = native.note.clone();

    let mut scan = scan_ofl_only(&native_probes, &native_note);

    // Merge native FTDI probes into the combined list (dedup by detail).
    for p in &native_probes {
        if !scan.probes.iter().any(|e| e.detail == p.detail) {
            scan.probes.push(p.clone());
        }
    }
    if !native_probes.is_empty() {
        eprintln!(
            "detect: native rusb FTDI VID {:#06x}: {} probe(s) (detect-only, no DONE)",
            FTDI_VID,
            native_probes.len()
        );
    }
    if !scan.native_probes.is_empty() || native_usb::feature_enabled() {
        scan.note = format!(
            "{}; {}",
            scan.note.trim_end_matches('.'),
            scan.native_note
        );
    }
    scan
}

fn scan_ofl_only(native_probes: &[UsbProbe], native_note: &str) -> UsbScan {
    let Some(ofl) = find_openfpgaloader() else {
        return UsbScan {
            ofl_path: None,
            probes: Vec::new(),
            native_probes: native_probes.to_vec(),
            raw: String::new(),
            note: "openFPGALoader not on PATH (set HELION_OPENFPGALOADER or install openFPGALoader)"
                .into(),
            native_note: native_note.to_string(),
        };
    };
    if !ofl.is_file() {
        return UsbScan {
            ofl_path: Some(ofl.clone()),
            probes: Vec::new(),
            native_probes: native_probes.to_vec(),
            raw: String::new(),
            note: format!(
                "openFPGALoader path {} is not a file",
                ofl.display()
            ),
            native_note: native_note.to_string(),
        };
    }
    let mut cmd = std::process::Command::new(&ofl);
    cmd.arg("--scan-usb");
    eprintln!("detect: invoking {} --scan-usb", ofl.display());
    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let raw = format!("{stdout}{stderr}");
            let probes = parse_scan_usb_output(&raw);
            let note = if probes.is_empty() {
                if out.status.success() {
                    "openFPGALoader --scan-usb: no USB programmer found".into()
                } else {
                    format!(
                        "openFPGALoader --scan-usb exit {}: {}",
                        out.status.code().unwrap_or(-1),
                        raw.lines().next().unwrap_or("(no output)").trim()
                    )
                }
            } else {
                format!(
                    "openFPGALoader --scan-usb: {} probe(s)",
                    probes.len()
                )
            };
            UsbScan {
                ofl_path: Some(ofl),
                probes,
                native_probes: native_probes.to_vec(),
                raw,
                note,
                native_note: native_note.to_string(),
            }
        }
        Err(e) => UsbScan {
            ofl_path: Some(ofl),
            probes: Vec::new(),
            native_probes: native_probes.to_vec(),
            raw: String::new(),
            note: format!("failed to spawn openFPGALoader: {e}"),
            native_note: native_note.to_string(),
        },
    }
}

fn parse_scan_usb_output(raw: &str) -> Vec<UsbProbe> {
    let mut probes = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_ascii_lowercase();
        // OFL 0.13.x prints column header + "empty"/"No USB devices found" even with 0 probes.
        // Never treat header chatter as a programmer (would falsely set physical_had).
        let skip = lower == "empty"
            || lower.starts_with("bus device")
            || lower.contains("no usb")
            || lower.contains("nothing")
            || lower.starts_with("usage")
            || lower.contains("not found")
            // Column header: "vid:pid" without a hex id.
            || (lower.contains("vid:pid") && !lower.contains("0x"));
        if skip {
            continue;
        }
        // Require a hex VID/PID (0x…) — header lines lack it; real probes and our fixtures have it.
        let has_hex_id = lower.contains("0x");
        let interesting = has_hex_id
            && (lower.contains("vid")
                || lower.contains("pid")
                || lower.contains("ftdi")
                || lower.contains("probe")
                || lower.contains("cable")
                || lower.contains("usb")
                || lower.contains(':'));
        if interesting {
            let name = t.split_whitespace().next().unwrap_or("usb").to_string();
            probes.push(UsbProbe {
                name,
                detail: t.to_string(),
                source: UsbProbeSource::OpenFpgaLoader,
            });
        }
    }
    // Dedup identical detail lines.
    probes.dedup_by(|a, b| a.detail == b.detail);
    probes
}

fn sim_cable_info() -> CableInfo {
    CableInfo {
        id: "sim0".into(),
        backend: CableBackend::Sim,
        part_hint: "HL10T-C32-1".into(),
        detail: "helion-hw sim cable (TAP CFG_W); in-process fabric".into(),
    }
}

fn mpsse_sim_cable_info() -> CableInfo {
    CableInfo {
        id: "mpsse-sim0".into(),
        backend: CableBackend::MpsseSim,
        part_hint: "HL10T-C32-1".into(),
        detail: "FTDI bitbang harness (Tap::tick): CFG_W .hbits DR + STAT; sim fabric DONE only — not board DONE".into(),
    }
}

fn ofl_cable_info(scan: &UsbScan) -> CableInfo {
    let ofl_probes: Vec<_> = scan
        .probes
        .iter()
        .filter(|p| p.source == UsbProbeSource::OpenFpgaLoader)
        .collect();
    let detail = if scan.ofl_path.is_none() {
        "openFPGALoader backend (binary not on PATH)".into()
    } else if ofl_probes.is_empty() {
        format!(
            "openFPGALoader backend — no USB probe ({})",
            scan.note
        )
    } else {
        format!(
            "openFPGALoader backend — {} USB probe(s); {}",
            ofl_probes.len(),
            ofl_probes
                .first()
                .map(|p| p.detail.as_str())
                .unwrap_or("")
        )
    };
    CableInfo {
        id: "ofl0".into(),
        backend: CableBackend::OpenFpgaLoader,
        part_hint: "HL10T-C32-1".into(),
        detail,
    }
}

fn native_cable_info(scan: &UsbScan) -> CableInfo {
    let enum_bit = if native_usb::feature_enabled() {
        if scan.native_probes.is_empty() {
            "rusb FTDI enumerate on (0 devices)"
        } else {
            "rusb FTDI enumerate on (probe(s) listed; detect-only)"
        }
    } else {
        "usb-native feature off"
    };
    CableInfo {
        id: "native0".into(),
        backend: CableBackend::NativeUsb,
        part_hint: "HL10T-C32-1".into(),
        detail: format!(
            "native USB ({enum_bit}); MPSSE opcodes via NativeFtdiMpsse (feature off→NotImplemented→OFL; no device→Io; never invents STAT / never DONE from enumerate alone)"
        ),
    }
}

/// List programming cables: `sim0`, `ofl0`, and `native0` (stub).
pub fn list_cables() -> Vec<CableInfo> {
    let scan = scan_usb_probes();
    vec![
        sim_cable_info(),
        mpsse_sim_cable_info(),
        ofl_cable_info(&scan),
        native_cable_info(&scan),
    ]
}

/// Detect programming targets. Sim always present; physical when OFL and/or
/// native rusb lists probes. Enumeration alone never claims program DONE.
pub fn detect_boards() -> DetectReport {
    let usb = scan_usb_probes();
    let physical_had = !usb.probes.is_empty();
    let cables = vec![
        sim_cable_info(),
        mpsse_sim_cable_info(),
        ofl_cable_info(&usb),
        native_cable_info(&usb),
    ];
    let ofl_n = usb
        .probes
        .iter()
        .filter(|p| p.source == UsbProbeSource::OpenFpgaLoader)
        .count();
    let native_n = usb.native_probes.len();
    let note = if physical_had {
        format!(
            "Physical USB probe(s) listed: ofl={ofl_n} native_ftdi={native_n} (detect only — program DONE requires OFL/sim/mpsse-sim success or validated native STAT TDO, not enumerate). OFL TAP_readback=none; native MPSSE=opcodes+open when usb-native (Io if open fails; NotImplemented→OFL if feature off). Use --cable usb|ofl|auto|native|mpsse-sim|sim."
        )
    } else if usb.ofl_path.is_some() || native_usb::feature_enabled() {
        format!(
            "No USB programmer attached. {}. Use --cable sim, or attach HAD/JTAG and retry detect.",
            usb.note
        )
    } else {
        "openFPGALoader not on PATH and usb-native off — cannot probe USB. Install openFPGALoader (or set HELION_OPENFPGALOADER) or build helion-hw with --features usb-native for FTDI enumerate + MPSSE opcode open path. Sim/mpsse-sim cables remain available (--cable sim|mpsse-sim); native without feature stays NotImplemented→OFL; with feature and no FTDI → honest Io (no invented STAT).".into()
    };
    DetectReport {
        cables,
        physical_had,
        note,
        usb,
    }
}

/// Resolve `--cable auto|sim|mpsse-sim|usb|ofl|native|sim0|mpsse-sim0|ofl0|usb0|native0`.
///
/// `auto` always selects the OFL board path (probe present → program may DONE;
/// USB=0 → program fails honestly). Never falls back to sim DONE.
pub fn resolve_cable(spec: &str) -> Result<CableInfo, String> {
    resolve_cable_from(spec, &detect_boards())
}

/// Paint-path resolve: uses a cached [`DetectReport`]. Must not shell OFL.
pub fn resolve_cable_from(spec: &str, det: &DetectReport) -> Result<CableInfo, String> {
    let s = spec.trim().to_ascii_lowercase();
    let sim = det
        .cables
        .iter()
        .find(|c| c.backend == CableBackend::Sim)
        .cloned()
        .unwrap_or_else(sim_cable_info);
    let ofl = det
        .cables
        .iter()
        .find(|c| c.backend == CableBackend::OpenFpgaLoader)
        .cloned()
        .unwrap_or_else(|| ofl_cable_info(&det.usb));
    let native = det
        .cables
        .iter()
        .find(|c| c.backend == CableBackend::NativeUsb)
        .cloned()
        .unwrap_or_else(|| native_cable_info(&det.usb));
    let mpsse = det
        .cables
        .iter()
        .find(|c| c.backend == CableBackend::MpsseSim)
        .cloned()
        .unwrap_or_else(mpsse_sim_cable_info);
    match s.as_str() {
        "" | "sim" | "sim0" => Ok(sim),
        "mpsse-sim" | "mpsse_sim" | "mpsse-sim0" | "bitbang" => Ok(mpsse),
        "usb" | "usb0" | "ofl" | "ofl0" | "openfpgaloader" => Ok(ofl),
        "native" | "native0" | "ftdi" | "libusb" => Ok(native),
        "auto" => {
            // Prefer OFL for board program. With USB=0, resolve to ofl so program
            // fails honestly (no soft-hold / no invented sim DONE). Explicit
            // --cable sim|mpsse-sim remains for in-process fabric only.
            let _ = &sim;
            Ok(ofl)
        }
        other => Err(format!(
            "unknown cable {other:?}: use --cable auto|sim|mpsse-sim|usb|ofl|native"
        )),
    }
}

/// Outcome of an openFPGALoader program attempt (never claims TAP DONE without a device).
#[derive(Clone, Debug)]
pub struct OflProgramReport {
    pub command: String,
    pub dry_run: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Board `-b` actually passed (after [`resolve_ofl_board`]), if any.
    pub board: Option<String>,
    /// Helion TAP STAT readback is never available via OFL today.
    pub tap_readback: bool,
    /// OFL `--verify` kind (SPI flash only when enabled).
    pub readback: OflReadbackKind,
    /// Parsed from OFL stdout/stderr after program (SPI verify honesty).
    pub verify_ok: Option<bool>,
    /// Short parse note (e.g. which verify phrase matched).
    pub verify_detail: String,
}

/// Parse openFPGALoader stdout/stderr for verify / done phrases.
///
/// Helion TAP `IR_STAT` is never present in OFL output — this only reflects
/// OFL's own programmer/verify messages (SPI `--verify` or generic Done).
pub fn parse_ofl_verify_output(
    stdout: &str,
    stderr: &str,
    readback: OflReadbackKind,
    exit_ok: bool,
) -> (Option<bool>, String) {
    let combined = format!("{stdout}\n{stderr}");
    let lower = combined.to_ascii_lowercase();
    // Failure phrases first (honest).
    let fail_markers = [
        "verify failed",
        "verification failed",
        "verify: fail",
        "verify error",
        "mismatch",
        "crc error",
        "done failed",
    ];
    for m in fail_markers {
        if lower.contains(m) {
            return (Some(false), format!("ofl_output contains {m:?}"));
        }
    }
    let ok_markers = [
        "verify: ok",
        "verify ok",
        "verification ok",
        "verified successfully",
        "verify success",
        "flash verified",
    ];
    for m in ok_markers {
        if lower.contains(m) {
            return (Some(true), format!("ofl_output contains {m:?}"));
        }
    }
    // Generic Done / done writing — programmer success, still not TAP STAT.
    let done_markers = ["done", "finished", "successfully"];
    let has_doneish = done_markers.iter().any(|m| lower.contains(m));
    match readback {
        OflReadbackKind::FlashSpiVerify => {
            if exit_ok {
                (
                    Some(true),
                    "HELION_OFL_VERIFY set and ofl exit 0 (no explicit verify phrase; not TAP STAT)"
                        .into(),
                )
            } else {
                (Some(false), "HELION_OFL_VERIFY set but ofl exit non-zero".into())
            }
        }
        OflReadbackKind::None => {
            if has_doneish && exit_ok {
                (
                    None,
                    "ofl programmer success phrase seen; TAP_readback=none STAT=(no readback)".into(),
                )
            } else if exit_ok {
                (
                    None,
                    "ofl exit 0; no verify phrase; TAP_readback=none STAT=(no readback)".into(),
                )
            } else {
                (None, "ofl exit non-zero".into())
            }
        }
    }
}

fn format_command(program: &std::path::Path, args: &[String]) -> String {
    let mut s = format!("{}", program.display());
    for a in args {
        s.push(' ');
        if a.contains(' ') {
            s.push('"');
            s.push_str(a);
            s.push('"');
        } else {
            s.push_str(a);
        }
    }
    s
}

fn build_ofl_program_args(
    bitstream: &std::path::Path,
    flash: bool,
    part: Option<&str>,
) -> (Vec<String>, Option<String>, OflReadbackKind) {
    let mut args = Vec::new();
    let board = resolve_ofl_board(part);
    if let Some(ref b) = board {
        args.push("-b".into());
        args.push(b.clone());
    } else if let Ok(cable) = std::env::var("HELION_OFL_CABLE") {
        // Cable only when no board (OFL typically wants one of -b / -c).
        let c = cable.trim();
        if !c.is_empty() {
            args.push("-c".into());
            args.push(c.to_string());
        }
    }
    // If board was set via default/env but user also set cable, append cable via EXTRA
    // or when board is set and HELION_OFL_CABLE is set, pass both (OFL accepts -b and -c).
    if board.is_some() {
        if let Ok(cable) = std::env::var("HELION_OFL_CABLE") {
            let c = cable.trim();
            if !c.is_empty() {
                args.push("-c".into());
                args.push(c.to_string());
            }
        }
    }
    if flash {
        args.push("-f".into());
    } else {
        args.push("-m".into());
    }
    // OFL `--verify` is SPI-flash only — never Helion TAP STAT. Opt-in via HELION_OFL_VERIFY.
    let readback = if flash && ofl_verify_enabled() {
        args.push("--verify".into());
        OflReadbackKind::FlashSpiVerify
    } else {
        OflReadbackKind::None
    };
    if let Ok(extra) = std::env::var("HELION_OFL_EXTRA") {
        for tok in extra.split_whitespace() {
            args.push(tok.to_string());
        }
    }
    args.push(bitstream.display().to_string());
    (args, board, readback)
}

/// Invoke openFPGALoader to program `bitstream` (`.hbits` or converted file).
///
/// Fails honestly when: OFL missing, no USB probe, dry-run, or OFL non-zero exit.
/// Does **not** invent TAP STAT DONE — caller must not claim hardware DONE on Err.
pub fn program_via_openfpgaloader(
    bitstream: &std::path::Path,
    flash: bool,
) -> Result<OflProgramReport, String> {
    program_via_openfpgaloader_for_part(bitstream, flash, None)
}

/// Like [`program_via_openfpgaloader`], applying HAD board-id defaults for `part`.
pub fn program_via_openfpgaloader_for_part(
    bitstream: &std::path::Path,
    flash: bool,
    part: Option<&str>,
) -> Result<OflProgramReport, String> {
    let ofl = find_openfpgaloader().ok_or_else(|| {
        "program: openFPGALoader not on PATH — install it or set HELION_OPENFPGALOADER; \
         or use --cable sim"
            .to_string()
    })?;
    if !ofl.is_file() {
        return Err(format!(
            "program: openFPGALoader path {} is not a file",
            ofl.display()
        ));
    }
    let scan = scan_usb_probes();
    if scan.probes.is_empty() {
        return Err(format!(
            "program: no USB programmer detected — {}\n  \
             Attach HAD / JTAG cable and re-run detect, or use --cable sim",
            scan.note
        ));
    }
    if !bitstream.is_file() {
        return Err(format!(
            "program: bitstream not found: {}",
            bitstream.display()
        ));
    }
    let (args, board, readback) = build_ofl_program_args(bitstream, flash, part);
    if let Some(ref b) = board {
        eprintln!(
            "program: HELION_OFL_BOARD/default → -b {b} (Helion-local alias until OFL upstream)"
        );
    } else if let Some(p) = part {
        if let Some(row) = lookup_had_board(p) {
            eprintln!(
                "program: tip set HELION_OFL_BOARD={} for part {} (currently no -b)",
                row.ofl_board, p
            );
        }
    }
    if flash && readback == OflReadbackKind::None {
        eprintln!(
            "program: no TAP readback over USB; OFL --verify is SPI-flash only \
             (set HELION_OFL_VERIFY=1 to request flash verify)"
        );
    } else if !flash {
        eprintln!(
            "program: no TAP STAT readback over USB (OFL SRAM path has no Helion IR_STAT)"
        );
    }
    let command = format_command(&ofl, &args);
    eprintln!("program: invoking {command}");
    if ofl_dry_run() {
        eprintln!("program: HELION_OFL_DRY_RUN=1 — not executing (no DONE claimed)");
        return Err(format!(
            "program: dry-run only (HELION_OFL_DRY_RUN=1); would run: {command}"
        ));
    }
    let mut cmd = std::process::Command::new(&ofl);
    cmd.args(&args);
    let out = cmd
        .output()
        .map_err(|e| format!("program: failed to spawn openFPGALoader: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !stdout.trim().is_empty() {
        eprintln!("program: ofl stdout:\n{}", stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        eprintln!("program: ofl stderr:\n{}", stderr.trim_end());
    }
    let code = out.status.code();
    if !out.status.success() {
        return Err(format!(
            "program: openFPGALoader failed (exit {}): {}\n  cmd: {command}",
            code.unwrap_or(-1),
            stderr
                .lines()
                .chain(stdout.lines())
                .find(|l| !l.trim().is_empty())
                .unwrap_or("(no output)")
                .trim()
        ));
    }
    let (verify_ok, verify_detail) =
        parse_ofl_verify_output(&stdout, &stderr, readback, true);
    if verify_ok == Some(false) {
        return Err(format!(
            "program: openFPGALoader exit 0 but verify parse failed ({verify_detail});              refusing DONE (no TAP STAT). cmd: {command}"
        ));
    }
    Ok(OflProgramReport {
        command,
        dry_run: false,
        exit_code: code,
        stdout,
        stderr,
        board,
        tap_readback: false,
        readback,
        verify_ok,
        verify_detail,
    })
}

/// Load `.hbits` packets and program the sim cable for `dev`.
pub fn program_packets(dev: &Device, packets: &[u8]) -> Result<(Bitstream, Stat), String> {
    if packets.is_empty() {
        return Err("program: empty bitstream (0 bytes)".into());
    }
    let bits = Bitstream::from_packets(packets)?;
    if bits.idcode != dev.idcode {
        return Err(format!(
            "program: bitstream idcode {:#010x} != device {} idcode {:#010x}",
            bits.idcode, dev.part, dev.idcode
        ));
    }
    let st = prog_sim(dev, &bits)?;
    Ok((bits, st))
}

/// Program a path to a `.hbits` file over the sim cable.
pub fn program_hbits_path(
    dev: &Device,
    path: &std::path::Path,
) -> Result<(Bitstream, Stat), String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("program: read {}: {e}", path.display()))?;
    eprintln!(
        "program: loading {} ({} bytes) onto {} via sim cable…",
        path.display(),
        bytes.len(),
        dev.part
    );
    let (bits, st) = program_packets(dev, &bytes)?;
    eprintln!(
        "program: progress CFG_W frames={} → STAT DONE={} GWE={} CRC_ERR={}",
        bits.frames.len(),
        st.done as u8,
        st.gwe as u8,
        st.crc_err as u8
    );
    Ok((bits, st))
}

/// Program `.hbits` using the resolved cable backend.
///
/// - `Sim`: in-process TAP; returns fabric STAT (DONE only after CFG_W).
/// - `OpenFpgaLoader`: requires OFL on PATH + USB probe; never returns DONE on soft-fail.
pub fn program_hbits_with_cable(
    dev: &Device,
    path: &std::path::Path,
    cable: &CableInfo,
    flash: bool,
) -> Result<ProgramOutcome, String> {
    match cable.backend {
        CableBackend::Sim => {
            let (bits, st) = program_hbits_path(dev, path)?;
            Ok(ProgramOutcome::Sim { bits, stat: st })
        }
        CableBackend::MpsseSim => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("program: read {}: {e}", path.display()))?;
            let bits = Bitstream::from_packets(&bytes)?;
            if bits.idcode != dev.idcode {
                return Err(format!(
                    "program: bitstream idcode {:#010x} != device {} idcode {:#010x}",
                    bits.idcode, dev.part, dev.idcode
                ));
            }
            eprintln!(
                "program: mpsse-sim CFG_W bitbang {} ({} bytes) → STAT readback (sim fabric only)…",
                path.display(),
                bytes.len()
            );
            let st = prog_mpsse_sim(dev, &bits)?;
            Ok(ProgramOutcome::MpsseSim { bits, stat: st })
        }
        CableBackend::NativeUsb => {
            // NativeFtdiMpsse: NotImplemented (feature off) → OFL; Io (no device /
            // unvalidated STAT) → hard error — never invent Helion TAP STAT.
            // Ok(stat_word) only after persistent-session live TDO parse with DONE=1.
            match try_native_mpsse_program_stat(path, flash) {
                Ok(stat_word) => {
                    let bytes = std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0);
                    eprintln!(
                        "program: native MPSSE persistent session STAT={stat_word:#010x} DONE=1 (live TDO)"
                    );
                    Ok(ProgramOutcome::NativeMpsse {
                        bytes,
                        stat_word,
                    })
                }
                Err(NativeUsbError::NotImplemented(msg)) => {
                    eprintln!(
                        "program: native MPSSE NotImplemented → {msg}; falling back to openFPGALoader"
                    );
                    program_hbits_with_cable(
                        dev,
                        path,
                        &CableInfo {
                            id: "ofl0".into(),
                            backend: CableBackend::OpenFpgaLoader,
                            part_hint: cable.part_hint.clone(),
                            detail: "OFL fallback after native NotImplemented".into(),
                        },
                        flash,
                    )
                }
                Err(NativeUsbError::Io(msg)) => Err(format!(
                    "program: native MPSSE I/O (no invented STAT): {msg}"
                )),
            }
        }
        CableBackend::OpenFpgaLoader => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("program: read {}: {e}", path.display()))?;
            if bytes.is_empty() {
                return Err("program: empty bitstream (0 bytes)".into());
            }
            // Validate Helion .hbits when magic matches; still pass file to OFL as-is.
            let bits = if bytes.starts_with(b"HBIT") {
                let b = Bitstream::from_packets(&bytes)?;
                if b.idcode != dev.idcode {
                    return Err(format!(
                        "program: bitstream idcode {:#010x} != device {} idcode {:#010x}",
                        b.idcode, dev.part, dev.idcode
                    ));
                }
                Some(b)
            } else {
                eprintln!(
                    "program: {} is not Helion HBIT magic — passing through to openFPGALoader",
                    path.display()
                );
                None
            };
            eprintln!(
                "program: loading {} ({} bytes) onto {} via openFPGALoader ({})…",
                path.display(),
                bytes.len(),
                dev.part,
                if flash { "flash" } else { "sram" }
            );
            let ofl = program_via_openfpgaloader_for_part(path, flash, Some(dev.part.as_str()))?;
            Ok(ProgramOutcome::OpenFpgaLoader {
                bits,
                ofl,
                bytes: bytes.len(),
            })
        }
    }
}

/// Result of `program_hbits_with_cable`.
#[derive(Clone, Debug)]
pub enum ProgramOutcome {
    Sim {
        bits: Bitstream,
        stat: Stat,
    },
    /// Bitbang CFG_W + STAT via [`FtdiBitbangSim`] (sim fabric DONE — not board DONE).
    MpsseSim {
        bits: Bitstream,
        stat: Stat,
    },
    /// Native FTDI MPSSE persistent session with live STAT TDO DONE=1 (real probe).
    NativeMpsse {
        bytes: usize,
        /// Helion STAT word parsed from live TDO (bit5 DONE must be 1).
        stat_word: u32,
    },
    OpenFpgaLoader {
        bits: Option<Bitstream>,
        ofl: OflProgramReport,
        bytes: usize,
    },
}

impl ProgramOutcome {
    pub fn backend(&self) -> CableBackend {
        match self {
            ProgramOutcome::Sim { .. } => CableBackend::Sim,
            ProgramOutcome::MpsseSim { .. } => CableBackend::MpsseSim,
            ProgramOutcome::NativeMpsse { .. } => CableBackend::NativeUsb,
            ProgramOutcome::OpenFpgaLoader { .. } => CableBackend::OpenFpgaLoader,
        }
    }

    /// Human summary line for CLI / GUI. Claims DONE only for sim TAP or OFL exit 0.
    /// OFL path never invents Helion TAP STAT bits — reports `TAP_readback=none`.
    /// `mpsse-sim` reports real sim-fabric STAT from bitbang readback (still not board DONE).
    pub fn summary_line(&self, sub: &str, part: &str) -> String {
        match self {
            ProgramOutcome::Sim { bits, stat } => format!(
                "hw {sub} backend=sim part={part} frames={} bytes={} STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={} (sim fabric; not board DONE)",
                bits.frames.len(),
                bits.packets.len(),
                stat.init as u8,
                stat.done as u8,
                stat.eos as u8,
                stat.gwe as u8,
                stat.gsr as u8,
                stat.gts as u8,
                stat.crc_err as u8
            ),
            ProgramOutcome::MpsseSim { bits, stat } => format!(
                "hw {sub} backend=mpsse-sim part={part} frames={} bytes={} STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={} (sim fabric via bitbang CFG_W; not board DONE)",
                bits.frames.len(),
                bits.packets.len(),
                stat.init as u8,
                stat.done as u8,
                stat.eos as u8,
                stat.gwe as u8,
                stat.gsr as u8,
                stat.gts as u8,
                stat.crc_err as u8
            ),
            ProgramOutcome::NativeMpsse { bytes, stat_word } => {
                let done = u8::from((stat_word >> helion_fabric::Stat::BIT_DONE) & 1 != 0);
                format!(
                    "hw {sub} backend=native-mpsse part={part} bytes={bytes} STAT={stat_word:#010x} DONE={done} (live TDO on persistent FTDI session; board STAT)"
                )
            }
            ProgramOutcome::OpenFpgaLoader { bits, ofl, bytes } => {
                let frames = bits.as_ref().map(|b| b.frames.len()).unwrap_or(0);
                let board = ofl
                    .board
                    .as_deref()
                    .unwrap_or("-");
                let vok = match ofl.verify_ok {
                    Some(true) => "1",
                    Some(false) => "0",
                    None => "unknown",
                };
                format!(
                    "hw {sub} backend=ofl part={part} frames={frames} bytes={bytes} ofl_board={board} ofl_exit={} DONE=1 (programmer ok; no TAP readback) TAP_readback=none ofl_verify={} verify_ok={vok} verify_detail={} STAT=(no readback) cmd={}",
                    ofl.exit_code.unwrap_or(0),
                    ofl.readback.as_str(),
                    ofl.verify_detail.replace(' ', "_"),
                    ofl.command
                )
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static OFL_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn tap_idcode_and_cfg_w_stat() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut tap = Tap::new(&dev);
        assert_eq!(tap.read_idcode(), 0x0001_1A1F);
        let st = tap.program(&Bitstream::empty(&dev)).unwrap();
        assert!(st.init && st.done && st.eos && st.gwe);
        assert!(!st.gsr && !st.gts && !st.crc_err);
        assert_eq!(tap.ir, IR_STAT);
        assert_eq!(st.word(), helion_fabric::Stat::STARTUP_WORD);
        let mut idle = Tap::new(&dev);
        assert_eq!(idle.read_idcode(), 0x0001_1A1F);
        let rst = idle.read_stat();
        assert_eq!(rst.word(), helion_fabric::Stat::RESET_WORD);
        assert_eq!(idle.ir, IR_STAT);
        assert_ne!(rst.word(), st.word());
    }

    #[test]
    fn helion_prog_mpsse_sim_counter_cfg_w() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let cable = resolve_cable("mpsse-sim").unwrap();
        assert_eq!(cable.backend, CableBackend::MpsseSim);
        let path = std::path::Path::new("/tmp/counter.hbits");
        if !path.is_file() {
            let bits = Bitstream::empty(&dev);
            let st = prog_mpsse_sim(&dev, &bits).unwrap();
            assert!(st.done);
            return;
        }
        let ok = program_hbits_with_cable(&dev, path, &cable, false).unwrap();
        match ok {
            ProgramOutcome::MpsseSim { stat, .. } => {
                assert!(stat.done);
                assert_eq!(stat.word(), helion_fabric::Stat::STARTUP_WORD);
            }
            other => panic!("expected MpsseSim outcome, got {:?}", other.backend()),
        }
    }

    #[test]
    fn helion_prog_sim_empty() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let st = prog_empty(&dev).unwrap();
        assert!(st.done && st.gwe && !st.crc_err);
    }

    #[test]
    fn dfx_partial_on_sim_cable() {
        use helion_bits::{bitgen, bitgen_pblock};
        use helion_ir::{CellKind, Design, PortDir};
        use helion_pack::pack;
        use helion_place::{place_with, PlaceOpts};
        use helion_route::route;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        fn nine(last: u64) -> Design {
            let mut d = Design::new("dfx");
            d.add_port("clk", PortDir::In);
            d.add_port("led", PortDir::Out);
            for i in 0..9u32 {
                let init = if i == 8 { last } else { 0x5555_5555_5555_5555 };
                d.add_cell(format!("u_lut{i}"), CellKind::Lut6 { init });
                d.add_cell(format!("u_ff{i}"), CellKind::Hff);
                d.connect("clk", format!("u_ff{i}"), "CLK");
                d.connect(format!("d{i}"), format!("u_lut{i}"), "O");
                d.connect(format!("d{i}"), format!("u_ff{i}"), "D");
                d.connect(format!("q{i}"), format!("u_ff{i}"), "Q");
                d.connect(format!("q{i}"), format!("u_lut{i}"), "I0");
            }
            d.add_cell("u_iob", CellKind::IobOut);
            d.connect("q0", "u_iob", "I");
            d.connect("led", "u_iob", "PAD");
            d
        }
        let pa = pack(&nine(0x5555_5555_5555_5555), &dev).unwrap();
        let pb = pack(&nine(0xAAAA_AAAA_AAAA_AAAA), &dev).unwrap();
        let pla = place_with(&pa, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let plb = place_with(&pb, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let ra = route(&pla, &dev).unwrap();
        let rb = route(&plb, &dev).unwrap();
        let full_a = bitgen(&dev, &ra).unwrap();
        let (rx, ry) = (pla.lutff_sites[8].0.x, pla.lutff_sites[8].0.y);
        let (sx, sy) = (pla.lutff_sites[0].0.x, pla.lutff_sites[0].0.y);
        let partial = bitgen_pblock(&dev, &rb, &[(rx, ry)]).unwrap();
        let mut cable = SimCable::open(&dev);
        cable.program(&full_a).unwrap();
        let st_maj = dev.clb_major(sx, sy).unwrap();
        let before = cable.fabric().frame_word(helion_device::Far::CLB_IO_CLK, st_maj, 0);
        cable.program_partial(&partial).unwrap();
        let after = cable.fabric().frame_word(helion_device::Far::CLB_IO_CLK, st_maj, 0);
        assert_eq!(before, after, "sim cable partial must not touch static frames");
        assert_eq!(
            cable.fabric().lut_init(rx, ry, 0),
            0xAAAA_AAAA_AAAA_AAAA
        );
        assert!(cable.stat().done);
    }

    #[test]
    fn detect_lists_sim_and_ofl_backends() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("HELION_OFL");
            std::env::remove_var("HELION_OPENFPGALOADER");
            std::env::remove_var("HELION_OFL_DRY_RUN");
            std::env::remove_var("HELION_OFL_BOARD");
        }
        let d = detect_boards();
        assert!(
            d.cables.iter().any(|c| c.backend == CableBackend::Sim),
            "sim must always be listed"
        );
        assert!(
            d.cables
                .iter()
                .any(|c| c.backend == CableBackend::OpenFpgaLoader),
            "ofl/usb backend must be advertised"
        );
        assert!(
            d.cables.iter().any(|c| c.backend == CableBackend::NativeUsb),
            "native USB stub must be advertised"
        );
        assert!(
            d.cables.iter().any(|c| c.backend == CableBackend::MpsseSim),
            "mpsse-sim CFG_W bitbang cable must be advertised"
        );
        assert!(d.text().contains("physical_had="));
        assert!(d.text().contains("had_board_ids"));
        assert!(d.text().contains("helion_hl10t"));
        assert!(d.text().contains("native_usb: feature="));
        assert!(d.text().contains("native_ftdi probes"));
        assert!(resolve_cable("sim").is_ok());
        assert!(resolve_cable("usb").is_ok());
        assert!(resolve_cable("ofl").is_ok());
        assert!(resolve_cable("native").is_ok());
        assert!(resolve_cable("auto").is_ok());
        assert!(resolve_cable("nope").is_err());
        // Without a real USB probe (typical CI), physical_had is false.
        if d.usb.probes.is_empty() {
            assert!(!d.physical_had);
            let auto = resolve_cable("auto").unwrap();
            // USB=0: auto stays on OFL so program refuses DONE (never invents sim DONE).
            assert_eq!(auto.backend, CableBackend::OpenFpgaLoader);
        }
    }

    #[test]
    fn cached_cable_resolve_does_not_reshell_ofl() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let n0 = usb_scan_invocations();
        let det = detect_boards();
        let n1 = usb_scan_invocations();
        assert!(n1 > n0, "detect_boards must scan USB/OFL once");
        let cable = resolve_cable_from("auto", &det).expect("auto cable");
        assert_eq!(
            usb_scan_invocations(),
            n1,
            "IDE paint path must not shell openFPGALoader again (got {})",
            usb_scan_invocations()
        );
        assert_eq!(cable.backend, CableBackend::OpenFpgaLoader);
    }

    #[test]
    fn backend_selection_auto_prefers_ofl_when_probes() {
        // Auto always OFL (USB=0 → honest program fail; probes → OFL program).
        let usb = resolve_cable("usb0").unwrap();
        assert_eq!(usb.backend, CableBackend::OpenFpgaLoader);
        assert_eq!(usb.id, "ofl0");
        let auto = resolve_cable("auto").unwrap();
        assert_eq!(auto.backend, CableBackend::OpenFpgaLoader);
    }

    #[test]
    fn auto_usb0_program_refuses_done_no_soft_hold() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("helion-auto-usb0");
        let _ = std::fs::create_dir_all(&dir);
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bits_path = dir.join("counter.hbits");
        std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
        // Ensure real OFL on PATH is used (or missing → still honest refuse).
        unsafe { std::env::remove_var("HELION_OPENFPGALOADER"); }
        unsafe { std::env::remove_var("HELION_OFL_DRY_RUN"); }
        let cable = resolve_cable("auto").unwrap();
        assert_eq!(cable.backend, CableBackend::OpenFpgaLoader);
        let err = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
        assert!(
            err.contains("no USB") || err.contains("openFPGALoader") || err.contains("programmer"),
            "USB=0 must honest-fail, got: {err}"
        );
        assert!(!err.contains("DONE=1"), "must not invent DONE: {err}");
        assert!(!err.to_ascii_lowercase().contains("soft-hold"), "must not soft-hold: {err}");
    }

    #[test]
    fn ofl_program_fails_honestly_without_device_or_binary() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("helion-ofl-test");
        let _ = std::fs::create_dir_all(&dir);
        let bits_path = dir.join("empty.hbits");
        // Minimal invalid so we fail before format if no ofl — use real empty bitstream when possible.
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let empty = Bitstream::empty(&dev);
        std::fs::write(&bits_path, &empty.packets).unwrap();

        // Point at a missing binary → PATH-style error (no DONE).
        unsafe { std::env::set_var("HELION_OPENFPGALOADER", dir.join("missing-openFPGALoader")); }
        let cable = resolve_cable("ofl").unwrap();
        let err = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
        assert!(
            err.contains("openFPGALoader") || err.contains("not a file") || err.contains("no USB"),
            "honest error, got: {err}"
        );
        unsafe { std::env::remove_var("HELION_OPENFPGALOADER"); }
    }

    #[test]
    fn ofl_dry_run_logs_and_refuses_done() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("helion-ofl-dry");
        let _ = std::fs::create_dir_all(&dir);
        let fake = dir.join("fake-ofl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Fake OFL: --scan-usb prints a probe line; program args echo ok.
            std::fs::write(
                &fake,
                "#!/bin/sh\nif [ \"$1\" = \"--scan-usb\" ]; then echo 'FTDI probe vid=0x0403 pid=0x6010'; exit 0; fi\necho ofl-ok; exit 0\n",
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        {
            // Skip dry-run spawn test on non-unix CI shapes.
            return;
        }
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bits_path = dir.join("t.hbits");
        std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
        unsafe { std::env::set_var("HELION_OPENFPGALOADER", &fake); }
        unsafe { std::env::set_var("HELION_OFL_DRY_RUN", "1"); }
        let cable = resolve_cable("usb").unwrap();
        assert_eq!(cable.backend, CableBackend::OpenFpgaLoader);
        let det = detect_boards();
        assert!(det.physical_had, "fake --scan-usb must enumerate a probe");
        let err = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
        assert!(err.contains("dry-run") || err.contains("HELION_OFL_DRY_RUN"), "{err}");
        unsafe { std::env::remove_var("HELION_OFL_DRY_RUN"); }
        // Real invoke with fake OFL that succeeds.
        let ok = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap();
        match ok {
            ProgramOutcome::OpenFpgaLoader { ofl, .. } => {
                assert_eq!(ofl.exit_code, Some(0));
                assert!(ofl.command.contains("fake-ofl") || ofl.command.contains("-m"));
            }
            ProgramOutcome::Sim { .. } | ProgramOutcome::MpsseSim { .. } | ProgramOutcome::NativeMpsse { .. } => panic!("expected ofl backend"),
        }
        unsafe { std::env::remove_var("HELION_OPENFPGALOADER"); }
    }

    #[test]
    fn program_packets_roundtrip_empty_hbits() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let empty = Bitstream::empty(&dev);
        let (bits, st) = program_packets(&dev, &empty.packets).unwrap();
        assert_eq!(bits.idcode, dev.idcode);
        assert!(st.done && st.gwe && !st.crc_err);
    }

    #[test]
    fn had_board_id_helpers_and_ofl_board_defaults() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let t = lookup_had_board("HL10T-C32-1").expect("T part");
        assert_eq!(t.idcode, 0x0001_1A1F);
        assert_eq!(t.ofl_board, "helion_hl10t");
        let dsp = lookup_had_board("HL10T-DSP1").expect("DSP part");
        assert_eq!(dsp.idcode, t.idcode);
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        assert_eq!(
            resolve_ofl_board(Some("HL10T-C32-1")).as_deref(),
            Some("helion_hl10t")
        );
        unsafe { std::env::set_var("HELION_OFL_BOARD", "none"); }
        assert_eq!(resolve_ofl_board(Some("HL10T-C32-1")), None);
        unsafe { std::env::set_var("HELION_OFL_BOARD", "custom_had"); }
        assert_eq!(
            resolve_ofl_board(Some("HL10T-C32-1")).as_deref(),
            Some("custom_had")
        );
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        let table = had_board_id_table_text();
        assert!(table.contains("HELION_OFL_BOARD"));
        assert!(table.contains("0x00011a1f") || table.contains("0x00011A1F"));
    }

    #[test]
    fn native_usb_stub_not_implemented_falls_back_to_ofl() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("helion-native-fallback");
        let _ = std::fs::create_dir_all(&dir);
        let fake = dir.join("fake-ofl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(
                &fake,
                "#!/bin/sh\nif [ \"$1\" = \"--scan-usb\" ]; then echo 'FTDI probe vid=0x0403 pid=0x6010'; exit 0; fi\necho ofl-ok; exit 0\n",
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        {
            return;
        }
        let err = try_native_usb_program(std::path::Path::new("/dev/null"), false).unwrap_err();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bits_path = dir.join("t.hbits");
        std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
        unsafe { std::env::set_var("HELION_OPENFPGALOADER", &fake); }
        unsafe { std::env::set_var("HELION_OFL_BOARD", "none"); } // keep cmd simple for assert
        let cable = resolve_cable("native").unwrap();
        assert_eq!(cable.backend, CableBackend::NativeUsb);
        if usb_native_feature_enabled() {
            // Feature on + no FTDI → honest Io (no OFL fallback, no invented STAT).
            assert!(matches!(err, NativeUsbError::Io(_)), "{err:?}");
            let e = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
            assert!(
                e.contains("native MPSSE") || e.contains("I/O") || e.contains("no FTDI"),
                "{e}"
            );
        } else {
            assert!(matches!(err, NativeUsbError::NotImplemented(_)), "{err:?}");
            let ok = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap();
            match ok {
                ProgramOutcome::OpenFpgaLoader { ofl, .. } => {
                    assert_eq!(ofl.exit_code, Some(0));
                    assert!(!ofl.tap_readback);
                    assert_eq!(ofl.readback, OflReadbackKind::None);
                    let line = ProgramOutcome::OpenFpgaLoader {
                        bits: None,
                        ofl: ofl.clone(),
                        bytes: 0,
                    }
                    .summary_line("program", &dev.part);
                    assert!(line.contains("no TAP readback"), "{line}");
                    assert!(line.contains("TAP_readback=none"), "{line}");
                    assert!(line.contains("STAT=(no readback)"), "{line}");
                }
                ProgramOutcome::Sim { .. } | ProgramOutcome::MpsseSim { .. } | ProgramOutcome::NativeMpsse { .. } => {
                    panic!("expected OFL fallback from native NotImplemented")
                }
            }
        }
        // Flash + HELION_OFL_VERIFY should request --verify (still not TAP readback).
        unsafe { std::env::set_var("HELION_OFL_VERIFY", "1"); }
        let ofl_cable = resolve_cable("ofl").unwrap();
        let flash_ok = program_hbits_with_cable(&dev, &bits_path, &ofl_cable, true).unwrap();
        match flash_ok {
            ProgramOutcome::OpenFpgaLoader { ofl, .. } => {
                assert!(ofl.command.contains("--verify"), "{}", ofl.command);
                assert_eq!(ofl.readback, OflReadbackKind::FlashSpiVerify);
                assert!(!ofl.tap_readback);
            }
            ProgramOutcome::Sim { .. } | ProgramOutcome::MpsseSim { .. } | ProgramOutcome::NativeMpsse { .. } => panic!("expected ofl"),
        }
        unsafe { std::env::remove_var("HELION_OFL_VERIFY"); }
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        unsafe { std::env::remove_var("HELION_OPENFPGALOADER"); }
    }

    #[test]
    fn parse_ofl_verify_output_honesty() {
        let (ok, detail) = parse_ofl_verify_output(
            "Write done\nVerify: OK\n",
            "",
            OflReadbackKind::FlashSpiVerify,
            true,
        );
        assert_eq!(ok, Some(true), "{detail}");
        assert!(detail.to_ascii_lowercase().contains("verify"), "{detail}");

        let (bad, detail) = parse_ofl_verify_output(
            "Verify failed: mismatch at 0x10\n",
            "",
            OflReadbackKind::FlashSpiVerify,
            true,
        );
        assert_eq!(bad, Some(false), "{detail}");

        let (none_v, detail) = parse_ofl_verify_output(
            "Done\n",
            "",
            OflReadbackKind::None,
            true,
        );
        assert_eq!(none_v, None, "{detail}");
        assert!(detail.contains("TAP_readback=none"), "{detail}");
    }

    #[test]
    fn native_ftdi_enumerate_detect_only_no_done_claim() {
        let scan = enumerate_ftdi();
        assert_eq!(scan.feature_enabled, usb_native_feature_enabled());
        let usb = scan_usb_probes();
        assert_eq!(usb.native_probes.len(), scan.probes.len());
        for p in &usb.native_probes {
            assert_eq!(p.source, UsbProbeSource::NativeRusb);
            assert!(p.detail.contains("detect-only"), "{}", p.detail);
            assert!(!p.detail.to_ascii_lowercase().contains("done=1"));
        }
        let det = detect_boards();
        assert!(det.text().contains("native_ftdi probes"));
        assert!(det.text().contains("native_note"));
        if !usb.native_probes.is_empty() {
            assert!(det.physical_had);
            assert!(
                det.note.contains("detect only") || det.note.contains("detect-only"),
                "{}",
                det.note
            );
        }
        let err = try_native_usb_program(std::path::Path::new("/dev/null"), false).unwrap_err();
        assert!(
            matches!(
                err,
                NativeUsbError::NotImplemented(_) | NativeUsbError::Io(_)
            ),
            "{err:?}"
        );
        if usb_native_feature_enabled() {
            assert!(matches!(err, NativeUsbError::Io(_)), "{err:?}");
        } else {
            assert!(matches!(err, NativeUsbError::NotImplemented(_)), "{err:?}");
        }
    }

    #[test]
    fn native_mpsse_opcode_path_and_honesty_gate() {
        // Opcode encoder exists without hardware.
        let ops = NativeFtdiMpsse::encode_read_stat();
        assert!(ops.contains(&MPSSE_SET_CLK_DIVISOR));
        assert!(ops.contains(&MPSSE_CLK_TMS_OUT_NEG_LSB));
        assert!(native_mpsse_status_note().contains("native_mpsse"));

        let err = try_native_usb_program(std::path::Path::new("/dev/null"), false).unwrap_err();
        if usb_native_feature_enabled() {
            assert!(
                matches!(err, NativeUsbError::Io(_)),
                "usb-native + no FTDI → Io, got {err:?}"
            );
            // Explicit --cable native must not invent STAT / soft-succeed.
            let dev = Device::load_part("HL10T-C32-1").unwrap();
            let dir = std::env::temp_dir().join("helion-native-mpsse-io");
            let _ = std::fs::create_dir_all(&dir);
            let bits_path = dir.join("t.hbits");
            std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
            let cable = resolve_cable("native").unwrap();
            let e = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
            assert!(
                e.contains("native MPSSE") || e.contains("I/O") || e.contains("no FTDI"),
                "{e}"
            );
            assert!(!e.to_ascii_lowercase().contains("done=1"));
        } else {
            assert!(
                matches!(err, NativeUsbError::NotImplemented(_)),
                "feature off → NotImplemented→OFL, got {err:?}"
            );
        }
        let table = had_board_id_table_text();
        assert!(table.contains("NativeFtdiMpsse") || table.contains("native_mpsse"));
        assert!(table.contains("TAP_readback=none"));
    }

    #[test]
    fn ofl_had_cable_notes_surface_board_alias() {
        let table = had_board_id_table_text();
        assert!(table.contains("helion_hl10t"));
        assert!(table.contains("HELION_OFL_BOARD"));
        assert!(table.contains("HELION_OFL_VERIFY") || table.contains("TAP_readback=none"));
        let det = detect_boards();
        assert!(det.text().contains("helion_hl10t"));
        assert!(det.text().contains("native_mpsse") || det.text().contains("NativeFtdiMpsse") || det.text().contains("native_usb"));
    }

    #[test]
    fn ofl_verify_parse_refuses_done_on_verify_fail_phrase() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("helion-ofl-verify-fail");
        let _ = std::fs::create_dir_all(&dir);
        let fake = dir.join("fake-ofl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(
                &fake,
                "#!/bin/sh\nif [ \"$1\" = \"--scan-usb\" ]; then echo 'FTDI probe vid=0x0403 pid=0x6010'; exit 0; fi\necho 'Verify failed: mismatch'; exit 0\n",
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        {
            return;
        }
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bits_path = dir.join("t.hbits");
        std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
        unsafe { std::env::set_var("HELION_OPENFPGALOADER", &fake); }
        unsafe { std::env::set_var("HELION_OFL_BOARD", "none"); }
        let cable = resolve_cable("ofl").unwrap();
        let err = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
        assert!(
            err.contains("verify") || err.contains("refusing DONE"),
            "{err}"
        );
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        unsafe { std::env::remove_var("HELION_OPENFPGALOADER"); }
    }

    /// Load fixture OFL logs from `fixtures/ofl/` — honest TAP_readback=none; never invent Helion STAT.
    fn load_ofl_fixture(name: &str) -> String {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("fixtures/ofl");
        p.push(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    #[test]
    fn ofl_fixture_logs_verify_and_tap_readback_none() {
        // SRAM Done → programmer success phrase, but TAP_readback=none / verify_ok=None
        let sram = load_ofl_fixture("sram_done_no_verify.txt");
        let (v, detail) = parse_ofl_verify_output(&sram, "", OflReadbackKind::None, true);
        assert_eq!(v, None, "{detail}");
        assert!(detail.contains("TAP_readback=none"), "{detail}");
        assert!(!detail.to_ascii_lowercase().contains("stat=0x"), "{detail}");

        let vok = load_ofl_fixture("flash_verify_ok.txt");
        let (v, detail) = parse_ofl_verify_output(&vok, "", OflReadbackKind::FlashSpiVerify, true);
        assert_eq!(v, Some(true), "{detail}");
        assert!(detail.to_ascii_lowercase().contains("verify"), "{detail}");

        let vfail = load_ofl_fixture("flash_verify_fail.txt");
        let (v, detail) = parse_ofl_verify_output(&vfail, "", OflReadbackKind::FlashSpiVerify, true);
        assert_eq!(v, Some(false), "{detail}");

        let generic = load_ofl_fixture("programmer_ok_generic.txt");
        let (v, detail) = parse_ofl_verify_output(&generic, "", OflReadbackKind::None, true);
        assert_eq!(v, None, "generic Done must not invent SPI verify or Helion STAT: {detail}");
        assert!(detail.contains("TAP_readback=none"), "{detail}");

        let crc = load_ofl_fixture("crc_error.txt");
        let (v, detail) = parse_ofl_verify_output(&crc, "", OflReadbackKind::None, false);
        assert!(v == Some(false) || v.is_none(), "{detail}");
        // Failure markers should win when present
        let (v2, d2) = parse_ofl_verify_output(&crc, "", OflReadbackKind::FlashSpiVerify, true);
        assert_eq!(v2, Some(false), "{d2}");

        // Dry-run fixture is documentation-only — parser still must not invent STAT
        let dry = load_ofl_fixture("dry_run_would_run.txt");
        let (v, detail) = parse_ofl_verify_output(&dry, "", OflReadbackKind::None, false);
        assert_eq!(v, None, "{detail}");
        assert!(!dry.to_ascii_lowercase().contains("done=1"));
        let _ = detail;
    }

    #[test]
    fn ofl_dry_run_env_still_refuses_done_with_fixture_binary() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join("helion-ofl-dry-fixture");
        let _ = std::fs::create_dir_all(&dir);
        let fake = dir.join("fake-ofl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(
                &fake,
                "#!/bin/sh\nif [ \"$1\" = \"--scan-usb\" ]; then echo 'FTDI probe vid=0x0403 pid=0x6010'; exit 0; fi\necho should-not-run; exit 0\n",
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        {
            return;
        }
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bits_path = dir.join("t.hbits");
        std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
        unsafe { std::env::set_var("HELION_OPENFPGALOADER", &fake); }
        unsafe { std::env::set_var("HELION_OFL_DRY_RUN", "1"); }
        unsafe { std::env::set_var("HELION_OFL_BOARD", "none"); }
        let cable = resolve_cable("ofl").unwrap();
        let err = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
        assert!(err.contains("dry-run") || err.contains("HELION_OFL_DRY_RUN"), "{err}");
        assert!(!err.to_ascii_lowercase().contains("done=1"));
        unsafe { std::env::remove_var("HELION_OFL_DRY_RUN"); }
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        unsafe { std::env::remove_var("HELION_OPENFPGALOADER"); }
    }

    #[test]
    fn ofl_scan_usb_empty_header_is_zero_probes() {
        // Real OFL 0.13.x empty output (Debian package) — header must not count as a probe.
        let raw = load_ofl_fixture("scan_usb_empty.txt");
        let probes = parse_scan_usb_output(&raw);
        assert!(
            probes.is_empty(),
            "header/empty chatter must not invent probes: {probes:?}"
        );
        let one = load_ofl_fixture("scan_usb_one_ftdi.txt");
        let probes = parse_scan_usb_output(&one);
        assert_eq!(probes.len(), 1, "{probes:?}");
        assert!(probes[0].detail.contains("0x0403"), "{probes:?}");
        assert_eq!(probes[0].source, UsbProbeSource::OpenFpgaLoader);
        // Fake line without 0x must still be ignored
        let junk = "Bus device vid:pid       probe type      manufacturer serial               product\n";
        assert!(parse_scan_usb_output(junk).is_empty());
    }

    /// When real `openFPGALoader` is on PATH (box apt install): detect sees binary, 0 probes, no DONE.
    #[test]
    fn ofl_real_binary_on_path_detect_zero_probes_no_done() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        // Clear overrides so PATH / system OFL is used when present.
        unsafe {
            std::env::remove_var("HELION_OPENFPGALOADER");
            std::env::remove_var("HELION_OFL");
            std::env::remove_var("HELION_OFL_DRY_RUN");
            std::env::remove_var("HELION_OFL_BOARD");
        }
        let Some(ofl) = find_openfpgaloader() else {
            // CI without OFL — skip (install is box-local).
            return;
        };
        assert!(ofl.is_file(), "{}", ofl.display());
        let scan = scan_usb_probes();
        assert_eq!(scan.ofl_path.as_deref(), Some(ofl.as_path()));
        let ofl_n = scan
            .probes
            .iter()
            .filter(|p| p.source == UsbProbeSource::OpenFpgaLoader)
            .count();
        // This Linux box has no FTDI; after header fix, OFL probes must be 0.
        // If a real probe appears in future CI, still never claim DONE from detect alone.
        let det = detect_boards();
        assert!(det.text().contains("ofl path") || det.text().contains("ofl probes"));
        assert!(
            !det.text().to_ascii_lowercase().contains("done=1"),
            "detect must not claim DONE: {}",
            det.text()
        );
        if ofl_n == 0 {
            assert!(!det.physical_had, "0 OFL probes → physical_had false");
            let note = scan.note.to_ascii_lowercase();
            assert!(
                note.contains("no usb") || note.contains("0 probe") || ofl_n == 0,
                "note={}",
                scan.note
            );
            let dev = Device::load_part("HL10T-C32-1").unwrap();
            let dir = std::env::temp_dir().join("helion-ofl-real-bin");
            let _ = std::fs::create_dir_all(&dir);
            let bits_path = dir.join("t.hbits");
            std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
            let cable = resolve_cable("ofl").unwrap();
            let err = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
            assert!(
                err.contains("no USB") || err.contains("programmer"),
                "honest refuse without probe: {err}"
            );
            assert!(!err.to_ascii_lowercase().contains("done=1"));
            // Dry-run with no probe still refuses before spawn (no DONE).
            unsafe { std::env::set_var("HELION_OFL_DRY_RUN", "1"); }
            let err2 = program_hbits_with_cable(&dev, &bits_path, &cable, false).unwrap_err();
            assert!(
                err2.contains("no USB")
                    || err2.contains("dry-run")
                    || err2.contains("HELION_OFL_DRY_RUN"),
                "{err2}"
            );
            unsafe { std::env::remove_var("HELION_OFL_DRY_RUN"); }
        }
    }

    #[test]
    fn ofl_fixture_scan_and_verify_never_invent_helion_stat() {
        let _doc = load_ofl_fixture("box_ofl_installed_no_probe.txt");
        assert!(_doc.contains("TAP_readback=none"));
        // Cross-check: SRAM/generic still TAP_readback=none
        for name in [
            "sram_done_no_verify.txt",
            "programmer_ok_generic.txt",
            "dry_run_would_run.txt",
        ] {
            let body = load_ofl_fixture(name);
            let (v, detail) = parse_ofl_verify_output(&body, "", OflReadbackKind::None, true);
            assert_eq!(v, None, "{name}: {detail}");
            if detail.contains("TAP_readback") {
                assert!(detail.contains("TAP_readback=none"), "{name}: {detail}");
            }
            assert!(!body.to_ascii_lowercase().contains("stat=0x"));
            assert!(!detail.to_ascii_lowercase().contains("stat=0x"));
        }
    }

    #[test]
    fn native_mpsse_and_ofl_honesty_coexist_on_box() {
        // usb-native on + 0 FTDI → Io; OFL on PATH + 0 probes → no USB; neither invents STAT.
        let native_err = try_native_usb_program(std::path::Path::new("/dev/null"), false);
        if usb_native_feature_enabled() {
            assert!(matches!(native_err, Err(NativeUsbError::Io(_))), "{native_err:?}");
        } else {
            assert!(matches!(native_err, Err(NativeUsbError::NotImplemented(_))), "{native_err:?}");
        }
        let det = detect_boards();
        assert!(det.text().contains("TAP_readback=none") || det.text().contains("never invent"));
        assert!(!det.text().to_ascii_lowercase().contains("done=1 from enumerate"));
        let _ = native_err;
    }

    #[test]
    fn had_board_lookup_and_ofl_board_resolve() {
        let _guard = OFL_ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        let row = lookup_had_board("hl10t-c32-1").expect("case-insensitive HAD lookup");
        assert_eq!(row.part, "HL10T-C32-1");
        assert_eq!(row.idcode, 0x0001_1A1F);
        assert_eq!(row.ofl_board, "helion_hl10t");
        assert_eq!(row.usb_vid, 0x0403);
        assert_eq!(row.usb_pid, 0x6010);
        assert!(lookup_had_board("no-such-part").is_none());
        assert_eq!(
            resolve_ofl_board(Some("HL10T-C32-1")).as_deref(),
            Some("helion_hl10t")
        );
        unsafe { std::env::set_var("HELION_OFL_BOARD", "none"); }
        assert_eq!(resolve_ofl_board(Some("HL10T-C32-1")), None);
        unsafe { std::env::set_var("HELION_OFL_BOARD", "custom_board"); }
        assert_eq!(
            resolve_ofl_board(Some("HL10T-C32-1")).as_deref(),
            Some("custom_board")
        );
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        let table = had_board_id_table_text();
        assert!(table.contains("helion_hl10t"));
        assert!(table.contains("TAP_readback=none"));
        assert!(!table.to_ascii_lowercase().contains("done=1 from table"));
    }

    #[test]
    fn tap_ir_len_and_tick_reset_path() {
        assert_eq!(Tap::IR_LEN, 6);
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut tap = Tap::new(&dev);
        assert_eq!(tap.state, TapState::TestLogicReset);
        for _ in 0..5 {
            let _ = tap.tick(true, false);
        }
        assert_eq!(tap.state, TapState::TestLogicReset);
        let _ = tap.tick(false, false);
        assert_eq!(tap.state, TapState::RunTestIdle);
        tap.reset();
        assert_eq!(tap.state, TapState::TestLogicReset);
        assert_eq!(tap.ir, IR_IDCODE);
        assert!(tap.cfg_last_err().is_none());
    }

    #[test]
    fn parse_scan_usb_dedups_and_skips_header_chatter() {
        let raw = concat!(
            "empty\n",
            "No USB devices found\n",
            "Bus device vid:pid       probe type      manufacturer serial               product\n",
            "FTDI 0x0403:0x6010 probe type FTDI\n",
            "FTDI 0x0403:0x6010 probe type FTDI\n",
            "noise without hex id probe\n",
        );
        let probes = parse_scan_usb_output(raw);
        assert_eq!(probes.len(), 1, "{probes:?}");
        assert!(probes[0].detail.contains("0x0403"));
        assert_eq!(probes[0].source, UsbProbeSource::OpenFpgaLoader);
        assert!(parse_scan_usb_output("Usage: openFPGALoader [options]\n").is_empty());
    }


}
