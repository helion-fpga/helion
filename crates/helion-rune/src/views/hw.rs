//! Hardware Manager model: cable resolve, USB detect, Program Device, and
//! STAT readback over the real `helion-hw` backends (the view lives in
//! [`crate::views::program`]).
//!
//! - Cable specs are exactly the old IDE's (`helion-ide.rs:2092`
//!   `auto|sim|usb|native`), resolved with [`helion_hw::resolve_cable_from`]
//!   against the last scan.
//! - Detect runs [`helion_hw::detect_boards`] (it may shell
//!   `openFPGALoader --scan-usb`) on a worker thread.
//! - Program Device runs the Tcl `program_hw` path
//!   (`helion_proj::Session::program_hw_cable`) on a worker thread with real
//!   stage progress; a board DONE is never claimed without a board readback.
//! - STAT reads the Helion TAP (`helion_hw::Tap::read_idcode/read_stat`) or
//!   the programmed sim fabric (`helion_hw::prog_sim`), mirroring
//!   `ide.rs:24842 hw_stat_report`; every read names its [`StatSource`].
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/tooling.rs`: `AppSpec::poll` drains the worker.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use crate::state::{AppState, ConsoleLine, MsgSev, ToastKind};
use crate::views::cad;

/// Program stages streamed by the worker (device, bitstream, program, STAT).
pub const STAGES: u8 = 4;

/// Cable pick, exactly the old IDE's specs (`helion-ide.rs:2092`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CableChoice {
    #[default]
    Auto,
    Sim,
    Usb,
    Native,
}

impl CableChoice {
    pub const ALL: [CableChoice; 4] = [
        CableChoice::Auto,
        CableChoice::Sim,
        CableChoice::Usb,
        CableChoice::Native,
    ];

    /// `helion_hw::resolve_cable_from` spec.
    pub fn spec(self) -> &'static str {
        match self {
            CableChoice::Auto => "auto",
            CableChoice::Sim => "sim",
            CableChoice::Usb => "usb",
            CableChoice::Native => "native",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CableChoice::Auto => "Automatic",
            CableChoice::Sim => "Simulation",
            CableChoice::Usb => "USB / openFPGALoader",
            CableChoice::Native => "Native FTDI",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            CableChoice::Auto => "board path",
            CableChoice::Sim => "in-process fabric",
            CableChoice::Usb => "USB / JTAG",
            CableChoice::Native => "FTDI MPSSE",
        }
    }

    /// Board cables need an attached USB programmer (old `cable_needs_phys`).
    pub fn needs_usb(self) -> bool {
        !matches!(self, CableChoice::Sim)
    }

    pub fn parse(spec: &str) -> Option<Self> {
        match spec.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(CableChoice::Auto),
            "sim" | "sim0" => Some(CableChoice::Sim),
            "usb" | "usb0" | "ofl" | "ofl0" | "openfpgaloader" => Some(CableChoice::Usb),
            "native" | "native0" | "ftdi" | "libusb" => Some(CableChoice::Native),
            _ => None,
        }
    }
}

/// Program-run phase. A real enum, never stringly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgramPhase {
    #[default]
    Idle,
    Programming,
    Done,
    Failed,
}

/// Which worker job is in flight (one at a time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwJobKind {
    Detect,
    Program,
    Stat,
}

impl HwJobKind {
    pub fn busy_label(self) -> &'static str {
        match self {
            HwJobKind::Detect => "Scanning USB…",
            HwJobKind::Program => "Programming…",
            HwJobKind::Stat => "Reading STAT…",
        }
    }
}

/// Where a STAT word came from. Shown on the pane so a sim read is never
/// mistaken for a board read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatSource {
    /// Unconfigured Helion TAP model (`Tap::read_stat` on a fresh fabric).
    SimTap,
    /// Sim fabric after programming (`prog_sim` / `prog_mpsse_sim`).
    SimProgrammed,
    /// Live TDO from a native FTDI session (board STAT).
    BoardTdo,
}

impl StatSource {
    pub fn label(self) -> &'static str {
        match self {
            StatSource::SimTap => "Simulated TAP · unconfigured",
            StatSource::SimProgrammed => "Simulated fabric · after program",
            StatSource::BoardTdo => "Board · live TDO",
        }
    }
}

/// One STAT bit (`helion_fabric::StatBit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatRow {
    pub bit: u8,
    pub name: &'static str,
    pub value: bool,
}

