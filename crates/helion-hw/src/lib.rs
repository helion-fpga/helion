//! IEEE 1149.1 TAP + helion-prog + cable detect (sim + openFPGALoader USB).
//!
//! openFPGALoader-class usefulness for HAD: list/detect programming targets,
//! load a `.hbits` from the last implement, program over the sim cable
//! (TAP CFG_W) **or** spawn `openFPGALoader` when on PATH with a USB probe,
//! and report clear errors when nothing is attached / OFL missing.
//!
//! Native libusb/FTDI is scaffolded via [`HadUsbTransport`] + [`NativeFtdiStub`]
//! (honest `NotImplemented` → OFL fallback). HAD board IDs / `HELION_OFL_BOARD`
//! defaults live in [`HAD_KNOWN_BOARDS`]. OFL USB path does **not** provide
//! Helion TAP STAT readback (`--verify` is SPI-flash only) — summaries say so.
//! Never claims hardware DONE without a device. No UNISIM/AMD IP — HAD is Helion's story.

use helion_bits::Bitstream;
use helion_device::Device;
use helion_fabric::{Fabric, Stat};

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
    shlen: u8,
}

impl Tap {
    pub fn new(dev: &Device) -> Self {
        Self {
            state: TapState::TestLogicReset,
            ir: IR_IDCODE,
            fabric: Fabric::new(dev),
            ir_shift: 0,
            shlen: 0,
        }
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

/// Programming backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CableBackend {
    /// In-process TAP + fabric (helion-hw sim cable).
    Sim,
    /// External `openFPGALoader` on PATH (USB/JTAG). Active physical path today.
    OpenFpgaLoader,
    /// Future native libusb/FTDI path ([`HadUsbTransport`]). Stubbed → NotImplemented.
    NativeUsb,
}

impl CableBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            CableBackend::Sim => "sim",
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

/// Future Helion-native USB/JTAG transport (libusb + FTDI MPSSE class).
///
/// Full driver is intentionally out of scope for this gap; the stub returns
/// [`NativeUsbError::NotImplemented`] so program paths fall back to OFL.
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

/// Honest stub until a real libusb/FTDI backend lands.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFtdiStub;

impl HadUsbTransport for NativeFtdiStub {
    fn name(&self) -> &'static str {
        "native-ftdi-stub"
    }

    fn open_probe(&mut self) -> Result<(), NativeUsbError> {
        Err(NativeUsbError::NotImplemented(
            "libusb/FTDI HadUsbTransport not implemented yet; use --cable ofl|usb (openFPGALoader)              or --cable auto (OFL when probes present)",
        ))
    }

    fn program_hbits(
        &mut self,
        _path: &std::path::Path,
        _flash: bool,
    ) -> Result<(), NativeUsbError> {
        self.open_probe()
    }

    fn read_stat(&mut self) -> Result<Option<u32>, NativeUsbError> {
        Err(NativeUsbError::NotImplemented(
            "TAP STAT readback over native USB not implemented; OFL path also has no Helion TAP readback",
        ))
    }
}

/// Try the native FTDI stub. Always `NotImplemented` today — callers fall back to OFL.
pub fn try_native_usb_program(
    path: &std::path::Path,
    flash: bool,
) -> Result<(), NativeUsbError> {
    let mut t = NativeFtdiStub;
    t.open_probe()?;
    t.program_hbits(path, flash)
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
        note: "Helion-T bring-up part; OFL board alias is Helion-local (not upstream openFPGALoader yet)",
    },
    HadBoardId {
        part: "HL10T-DSP1",
        idcode: 0x0001_1A1F,
        ofl_board: "helion_hl10t",
        usb_vid: 0x0403,
        usb_pid: 0x6010,
        note: "Same IDCODE as HL10T-C32-1; DSP/MAC27 site variant",
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
        "docs: set HELION_OFL_BOARD=<ofl_board> to pass -b; HELION_OFL_BOARD=none disables -b;          unset → default ofl_board for known part. HELION_OFL_CABLE / HELION_OFL_EXTRA /          HELION_OFL_VERIFY (flash --verify) / HELION_OFL_DRY_RUN also apply.
",
    );
    for b in HAD_KNOWN_BOARDS {
        out.push_str(&format!(
            "had_board part={} idcode={:#010x} ofl_board={} usb_vid={:#06x} usb_pid={:#06x} — {}
",
            b.part, b.idcode, b.ofl_board, b.usb_vid, b.usb_pid, b.note
        ));
    }
    out.push_str(
        "native_usb: stub (HadUsbTransport/NativeFtdiStub) — NotImplemented; active USB path is OFL
",
    );
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

