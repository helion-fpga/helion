//! Native FTDI cable model: open the `helion-hw` JTAG engine, scan the
//! chain, set TCK, and raw `scan_ir_hw_jtag` / `scan_dr_hw_jtag` (the view
//! lives in [`crate::views::hw_jtag_view`]).
//!
//! Every step is `helion-hw`: [`helion_hw::open_usb_ftdi`] opens the cable,
//! [`helion_hw::JtagEngine`] encodes MPSSE and decodes TDO. Work runs on a
//! worker; the UI keeps a [`ChainSnap`].
//!
//! Honesty: the engine is verified byte-for-byte against the FTDI MPSSE
//! encodings and end to end against an in-process mock FTDI — never on a
//! physical board. Every result names its [`CableKind`], whose label says
//! so. [`FixtureBoard`] routes the cable to that mock; only scenes/tests set
//! it. Like a USB device, the fixture board persists between opens and only
//! one opener holds it at a time.

use std::sync::{Arc, Mutex, mpsc};

use helion_hw::{
    CableKind, ChainDevice, JtagCable, JtagEngine, MockChainDevice, MockFtdi, MpssePort, Tap,
};

use crate::state::{AppState, ConsoleLine, MsgSev};

/// TCK choices offered in the chain card (FT2232H, divide-by-5 off).
pub const TCK_CHOICES: [u32; 5] = [30_000_000, 15_000_000, 6_000_000, 1_000_000, 100_000];

/// Scan-log rows kept.
pub const SCAN_LOG_MAX: usize = 32;

/// Plain-language verification status for the native path.
pub const NATIVE_NOTE: &str = "Native FTDI engine: MPSSE encoding checked byte-for-byte against \
    the FTDI spec and run end to end against a simulated FTDI + Helion TAP. Not yet verified on a \
    physical board.";

/// Fixture board behind the native cable (scenes/tests only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockBoard {
    /// One Helion device of `part`, optionally with a SPI config flash
    /// (`(jedec_id, capacity_bytes)`).
    Helion {
        part: String,
        flash: Option<([u8; 3], usize)>,
    },
    /// Helion between an xc7a35t-style neighbour and an ARM DAP.
    Chain { part: String },
}

/// A persistent simulated board: the mock FTDI sits in the slot while no
/// cable holds it.
#[derive(Clone)]
pub struct FixtureBoard {
    pub spec: MockBoard,
    slot: Arc<Mutex<Option<MockFtdi>>>,
}

impl std::fmt::Debug for FixtureBoard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureBoard")
            .field("spec", &self.spec)
            .finish()
    }
}

impl FixtureBoard {
    pub fn new(spec: MockBoard) -> Result<Self, String> {
        let m = mock_port(&spec)?;
        Ok(Self {
            spec,
            slot: Arc::new(Mutex::new(Some(m))),
        })
    }

    /// Run `f` on the board while no cable holds it (tests).
    pub fn with_board<R>(&self, f: impl FnOnce(&mut MockFtdi) -> R) -> Option<R> {
        let mut g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        g.as_mut().map(f)
    }
}

/// The fixture board leased to one cable; returned to its slot on drop.
struct LeasedMock {
    slot: Arc<Mutex<Option<MockFtdi>>>,
    m: Option<MockFtdi>,
}

impl LeasedMock {
    fn take(b: &FixtureBoard) -> Result<Self, String> {
        let m = b
            .slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or("native cable already open (the debug target holds it — close_hw_target)")?;
        Ok(Self {
            slot: b.slot.clone(),
            m: Some(m),
        })
    }

    fn m(&mut self) -> &mut MockFtdi {
        self.m.as_mut().expect("leased")
    }
}

impl Drop for LeasedMock {
    fn drop(&mut self) {
        if let Some(m) = self.m.take() {
            *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(m);
        }
    }
}