impl StatRow {
    /// UG893-style card description (old `helion-ide.rs:13549`).
    pub fn description(&self) -> &'static str {
        match self.name {
            "CRC_ERR" => "Bitstream CRC integrity check",
            "INIT" => "Configuration memory initialized",
            "GTS" => "Global 3-state I/O tree",
            "GSR" => "Global Set/Reset tree",
            "GWE" => "Global Write Enable",
            "DONE" => "Fabric configuration complete",
            "EOS" => "End of Startup sequence",
            _ => "Status register bit",
        }
    }
}

/// TAP IDCODE + STAT word, mirroring `ide.rs::HwStatReport`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwStat {
    pub source: StatSource,
    pub part: String,
    pub idcode: u32,
    pub ir: u8,
    pub word: u32,
    pub bits: Vec<StatRow>,
}

impl HwStat {
    fn from_stat(
        st: &helion_fabric::Stat,
        source: StatSource,
        part: &str,
        idcode: u32,
        ir: u8,
    ) -> Self {
        Self {
            source,
            part: part.to_string(),
            idcode,
            ir,
            word: st.word(),
            bits: st
                .bits()
                .iter()
                .map(|b| StatRow {
                    bit: b.bit,
                    name: b.name,
                    value: b.value,
                })
                .collect(),
        }
    }

    fn from_word(word: u32, source: StatSource, part: &str, idcode: u32) -> Self {
        use helion_fabric::Stat;
        let on = |b: u8| (word >> b) & 1 != 0;
        let st = Stat {
            init: on(Stat::BIT_INIT),
            done: on(Stat::BIT_DONE),
            eos: on(Stat::BIT_EOS),
            crc_err: on(Stat::BIT_CRC_ERR),
            gwe: on(Stat::BIT_GWE),
            gsr: on(Stat::BIT_GSR),
            gts: on(Stat::BIT_GTS),
        };
        Self::from_stat(&st, source, part, idcode, helion_hw::IR_STAT)
    }

    pub fn bit(&self, spec: &str) -> Option<&StatRow> {
        let spec = spec.trim();
        if spec.is_empty() {
            return self.bits.iter().find(|b| b.name == "DONE");
        }
        if let Ok(i) = spec.parse::<u8>() {
            return self.bits.iter().find(|b| b.bit == i);
        }
        self.bits.iter().find(|b| b.name.eq_ignore_ascii_case(spec))
    }

    pub fn done(&self) -> bool {
        self.bits.iter().any(|b| b.name == "DONE" && b.value)
    }

    /// Console dump (old `HwStatReport::text`).
    pub fn text(&self) -> String {
        let mut s = format!(
            "hw_stat source={} part={} idcode={:#010x} ir={:#04x} word={:#010x}",
            self.source.label(),
            self.part,
            self.idcode,
            self.ir,
            self.word
        );
        for b in &self.bits {
            s.push_str(&format!(
                "\nBIT={} NAME={} VALUE={} DESC={}",
                b.bit,
                b.name,
                u8::from(b.value),
                b.description()
            ));
        }
        s
    }
}

/// Outcome of one Program Device run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramResult {
    pub ok: bool,
    /// `sim`, `mpsse-sim`, `ofl`, `native-mpsse` (empty when unknown).
    pub backend: String,
    /// Engine summary line (or the error).
    pub line: String,
    pub frames: usize,
    pub bytes: usize,
}

impl ProgramResult {
    /// One-line verdict that never claims a board DONE the engine did not read.
    pub fn verdict(&self, stat: Option<&HwStat>) -> String {
        if !self.ok {
            return "Programming failed".into();
        }
        match self.backend.as_str() {
            "sim" | "mpsse-sim" => {
                let done = stat.map(|s| u8::from(s.done())).unwrap_or(0);
                format!("Simulated fabric programmed · DONE={done} (not a physical board)")
            }
            "ofl" => "openFPGALoader reported success · DONE not read back".into(),
            "native-mpsse" => {
                let done = stat.map(|s| u8::from(s.done())).unwrap_or(0);
                format!("Board programmed · DONE={done} from live STAT")
            }
            _ => "Program finished".into(),
        }
    }
}