/// USB probe row from `openFPGALoader --scan-usb` (or empty when OFL missing / no device).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsbProbe {
    pub name: String,
    pub detail: String,
}

/// Result of probing for `openFPGALoader` + USB programmers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsbScan {
    pub ofl_path: Option<std::path::PathBuf>,
    pub probes: Vec<UsbProbe>,
    pub raw: String,
    pub note: String,
}

/// Result of board/cable detection (honest about missing physical HAD).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectReport {
    pub cables: Vec<CableInfo>,
    /// True when a USB programmer enumerates via openFPGALoader `--scan-usb`.
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
        out.push_str(&format!("ofl probes {}\n", self.usb.probes.len()));
        for p in &self.usb.probes {
            out.push_str(&format!("probe {} — {}\n", p.name, p.detail));
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

/// Run `openFPGALoader --scan-usb` when available. Never fabricates probes.
pub fn scan_usb_probes() -> UsbScan {
    let Some(ofl) = find_openfpgaloader() else {
        return UsbScan {
            ofl_path: None,
            probes: Vec::new(),
            raw: String::new(),
            note: "openFPGALoader not on PATH (set HELION_OPENFPGALOADER or install openFPGALoader)"
                .into(),
        };
    };
    if !ofl.is_file() {
        return UsbScan {
            ofl_path: Some(ofl.clone()),
            probes: Vec::new(),
            raw: String::new(),
            note: format!(
                "openFPGALoader path {} is not a file",
                ofl.display()
            ),
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
                raw,
                note,
            }
        }
        Err(e) => UsbScan {
            ofl_path: Some(ofl),
            probes: Vec::new(),
            raw: String::new(),
            note: format!("failed to spawn openFPGALoader: {e}"),
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
        // openFPGALoader --scan-usb lines typically mention FTDI/USB/vid/pid/probe/cable.
        let interesting = lower.contains("vid")
            || lower.contains("pid")
            || lower.contains("ftdi")
            || lower.contains("probe")
            || lower.contains("cable")
            || lower.contains("usb")
            || lower.contains("0x");
        let skip = lower.contains("no usb")
            || lower.contains("nothing")
            || lower.starts_with("usage")
            || lower.contains("not found");
        if interesting && !skip {
            let name = t.split_whitespace().next().unwrap_or("usb").to_string();
            probes.push(UsbProbe {
                name,
                detail: t.to_string(),
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

fn ofl_cable_info(scan: &UsbScan) -> CableInfo {
    let detail = if scan.ofl_path.is_none() {
        "openFPGALoader backend (binary not on PATH)".into()
    } else if scan.probes.is_empty() {
        format!(
            "openFPGALoader backend — no USB probe ({})",
            scan.note
        )
    } else {
        format!(
            "openFPGALoader backend — {} USB probe(s); {}",
            scan.probes.len(),
            scan.probes
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

fn native_cable_info() -> CableInfo {
    CableInfo {
        id: "native0".into(),
        backend: CableBackend::NativeUsb,
        part_hint: "HL10T-C32-1".into(),
        detail: "native libusb/FTDI stub (HadUsbTransport) — NotImplemented; OFL fallback is the active USB path"
            .into(),
    }
}

/// List programming cables: `sim0`, `ofl0`, and `native0` (stub).
pub fn list_cables() -> Vec<CableInfo> {
    let scan = scan_usb_probes();
    vec![
        sim_cable_info(),
        ofl_cable_info(&scan),
        native_cable_info(),
    ]
}

/// Detect programming targets. Sim always present; physical only when USB probes enumerate.
pub fn detect_boards() -> DetectReport {
    let usb = scan_usb_probes();
    let physical_had = !usb.probes.is_empty();
    let cables = vec![
        sim_cable_info(),
        ofl_cable_info(&usb),
        native_cable_info(),
    ];
    let note = if physical_had {
        format!(
            "Physical USB programmer present via openFPGALoader ({} probe(s)). Use --cable usb|ofl|auto.",
            usb.probes.len()
        )
    } else if usb.ofl_path.is_some() {
        format!(
            "No USB programmer attached. {} Use --cable sim, or attach HAD/JTAG and retry detect.",
            usb.note
        )
    } else {
        "openFPGALoader not on PATH — cannot probe USB. Install openFPGALoader (or set HELION_OPENFPGALOADER). Sim cable remains available (--cable sim).".into()
    };
    DetectReport {
        cables,
        physical_had,
        note,
        usb,
    }
}

/// Resolve `--cable auto|sim|usb|ofl|native|sim0|ofl0|usb0|native0`.
pub fn resolve_cable(spec: &str) -> Result<CableInfo, String> {
    let s = spec.trim().to_ascii_lowercase();
    let det = detect_boards();
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
        .unwrap_or_else(native_cable_info);
    match s.as_str() {
        "" | "sim" | "sim0" => Ok(sim),
        "usb" | "usb0" | "ofl" | "ofl0" | "openfpgaloader" => Ok(ofl),
        "native" | "native0" | "ftdi" | "libusb" => Ok(native),
        "auto" => {
            if det.physical_had {
                Ok(ofl)
            } else {
                Ok(sim)
            }
        }
        other => Err(format!(
            "unknown cable {other:?}: use --cable auto|sim|usb|ofl|native"
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
    Ok(OflProgramReport {
        command,
        dry_run: false,
        exit_code: code,
        stdout,
        stderr,
        board,
        tap_readback: false,
        readback,
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
        CableBackend::NativeUsb => {
            // Honest stub: try native, then fall back to OFL when NotImplemented.
            match try_native_usb_program(path, flash) {
                Ok(()) => Err(
                    "program: native USB stub reported success without a driver — refusing DONE"
                        .into(),
                ),
                Err(NativeUsbError::NotImplemented(msg)) => {
                    eprintln!("program: native USB stub → {msg}; falling back to openFPGALoader");
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
                Err(NativeUsbError::Io(msg)) => Err(format!("program: native USB I/O: {msg}")),
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
            ProgramOutcome::OpenFpgaLoader { .. } => CableBackend::OpenFpgaLoader,
        }
    }

    /// Human summary line for CLI / GUI. Claims DONE only for sim TAP or OFL exit 0.
    /// OFL path never invents Helion TAP STAT bits — reports `TAP_readback=none`.
    pub fn summary_line(&self, sub: &str, part: &str) -> String {
        match self {
            ProgramOutcome::Sim { bits, stat } => format!(
                "hw {sub} backend=sim part={part} frames={} bytes={} STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
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
            ProgramOutcome::OpenFpgaLoader { bits, ofl, bytes } => {
                let frames = bits.as_ref().map(|b| b.frames.len()).unwrap_or(0);
                let board = ofl
                    .board
                    .as_deref()
                    .unwrap_or("-");
                format!(
                    "hw {sub} backend=ofl part={part} frames={frames} bytes={bytes} ofl_board={board} ofl_exit={} DONE=1 (programmer ok; no TAP readback) TAP_readback=none ofl_verify={} STAT=(no readback) cmd={}",
                    ofl.exit_code.unwrap_or(0),
                    ofl.readback.as_str(),
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
        assert!(d.text().contains("physical_had="));
        assert!(d.text().contains("had_board_ids"));
        assert!(d.text().contains("helion_hl10t"));
        assert!(d.text().contains("native_usb: stub"));
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
            assert_eq!(auto.backend, CableBackend::Sim);
        }
    }

    #[test]
    fn backend_selection_auto_prefers_ofl_when_probes() {
        // Auto without probes → sim (covered above). With empty scan, usb still resolves to ofl backend.
        let usb = resolve_cable("usb0").unwrap();
        assert_eq!(usb.backend, CableBackend::OpenFpgaLoader);
        assert_eq!(usb.id, "ofl0");
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
            ProgramOutcome::Sim { .. } => panic!("expected ofl backend"),
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
        assert!(matches!(err, NativeUsbError::NotImplemented(_)), "{err:?}");
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bits_path = dir.join("t.hbits");
        std::fs::write(&bits_path, &Bitstream::empty(&dev).packets).unwrap();
        unsafe { std::env::set_var("HELION_OPENFPGALOADER", &fake); }
        unsafe { std::env::set_var("HELION_OFL_BOARD", "none"); } // keep cmd simple for assert
        let cable = resolve_cable("native").unwrap();
        assert_eq!(cable.backend, CableBackend::NativeUsb);
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
            ProgramOutcome::Sim { .. } => panic!("expected OFL fallback from native stub"),
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
            ProgramOutcome::Sim { .. } => panic!("expected ofl"),
        }
        unsafe { std::env::remove_var("HELION_OFL_VERIFY"); }
        unsafe { std::env::remove_var("HELION_OFL_BOARD"); }
        unsafe { std::env::remove_var("HELION_OPENFPGALOADER"); }
    }

}