impl MpssePort for LeasedMock {
    fn describe(&self) -> String {
        self.m.as_ref().map(|m| m.describe()).unwrap_or_default()
    }
    fn usb_reset(&mut self) -> Result<(), String> {
        self.m().usb_reset()
    }
    fn set_latency_timer(&mut self, ms: u8) -> Result<(), String> {
        self.m().set_latency_timer(ms)
    }
    fn set_bitmode(&mut self, mask: u8, mode: u8) -> Result<(), String> {
        self.m().set_bitmode(mask, mode)
    }
    fn purge(&mut self) -> Result<(), String> {
        self.m().purge()
    }
    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        self.m().write(data)
    }
    fn read(&mut self, n: usize) -> Result<Vec<u8>, String> {
        self.m().read(n)
    }
    fn board(&mut self) -> Option<&mut Tap> {
        self.m.as_mut()?.board()
    }
    fn is_mock(&self) -> bool {
        true
    }
}

/// What a worker needs to open the native cable.
#[derive(Clone, Debug)]
pub struct NativeOpen {
    pub tck_hz: u32,
    pub mock: Option<FixtureBoard>,
}

/// UI copy of an opened chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainSnap {
    pub port: String,
    pub kind: CableKind,
    pub tck_hz: u32,
    pub divisor: u16,
    pub devices: Vec<ChainDevice>,
    pub target: usize,
}

/// One raw scan in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanRow {
    pub ir: bool,
    pub bits: usize,
    pub tdi: String,
    pub tdo: Result<String, String>,
}

#[derive(Debug)]
enum NatEvent {
    Scanned(Result<ChainSnap, String>),
    Raw(Box<ScanRow>),
}

#[derive(Clone)]
struct NatJob {
    rx: Arc<Mutex<mpsc::Receiver<NatEvent>>>,
}

/// Native cable state under the Hardware Manager.
#[derive(Clone)]
pub struct NativeState {
    pub tck_hz: u32,
    pub chain: Option<ChainSnap>,
    pub scans: Vec<ScanRow>,
    pub last_error: Option<String>,
    /// Scenes/tests only: the cable reaches this simulated board.
    pub mock: Option<FixtureBoard>,
    /// IR loaded by the last `scan_ir_hw_jtag` (re-applied before a DR scan,
    /// since each worker reopens the cable and the open resets the TAP).
    pub ir: Option<u8>,
    job: Option<NatJob>,
}

impl Default for NativeState {
    fn default() -> Self {
        Self {
            tck_hz: 6_000_000,
            chain: None,
            scans: Vec::new(),
            last_error: None,
            mock: None,
            ir: None,
            job: None,
        }
    }
}

impl std::fmt::Debug for NativeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeState")
            .field("tck_hz", &self.tck_hz)
            .field("chain", &self.chain.as_ref().map(|c| c.devices.len()))
            .field("busy", &self.job.is_some())
            .finish()
    }
}

impl NativeState {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn open_cfg(&self) -> NativeOpen {
        NativeOpen {
            tck_hz: self.tck_hz,
            mock: self.mock.clone(),
        }
    }

    /// Cable kind a native open would produce.
    pub fn kind(&self) -> CableKind {
        if self.mock.is_some() {
            CableKind::NativeMock
        } else {
            CableKind::NativeUsb
        }
    }
}

fn snap<P: helion_hw::MpssePort>(e: &JtagEngine<P>) -> ChainSnap {
    ChainSnap {
        port: e.port().describe(),
        kind: JtagCable::kind(e),
        tck_hz: e.tck_hz(),
        divisor: e.divisor(),
        devices: e.chain().to_vec(),
        target: e.target(),
    }
}