/// Which bitstream Program Device writes.
#[derive(Clone, Debug)]
pub enum BitsSource {
    /// PROGRAM.FILE picked with Browse (`.hbits` packet stream).
    File(PathBuf),
    /// In-memory artifact (last implementation run or Tcl session).
    Memory(Arc<helion_bits::Bitstream>),
}

/// Inputs for one program job (moved onto the worker).
#[derive(Clone, Debug)]
pub struct ProgramJob {
    pub cable: String,
    pub part: String,
    pub bits: BitsSource,
}

/// Worker -> UI message.
#[derive(Debug)]
pub enum HwEvent {
    Line { text: String, sev: MsgSev },
    Stage { step: u8, label: String },
    Detected(Box<helion_hw::DetectReport>),
    Programmed(Result<(ProgramResult, Result<HwStat, String>), ProgramResult>),
    Stat(Result<HwStat, String>),
}

#[derive(Clone)]
struct HwJob {
    kind: HwJobKind,
    rx: Arc<Mutex<mpsc::Receiver<HwEvent>>>,
}

/// Hardware Manager state: cable pick, last scan, program run, STAT read.
#[derive(Clone, Default)]
pub struct ProgramState {
    pub cable: CableChoice,
    /// Last `detect_boards` result (`None` = never scanned).
    pub detect: Option<helion_hw::DetectReport>,
    /// PROGRAM.FILE picked with Browse (`None` = use the in-memory artifact).
    pub file: Option<PathBuf>,
    pub phase: ProgramPhase,
    /// Completed program stage (0..=[`STAGES`]) and its label.
    pub stage: u8,
    pub stage_label: String,
    pub result: Option<ProgramResult>,
    /// Backend of the last successful program (`sim`, `ofl`, ...).
    pub programmed_backend: Option<String>,
    pub stat: Option<HwStat>,
    /// Why the last STAT read produced no word (shown instead of cards).
    pub stat_note: Option<String>,
    pub stat_selected: Option<&'static str>,
    /// Hardware tree selection (drives which panel shows).
    pub node: crate::views::hw_tree::HwNode,
    /// Set Up Debug + the programmed debug target (ILA / VIO / AXI).
    pub debug: crate::views::hw_debug::HwDebugState,
    /// Configuration memory device under the hardware device.
    pub cfgmem: crate::views::hw_cfgmem::CfgMemState,
    /// Native FTDI cable: TCK, scanned chain, raw scan log.
    pub native: crate::views::hw_native::NativeState,
    job: Option<HwJob>,
}

impl std::fmt::Debug for ProgramState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramState")
            .field("cable", &self.cable)
            .field("scanned", &self.detect.is_some())
            .field("file", &self.file)
            .field("phase", &self.phase)
            .field("stage", &self.stage)
            .field("result", &self.result)
            .field("stat", &self.stat)
            .field("job", &self.job.as_ref().map(|j| j.kind))
            .finish()
    }
}

impl ProgramState {
    pub fn busy(&self) -> Option<HwJobKind> {
        self.job.as_ref().map(|j| j.kind)
    }

    pub fn usb_count(&self) -> usize {
        self.detect
            .as_ref()
            .map(|d| d.usb.probes.len())
            .unwrap_or(0)
    }

    pub fn physical(&self) -> bool {
        self.detect.as_ref().is_some_and(|d| d.physical_had)
    }

    /// Resolved cable for the current pick, once a scan exists (Simulation
    /// resolves without one: it never touches USB).
    pub fn resolved(&self) -> Option<helion_hw::CableInfo> {
        match &self.detect {
            Some(d) => helion_hw::resolve_cable_from(self.cable.spec(), d).ok(),
            None if self.cable == CableChoice::Sim => {
                helion_hw::resolve_cable_from("sim", &unscanned_report()).ok()
            }
            None => None,
        }
    }

    /// Reset per-project results (cable pick and last scan stay).
    pub fn reset_results(&mut self) {
        // A new project invalidates the debug build, the programmed debug
        // target and any flash image (unless a worker still owns them).
        if self.debug.busy().is_none() {
            self.debug.reset();
        }
        if self.cfgmem.busy().is_none() {
            self.cfgmem = Default::default();
        }
        self.node = Default::default();
        self.phase = ProgramPhase::Idle;
        self.stage = 0;
        self.stage_label.clear();
        self.result = None;
        self.programmed_backend = None;
        self.stat = None;
        self.stat_note = None;
        self.stat_selected = None;
        self.file = None;
    }
}

/// An empty scan used only to resolve the always-present sim cable.
pub fn unscanned_report() -> helion_hw::DetectReport {
    helion_hw::DetectReport {
        cables: Vec::new(),
        physical_had: false,
        note: String::new(),
        usb: helion_hw::UsbScan {
            ofl_path: None,
            probes: Vec::new(),
            native_probes: Vec::new(),
            raw: String::new(),
            note: String::new(),
            native_note: String::new(),
        },
    }
}

/// `scan USB=N · OFL=path · physical HAD=0|1` (old `program_status` line).
pub fn scan_line(d: &helion_hw::DetectReport) -> String {
    let ofl = d
        .usb
        .ofl_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(not on PATH)".into());
    format!(
        "USB={} · OFL={} · physical HAD={}",
        d.usb.probes.len(),
        ofl,
        u8::from(d.physical_had)
    )
}

/// Parse `backend=xxx` from a `program_hw` summary line.
pub fn backend_of(line: &str) -> String {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix("backend="))
        .unwrap_or("")
        .to_string()
}

fn console(s: &mut AppState, text: impl Into<String>, sev: MsgSev) {
    s.console.lines.push(ConsoleLine {
        text: text.into(),
        sev,
    });
}

/// The bitstream Program Device would write, with a one-line label.
pub fn bits_source(s: &AppState) -> Option<(BitsSource, String)> {
    if let Some(p) = &s.program.file {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string());
        return Some((BitsSource::File(p.clone()), format!("File · {name}")));
    }
    if let Some(b) = &s.bitstream.bits {
        let label = format!(
            "{} · {} frames · {} B",
            if s.bitstream.source.is_empty() {
                "impl_1"
            } else {
                s.bitstream.source.as_str()
            },
            cad::grouped(s.bitstream.frames),
            cad::grouped(s.bitstream.bytes)
        );
        return Some((BitsSource::Memory(b.clone()), label));
    }
    if let Some(b) = &s.tcl.session.bitstream {
        let label = format!(
            "Tcl session · {} frames · {} B",
            cad::grouped(b.frames.len()),
            cad::grouped(b.packets.len())
        );
        return Some((BitsSource::Memory(Arc::new(b.clone())), label));
    }
    None
}

/// Part to program: the bitstream's own part when known, else the project.
pub fn program_part(s: &AppState) -> String {
    if s.program.file.is_none() && !s.bitstream.part.is_empty() && s.bitstream.bits.is_some() {
        return s.bitstream.part.clone();
    }
    s.project.part.clone()
}

/// Why Program Device is unavailable right now (`None` = enabled).
pub fn program_block_reason(s: &AppState) -> Option<String> {
    if let Some(kind) = s.program.busy() {
        return Some(format!("Busy: {}", kind.busy_label()));
    }
    if let Some(why) = crate::jobs::program_busy(s) {
        return Some(format!("Busy: {why}"));
    }
    if bits_source(s).is_none() {
        return Some("No bitstream yet. Generate Bitstream first.".into());
    }
    if s.program.cable.needs_usb() {
        match &s.program.detect {
            None => {
                return Some("USB not scanned. Run Detect, or use the Simulation cable.".into());
            }
            Some(d) if !d.physical_had => {
                return Some(
                    "No USB programmer (USB=0). Use Simulation for fabric-only programming.".into(),
                );
            }
            Some(_) => {}
        }
    }
    None
}

/// Why Refresh STAT is unavailable (`None` = enabled).
pub fn stat_block_reason(s: &AppState) -> Option<String> {
    if let Some(kind) = s.program.busy() {
        return Some(format!("Busy: {}", kind.busy_label()));
    }
    let sim_programmed = matches!(
        s.program.programmed_backend.as_deref(),
        Some("sim") | Some("mpsse-sim")
    );
    if s.program.cable.needs_usb() && !sim_programmed {
        return Some(
            "STAT readback runs on the Simulation cable; openFPGALoader has no TAP readback."
                .into(),
        );
    }
    None
}