fn mock_port(b: &MockBoard) -> Result<MockFtdi, String> {
    let helion = |part: &str| -> Result<Tap, String> {
        let dev = helion_device::Device::load_part(part)?;
        Ok(Tap::new(&dev))
    };
    Ok(match b {
        MockBoard::Helion { part, flash } => {
            let mut tap = helion(part)?;
            if let Some((id, cap)) = flash {
                let mut f = helion_hw::SpiFlashSim::new(*cap);
                f.jedec_id = *id;
                tap.attach_config_flash(f);
            }
            MockFtdi::with_chain(vec![MockChainDevice::Helion(Box::new(tap))])
        }
        MockBoard::Chain { part } => MockFtdi::with_chain(vec![
            MockChainDevice::Generic(helion_hw::GenericTap::new(Some(0x0362_D093), 6)),
            MockChainDevice::Helion(Box::new(helion(part)?)),
            MockChainDevice::Generic(helion_hw::GenericTap::new(Some(0x4BA0_0477), 4)),
        ]),
    })
}

/// Open the native cable (worker side): the USB FTDI, or the fixture board.
pub fn open_cable(cfg: &NativeOpen) -> Result<(Box<dyn JtagCable>, ChainSnap), String> {
    if let Some(b) = &cfg.mock {
        let e = JtagEngine::open(LeasedMock::take(b)?, cfg.tck_hz)?;
        let s = snap(&e);
        return Ok((Box::new(e), s));
    }
    let port = helion_hw::open_usb_ftdi(None, 'A')?;
    let e = JtagEngine::open(port, cfg.tck_hz)?;
    let s = snap(&e);
    Ok((Box::new(e), s))
}

fn console(s: &mut AppState, text: impl Into<String>, sev: MsgSev) {
    s.console.lines.push(ConsoleLine {
        text: text.into(),
        sev,
    });
}

/// The programmed debug target holds the native cable open.
pub fn held_by_debug(s: &AppState) -> bool {
    s.program.debug.target.is_some()
        && s.program
            .debug
            .snap
            .as_ref()
            .is_some_and(|t| t.cable.is_native())
}

/// `close_hw_target`: release the native cable held by the debug target.
pub fn close_target(s: &mut AppState) -> Result<String, String> {
    if s.program.debug.busy().is_some() {
        return Err("close_hw_target: a debug job is running".into());
    }
    let d = &mut s.program.debug;
    d.target = None;
    d.snap = None;
    Ok("close_hw_target: native cable released".into())
}

fn spawn(s: &mut AppState, body: impl FnOnce() -> NatEvent + Send + 'static) -> Result<(), String> {
    if s.program.native.busy() {
        return Err("native cable busy".into());
    }
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("helion-native-jtag".into())
        .spawn(move || {
            let _ = tx.send(body());
        })
        .map_err(|e| format!("native JTAG worker: {e}"))?;
    s.program.native.job = Some(NatJob {
        rx: Arc::new(Mutex::new(rx)),
    });
    Ok(())
}

/// Why the chain card's actions are unavailable.
pub fn scan_block(s: &AppState) -> Option<String> {
    if s.program.cable != crate::views::hw::CableChoice::Native {
        return Some("Chain scan drives the cable directly: select the Native FTDI cable.".into());
    }
    if s.program.native.busy() {
        return Some("Busy: native cable in use".into());
    }
    if s.program.debug.busy().is_some() || s.program.cfgmem.busy().is_some() {
        return Some("Busy: a debug or flash job holds the cable".into());
    }
    if held_by_debug(s) {
        return Some(
            "The programmed debug target holds the cable. Close Target releases it.".into(),
        );
    }
    None
}

/// `refresh_hw_target`: open the cable, reset, scan IDCODEs + IR lengths.
pub fn scan(s: &mut AppState) -> Result<String, String> {
    if let Some(why) = scan_block(s) {
        return Err(format!("refresh_hw_target: {why}"));
    }
    let cfg = s.program.native.open_cfg();
    let msg = format!(
        "refresh_hw_target native TCK={:.3} MHz",
        f64::from(cfg.tck_hz) / 1e6
    );
    console(s, msg.clone(), MsgSev::Info);
    spawn(s, move || {
        NatEvent::Scanned(open_cable(&cfg).map(|(_, snap)| snap))
    })?;
    Ok(msg)
}