// ---- blocking cores (worker thread, Tcl verbs, scenes) ----

/// Read IDCODE + STAT: the programmed sim fabric when the last program used
/// a sim backend (and a bitstream exists), else the unconfigured TAP model.
pub fn read_stat_blocking(
    part: &str,
    bits: Option<&helion_bits::Bitstream>,
    programmed_backend: Option<&str>,
) -> Result<HwStat, String> {
    let dev = helion_device::Device::load_part(part)
        .map_err(|e| format!("report_hw_stat: HAD {part}: {e}"))?;
    match (programmed_backend, bits) {
        (Some("sim"), Some(b)) => {
            let st = helion_hw::prog_sim(&dev, b)?;
            Ok(HwStat::from_stat(
                &st,
                StatSource::SimProgrammed,
                &dev.part,
                dev.idcode,
                helion_hw::IR_STAT,
            ))
        }
        (Some("mpsse-sim"), Some(b)) => {
            let st = helion_hw::prog_mpsse_sim(&dev, b)?;
            Ok(HwStat::from_stat(
                &st,
                StatSource::SimProgrammed,
                &dev.part,
                dev.idcode,
                helion_hw::IR_STAT,
            ))
        }
        _ => {
            let mut tap = helion_hw::Tap::new(&dev);
            let idcode = tap.read_idcode();
            let st = tap.read_stat();
            Ok(HwStat::from_stat(
                &st,
                StatSource::SimTap,
                &dev.part,
                idcode,
                tap.ir,
            ))
        }
    }
}

/// Program Device body: load the HAD model and bitstream, run
/// `Session::program_hw_cable` (the Tcl `program_hw` path), then read STAT
/// where the backend allows it. Emits real stage progress.
pub fn program_blocking(job: &ProgramJob, emit: &dyn Fn(HwEvent)) -> HwEvent {
    let stage = |step: u8, label: String| emit(HwEvent::Stage { step, label });
    let fail = |line: String| {
        HwEvent::Programmed(Err(ProgramResult {
            ok: false,
            backend: String::new(),
            line,
            frames: 0,
            bytes: 0,
        }))
    };
    stage(1, format!("Loading device {}", job.part));
    let dev = match helion_device::Device::load_part(&job.part) {
        Ok(d) => d,
        Err(e) => return fail(format!("program_hw: HAD {}: {e}", job.part)),
    };
    stage(2, "Loading bitstream".into());
    let bits: helion_bits::Bitstream = match &job.bits {
        BitsSource::Memory(b) => (**b).clone(),
        BitsSource::File(p) => {
            let bytes = match std::fs::read(p) {
                Ok(b) => b,
                Err(e) => return fail(format!("program_hw: read {}: {e}", p.display())),
            };
            match helion_bits::Bitstream::from_packets(&bytes) {
                Ok(b) => b,
                Err(e) => {
                    return fail(format!(
                        "program_hw: {} is not a Helion .hbits packet stream: {e}",
                        p.display()
                    ));
                }
            }
        }
    };
    let (frames, bytes) = (bits.frames.len(), bits.packets.len());
    emit(HwEvent::Line {
        text: format!("program_hw bitstream frames={frames} bytes={bytes}"),
        sev: MsgSev::Info,
    });
    stage(3, format!("Programming via cable={}", job.cable));
    let mut session = helion_proj::Session::new(helion_proj::Mode::Project);
    session.bitstream = Some(bits.clone());
    session.open_hw_manager();
    let line = match session.program_hw_cable(&dev, &job.cable) {
        Ok(line) => line,
        Err(e) => {
            return HwEvent::Programmed(Err(ProgramResult {
                ok: false,
                backend: backend_of(&e),
                line: e,
                frames,
                bytes,
            }));
        }
    };
    emit(HwEvent::Line {
        text: line.clone(),
        sev: MsgSev::Info,
    });
    let backend = backend_of(&line);
    stage(4, "Reading STAT".into());
    let stat = match backend.as_str() {
        "sim" | "mpsse-sim" => read_stat_blocking(&dev.part, Some(&bits), Some(&backend)),
        "native-mpsse" => line
            .split_whitespace()
            .find_map(|w| w.strip_prefix("STAT=0x"))
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .map(|w| HwStat::from_word(w, StatSource::BoardTdo, &dev.part, dev.idcode))
            .ok_or_else(|| "native STAT word missing from the program result".to_string()),
        _ => Err("openFPGALoader has no TAP readback: STAT and DONE are not read back.".into()),
    };
    HwEvent::Programmed(Ok((
        ProgramResult {
            ok: true,
            backend,
            line,
            frames,
            bytes,
        },
        stat,
    )))
}

// ---- worker plumbing ----

fn spawn_job(
    s: &mut AppState,
    kind: HwJobKind,
    body: impl FnOnce(&mpsc::Sender<HwEvent>) + Send + 'static,
) -> bool {
    if s.program.job.is_some() {
        return false;
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(format!("helion-hw-{kind:?}").to_lowercase())
        .spawn(move || body(&tx));
    match spawned {
        Ok(_) => {
            s.program.job = Some(HwJob {
                kind,
                rx: Arc::new(Mutex::new(rx)),
            });
            true
        }
        Err(e) => {
            console(s, format!("ERROR: hardware worker: {e}"), MsgSev::Error);
            false
        }
    }
}

/// Detect: `helion_hw::detect_boards` off the UI thread.
pub fn detect(s: &mut AppState) {
    console(s, "detect_hw_cables", MsgSev::Info);
    spawn_job(s, HwJobKind::Detect, |tx| {
        let report = helion_hw::detect_boards();
        let _ = tx.send(HwEvent::Detected(Box::new(report)));
    });
}

/// Program Device: worker run of [`program_blocking`]. No-op (with the
/// reason on the console) when [`program_block_reason`] blocks it.
pub fn program(s: &mut AppState) {
    if let Some(why) = program_block_reason(s) {
        console(s, format!("ERROR: program_hw: {why}"), MsgSev::Error);
        return;
    }
    let Some((bits, _)) = bits_source(s) else {
        return;
    };
    let job = ProgramJob {
        cable: s.program.cable.spec().to_string(),
        part: program_part(s),
        bits,
    };
    console(s, format!("program_hw cable={}", job.cable), MsgSev::Info);
    let started = spawn_job(s, HwJobKind::Program, move |tx| {
        let emit = |ev: HwEvent| {
            let _ = tx.send(ev);
        };
        let done = program_blocking(&job, &emit);
        let _ = tx.send(done);
    });
    if started {
        s.program.phase = ProgramPhase::Programming;
        s.program.stage = 0;
        s.program.stage_label = "Starting".into();
        s.program.result = None;
    }
}

/// Refresh STAT: worker run of [`read_stat_blocking`].
pub fn refresh_stat(s: &mut AppState) {
    if let Some(why) = stat_block_reason(s) {
        console(s, format!("ERROR: report_hw_stat: {why}"), MsgSev::Error);
        return;
    }
    let part = program_part(s);
    let bits = bits_source(s).and_then(|(b, _)| match b {
        BitsSource::Memory(m) => Some(m),
        BitsSource::File(_) => None,
    });
    let backend = s.program.programmed_backend.clone();
    console(s, "report_hw_stat", MsgSev::Info);
    spawn_job(s, HwJobKind::Stat, move |tx| {
        let r = read_stat_blocking(&part, bits.as_deref(), backend.as_deref());
        let _ = tx.send(HwEvent::Stat(r));
    });
}

/// Synchronous STAT read for the Tcl `report_hw_stat` verb.
pub fn report_hw_stat(s: &mut AppState) -> Result<String, String> {
    if let Some(why) = stat_block_reason(s) {
        return Err(format!("report_hw_stat: {why}"));
    }
    let part = program_part(s);
    let bits = bits_source(s).and_then(|(b, _)| match b {
        BitsSource::Memory(m) => Some(m),
        BitsSource::File(_) => None,
    });
    let r = read_stat_blocking(
        &part,
        bits.as_deref(),
        s.program.programmed_backend.as_deref(),
    )?;
    let text = r.text();
    s.program.stat = Some(r);
    s.program.stat_note = None;
    Ok(text)
}

/// Click a STAT card / Tcl `select_hw_stat NAME|BIT`.
pub fn select_hw_stat(s: &mut AppState, spec: &str) -> Result<String, String> {
    let stat = s
        .program
        .stat
        .as_ref()
        .ok_or("select_hw_stat: no STAT read yet (report_hw_stat first)")?;
    let row = *stat
        .bit(spec)
        .ok_or_else(|| format!("select_hw_stat: no bit {spec}"))?;
    s.program.stat_selected = Some(row.name);
    Ok(format!(
        "hw_stat NAME={} BIT={} VALUE={} DESC={}",
        row.name,
        row.bit,
        u8::from(row.value),
        row.description()
    ))
}

/// Mirror a Tcl `program_hw` result into the pane.
pub fn note_tcl_program(s: &mut AppState, result: &Result<String, String>) {
    let (ok, line) = match result {
        Ok(l) => (true, l.clone()),
        Err(e) => (false, e.clone()),
    };
    let backend = backend_of(&line);
    if ok {
        s.program.programmed_backend = Some(backend.clone());
    }
    s.program.phase = if ok {
        ProgramPhase::Done
    } else {
        ProgramPhase::Failed
    };
    s.program.stage = if ok { STAGES } else { 0 };
    s.program.result = Some(ProgramResult {
        ok,
        backend,
        line,
        frames: s
            .tcl
            .session
            .bitstream
            .as_ref()
            .map(|b| b.frames.len())
            .unwrap_or(0),
        bytes: s
            .tcl
            .session
            .bitstream
            .as_ref()
            .map(|b| b.packets.len())
            .unwrap_or(0),
    });
}

/// Apply one worker event.
pub fn deliver(s: &mut AppState, ev: HwEvent) {
    match ev {
        HwEvent::Line { text, sev } => console(s, text, sev),
        HwEvent::Stage { step, label } => {
            s.program.stage = step;
            s.program.stage_label = label;
        }
        HwEvent::Detected(report) => {
            let line = format!("detect {}", scan_line(&report));
            let sev = if report.physical_had {
                MsgSev::Info
            } else {
                MsgSev::Warn
            };
            console(s, line, sev);
            s.program.detect = Some(*report);
            s.program.job = None;
        }
        HwEvent::Programmed(r) => {
            s.program.job = None;
            match r {
                Ok((res, stat)) => {
                    s.program.phase = ProgramPhase::Done;
                    s.program.stage = STAGES;
                    s.program.programmed_backend = Some(res.backend.clone());
                    match stat {
                        Ok(st) => {
                            s.program.stat = Some(st);
                            s.program.stat_note = None;
                        }
                        Err(note) => {
                            s.program.stat = None;
                            s.program.stat_note = Some(note);
                        }
                    }
                    let verdict = res.verdict(s.program.stat.as_ref());
                    s.toasts
                        .push(ToastKind::Success, "Program Device", &verdict);
                    s.program.result = Some(res);
                }
                Err(res) => {
                    s.program.phase = ProgramPhase::Failed;
                    console(s, format!("ERROR: {}", res.line), MsgSev::Error);
                    let detail: String = res.line.chars().take(140).collect();
                    s.toasts
                        .push(ToastKind::Error, "Program Device failed", &detail);
                    s.program.result = Some(res);
                }
            }
        }
        HwEvent::Stat(r) => {
            s.program.job = None;
            match r {
                Ok(st) => {
                    console(s, st.text(), MsgSev::Info);
                    s.program.stat = Some(st);
                    s.program.stat_note = None;
                }
                Err(e) => {
                    console(s, format!("ERROR: {e}"), MsgSev::Error);
                    s.program.stat_note = Some(e);
                }
            }
        }
    }
}

/// `AppSpec::poll` wiring: drain the hardware worker.
pub fn poll(s: &mut AppState) -> bool {
    let dbg = crate::views::hw_debug::poll(s);
    let cfg = crate::views::hw_cfgmem::poll(s);
    let nat = crate::views::hw_native::poll(s);
    poll_program(s) || dbg || cfg || nat
}

fn poll_program(s: &mut AppState) -> bool {
    let Some(job) = s.program.job.clone() else {
        return false;
    };
    let mut changed = false;
    loop {
        let next = job.rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
        match next {
            Ok(ev) => {
                deliver(s, ev);
                changed = true;
                if s.program.job.is_none() {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                s.program.job = None;
                if job.kind == HwJobKind::Program {
                    s.program.phase = ProgramPhase::Failed;
                }
                console(
                    s,
                    "ERROR: hardware worker exited unexpectedly",
                    MsgSev::Error,
                );
                changed = true;
                break;
            }
        }
    }
    changed
}

/// Block until the in-flight job finishes (tests and scenes only).
pub fn wait_idle(s: &mut AppState, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while s.program.job.is_some() {
        if start.elapsed() > timeout {
            return false;
        }
        if !poll(s) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    true
}

/// Fixture scan for scenes/tests: a real `DetectReport` shape with no USB
/// programmer (what `detect_boards` returns on a machine without one).
pub fn no_usb_fixture() -> helion_hw::DetectReport {
    let mut d = unscanned_report();
    d.note = "No USB programmer attached.".into();
    d.usb.note = "openFPGALoader not on PATH".into();
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program_state() -> AppState {
        let mut s = crate::init();
        s.center = crate::state::CenterPane::Program;
        s
    }

    #[test]
    fn cable_specs_resolve_through_helion_hw() {
        let mut s = program_state();
        assert!(s.program.resolved().is_none(), "auto needs a scan");
        s.program.cable = CableChoice::Sim;
        assert_eq!(s.program.resolved().unwrap().id, "sim0");
        s.program.detect = Some(no_usb_fixture());
        s.program.cable = CableChoice::Auto;
        let auto = s.program.resolved().unwrap();
        assert_eq!(auto.backend, helion_hw::CableBackend::OpenFpgaLoader);
        s.program.cable = CableChoice::Native;
        assert_eq!(
            s.program.resolved().unwrap().backend,
            helion_hw::CableBackend::NativeUsb
        );
        assert_eq!(CableChoice::parse("ofl"), Some(CableChoice::Usb));
        assert_eq!(CableChoice::parse("bogus"), None);
    }

    #[test]
    fn refresh_stat_reads_the_unconfigured_tap() {
        let mut s = program_state();
        s.program.cable = CableChoice::Sim;
        let text = report_hw_stat(&mut s).expect("TAP read");
        let stat = s.program.stat.clone().unwrap();
        assert_eq!(stat.source, StatSource::SimTap);
        assert!(!stat.done(), "an unconfigured TAP is never DONE");
        assert!(stat.bit("GSR").unwrap().value, "reset holds GSR");
        assert!(text.contains("NAME=DONE VALUE=0"), "{text}");
        assert!(select_hw_stat(&mut s, "gts").is_ok());
        assert_eq!(s.program.stat_selected, Some("GTS"));
        s.program.cable = CableChoice::Auto;
        assert!(report_hw_stat(&mut s).is_err(), "no TAP readback over OFL");
    }

    #[test]
    fn bad_bitstream_file_fails_honestly() {
        let path = std::env::temp_dir().join(format!("helion-bad-{}.hbits", std::process::id()));
        std::fs::write(&path, b"not a bitstream").unwrap();
        let job = ProgramJob {
            cable: "sim".into(),
            part: crate::live::FIXTURE_PART.into(),
            bits: BitsSource::File(path.clone()),
        };
        match program_blocking(&job, &|_| {}) {
            HwEvent::Programmed(Err(res)) => {
                assert!(!res.ok);
                assert!(res.line.contains(".hbits"), "{}", res.line);
            }
            other => panic!("bad file must fail: {other:?}"),
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn backend_parses_from_summary_lines() {
        assert_eq!(
            backend_of("program_hw cable=sim0 backend=sim part=X frames=1"),
            "sim"
        );
        assert_eq!(backend_of("no backend here"), "");
    }

    #[test]
    fn tcl_report_hw_stat_and_select() {
        let mut s = program_state();
        s.center = crate::state::CenterPane::Summary;
        crate::tcl::eval_command(&mut s, "report_hw_stat");
        assert_eq!(s.center, crate::state::CenterPane::Program);
        assert!(
            s.console
                .lines
                .last()
                .is_some_and(|l| l.text.contains("Simulation cable")),
            "auto cable refuses a sim read with the reason"
        );
        s.program.cable = CableChoice::Sim;
        crate::tcl::eval_command(&mut s, "report_hw_stat");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.starts_with("hw_stat source=")),
            "STAT dump must reach the console"
        );
        crate::tcl::eval_command(&mut s, "select_hw_stat DONE");
        assert_eq!(s.program.stat_selected, Some("DONE"));
    }
}