/// Pick a TCK (takes effect on the next open).
pub fn set_tck(s: &mut AppState, hz: u32) -> Result<String, String> {
    if hz == 0 {
        return Err("PARAM.FREQUENCY must be > 0".into());
    }
    let div = helion_hw::mpsse::divisor_for_hz(hz);
    let real = helion_hw::mpsse::tck_hz(div, false);
    s.program.native.tck_hz = real;
    Ok(format!("PARAM.FREQUENCY {real} (divisor {div})"))
}

fn parse_hex(s: &str) -> Result<Vec<u8>, String> {
    let h = s
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .replace('_', "");
    if h.is_empty() || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("bad hex '{s}'"));
    }
    let padded = if h.len() % 2 == 1 { format!("0{h}") } else { h };
    let mut be: Vec<u8> = (0..padded.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&padded[i..i + 2], 16).expect("checked hex"))
        .collect();
    be.reverse();
    Ok(be)
}

fn to_hex(bytes: &[u8], bits: usize) -> String {
    let n = bits.div_ceil(4).max(1);
    let mut s = String::new();
    for b in bytes.iter().rev() {
        s.push_str(&format!("{b:02X}"));
    }
    let s = s.trim_start_matches('0').to_string();
    format!("{s:0>n$}")
}

/// `scan_ir_hw_jtag len -tdi hex` / `scan_dr_hw_jtag len -tdi hex` on the
/// target device (others in BYPASS). DR scans re-load the last IR first.
pub fn raw_scan(s: &mut AppState, ir: bool, bits: usize, tdi: &str) -> Result<String, String> {
    if let Some(why) = scan_block(s) {
        return Err(why);
    }
    if bits == 0 || bits > 4096 {
        return Err("length must be 1..=4096 bits".into());
    }
    let mut data = parse_hex(tdi)?;
    data.resize(bits.div_ceil(8), 0);
    if ir {
        let v = data.first().copied().unwrap_or(0);
        s.program.native.ir = Some(v);
    }
    let cfg = s.program.native.open_cfg();
    let load_ir = s.program.native.ir;
    let tdi_s = to_hex(&data, bits);
    spawn(s, move || {
        let r = (|| -> Result<String, String> {
            let (mut cable, snap) = open_cable(&cfg)?;
            let ir_len = snap.devices[snap.target].ir_len as usize;
            if ir {
                if bits != ir_len {
                    return Err(format!("target IR is {ir_len} bits, not {bits}"));
                }
                cable.shift_ir(data[0])?;
                return Ok(format!("IR {:#04x} loaded", data[0]));
            }
            if let Some(v) = load_ir {
                cable.shift_ir(v)?;
            }
            let out = cable.shift_dr_bits(&data, bits)?;
            Ok(to_hex(&out, bits))
        })();
        NatEvent::Raw(Box::new(ScanRow {
            ir,
            bits,
            tdi: tdi_s,
            tdo: r,
        }))
    })?;
    Ok(format!(
        "{} {bits}",
        if ir {
            "scan_ir_hw_jtag"
        } else {
            "scan_dr_hw_jtag"
        }
    ))
}

fn deliver(s: &mut AppState, ev: NatEvent) {
    s.program.native.job = None;
    match ev {
        NatEvent::Scanned(Ok(snap)) => {
            console(
                s,
                format!(
                    "refresh_hw_target: {} device(s) · {} · {}",
                    snap.devices.len(),
                    snap.port,
                    snap.kind.label()
                ),
                MsgSev::Info,
            );
            s.program.native.chain = Some(snap);
            s.program.native.last_error = None;
        }
        NatEvent::Scanned(Err(e)) => {
            console(s, format!("ERROR: refresh_hw_target: {e}"), MsgSev::Error);
            s.program.native.chain = None;
            s.program.native.last_error = Some(e);
        }
        NatEvent::Raw(row) => {
            match &row.tdo {
                Ok(v) => console(
                    s,
                    format!(
                        "scan_{}_hw_jtag {}: {v}",
                        if row.ir { "ir" } else { "dr" },
                        row.bits
                    ),
                    MsgSev::Info,
                ),
                Err(e) => console(s, format!("ERROR: scan_hw_jtag: {e}"), MsgSev::Error),
            }
            s.program.native.last_error = row.tdo.clone().err();
            s.program.native.scans.insert(0, *row);
            s.program.native.scans.truncate(SCAN_LOG_MAX);
        }
    }
}

/// Drain the worker (called from [`crate::views::hw::poll`]).
pub fn poll(s: &mut AppState) -> bool {
    let Some(job) = s.program.native.job.clone() else {
        return false;
    };
    let next = job.rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
    match next {
        Ok(ev) => {
            deliver(s, ev);
            true
        }
        Err(mpsc::TryRecvError::Empty) => false,
        Err(mpsc::TryRecvError::Disconnected) => {
            s.program.native.job = None;
            s.program.native.last_error = Some("native JTAG worker exited".into());
            true
        }
    }
}

pub fn wait_idle(s: &mut AppState, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while s.program.native.busy() {
        if start.elapsed() > timeout {
            return false;
        }
        if !poll(s) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    true
}

// ---- Tcl ----

pub const VERBS: &[&str] = &[
    "refresh_hw_target",
    "close_hw_target",
    "get_hw_devices",
    "scan_ir_hw_jtag",
    "scan_dr_hw_jtag",
];

pub fn help(verb: &str) -> Option<(&'static str, &'static str)> {
    Some(match verb {
        "refresh_hw_target" => (
            "refresh_hw_target",
            "scan the native FTDI JTAG chain (IDCODEs, IR lengths)",
        ),
        "close_hw_target" => (
            "close_hw_target",
            "release the native cable held by the debug target",
        ),
        "get_hw_devices" => ("get_hw_devices", "list devices on the scanned JTAG chain"),
        "scan_ir_hw_jtag" => (
            "scan_ir_hw_jtag len -tdi hex",
            "shift the target's instruction register (others in BYPASS)",
        ),
        "scan_dr_hw_jtag" => (
            "scan_dr_hw_jtag len -tdi hex",
            "shift the target's data register; returns TDO hex",
        ),
        _ => return None,
    })
}

fn finish(s: &mut AppState, started: Result<String, String>) -> Result<String, String> {
    started?;
    if !wait_idle(s, std::time::Duration::from_secs(30)) {
        return Err("native cable timed out".into());
    }
    match &s.program.native.last_error {
        Some(e) => Err(e.clone()),
        None => Ok(String::new()),
    }
}

pub fn tcl_verb(s: &mut AppState, verb: &str, args: &[String]) -> Result<String, String> {
    match verb {
        "refresh_hw_target" => {
            let r = scan(s);
            finish(s, r)?;
            Ok(device_lines(s))
        }
        "close_hw_target" => close_target(s),
        "get_hw_devices" => {
            if s.program.native.chain.is_none() {
                return Err("get_hw_devices: no scanned chain (refresh_hw_target)".into());
            }
            Ok(device_lines(s))
        }
        "scan_ir_hw_jtag" | "scan_dr_hw_jtag" => {
            let bits: usize = args
                .first()
                .and_then(|a| a.parse().ok())
                .ok_or_else(|| format!("{verb}: length required"))?;
            let tdi = args
                .iter()
                .position(|a| a == "-tdi")
                .and_then(|i| args.get(i + 1))
                .map(String::as_str)
                .unwrap_or("0");
            let r = raw_scan(s, verb == "scan_ir_hw_jtag", bits, tdi);
            finish(s, r)?;
            match s.program.native.scans.first().map(|r| r.tdo.clone()) {
                Some(Ok(v)) => Ok(v),
                Some(Err(e)) => Err(e),
                None => Err("no scan result".into()),
            }
        }
        _ => Err(format!("unknown verb {verb}")),
    }
}

fn device_lines(s: &AppState) -> String {
    let Some(c) = &s.program.native.chain else {
        return String::new();
    };
    c.devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            format!(
                "{} idcode={} ir={}{}",
                d.name,
                d.idcode
                    .map(|v| format!("{v:#010x}"))
                    .unwrap_or_else(|| "bypass".into()),
                d.ir_len,
                if i == c.target { " target" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Point the native cable at a fixture board (scenes/tests only).
pub fn use_mock_board(s: &mut AppState, board: MockBoard) -> Result<(), String> {
    s.program.cable = crate::views::hw::CableChoice::Native;
    s.program.native.mock = Some(FixtureBoard::new(board)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        let mut s = AppState::demo();
        use_mock_board(
            &mut s,
            MockBoard::Chain {
                part: "HL10T-C32-1".into(),
            },
        )
        .unwrap();
        s
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(parse_hex("0x1A2B").unwrap(), vec![0x2B, 0x1A]);
        assert_eq!(to_hex(&[0x2B, 0x1A], 16), "1A2B");
        assert_eq!(to_hex(&[0x03], 6), "03");
        assert!(parse_hex("zz").is_err());
    }

    #[test]
    fn refresh_hw_target_scans_the_mock_chain() {
        let mut s = state();
        let out = tcl_verb(&mut s, "refresh_hw_target", &[]).unwrap();
        assert!(
            out.contains("Helion HL10T idcode=0x00011a1f ir=6 target"),
            "{out}"
        );
        assert!(out.contains("xc7a35t"), "{out}");
        let c = s.program.native.chain.clone().unwrap();
        assert_eq!(c.kind, CableKind::NativeMock);
        assert_eq!(c.devices.len(), 3);
        assert_eq!(c.tck_hz, 6_000_000);
        assert_eq!(c.divisor, 4);
    }

    #[test]
    fn user2_round_trip_through_tcl() {
        let mut s = state();
        tcl_verb(
            &mut s,
            "scan_ir_hw_jtag",
            &["6".into(), "-tdi".into(), "22".into()],
        )
        .unwrap();
        // USER2 captures its reset value, then latches the written word; the
        // board persists across cable opens like a real one.
        let dr = |s: &mut AppState, v: &str| {
            tcl_verb(
                s,
                "scan_dr_hw_jtag",
                &["64".into(), "-tdi".into(), v.into()],
            )
            .unwrap()
        };
        assert_eq!(dr(&mut s, "0xCAFE"), "0000000000000000");
        assert_eq!(dr(&mut s, "0x0"), "000000000000CAFE");
        let latched = s
            .program
            .native
            .mock
            .as_ref()
            .unwrap()
            .with_board(|m| m.helion_tap().unwrap().user_register(helion_hw::IR_USR2));
        assert_eq!(latched, Some(Some(0)));
        assert!(tcl_verb(&mut s, "scan_ir_hw_jtag", &["5".into()]).is_err());
        assert_eq!(s.program.native.scans.len(), 4);
    }

    #[test]
    fn tck_snaps_to_a_real_divisor() {
        let mut s = state();
        assert!(set_tck(&mut s, 7_000_000).unwrap().contains("6000000"));
        assert_eq!(s.program.native.tck_hz, 6_000_000);
        assert!(set_tck(&mut s, 0).is_err());
    }

    #[test]
    fn usb_open_without_cable_is_honest() {
        let mut s = AppState::demo();
        s.program.cable = crate::views::hw::CableChoice::Native;
        let r = tcl_verb(&mut s, "refresh_hw_target", &[]);
        // No FTDI on CI: the error names the missing cable, no chain invented.
        if let Err(e) = r {
            assert!(e.contains("FTDI") || e.contains("usb"), "{e}");
            assert!(s.program.native.chain.is_none());
        }
    }

    #[test]
    fn other_cables_are_refused_with_a_reason() {
        let mut s = AppState::demo();
        s.program.cable = crate::views::hw::CableChoice::Sim;
        assert!(scan_block(&s).unwrap().contains("Native FTDI"));
    }
}
