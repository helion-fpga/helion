//! Configuration memory model: Add Configuration Memory Device,
//! `write_cfgmem`, and Program / Verify / Erase / Boot (the view lives in
//! [`crate::views::hw_cfgmem_view`]).
//!
//! Every step is `helion-hw`: [`helion_hw::write_cfgmem`] builds the `.mcs`
//! / `.bin` image, [`helion_hw::CfgMemSession`] runs the indirect SPI flow
//! over the Simulation cable (TAP `USER1` → JEDEC SPI flash model) or the
//! Native FTDI cable (MPSSE JTAG engine → the TAP's `SPI_ACCESS` port →
//! the board's flash; not yet verified on a physical board). On an
//! openFPGALoader cable, Program writes the bitstream to SPI flash through
//! `openFPGALoader -f`; erase / verify / blank check / boot are not
//! available there and say so. BPI parts stay image-only (no BPI model).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use helion_hw::{
    CFGMEM_PARTS, CfgMemFormat, CfgMemImage, CfgMemInterface, CfgMemPart, CfgMemProgramOpts,
    CfgMemReport, CfgMemSession,
};
use rune::TextEdit;

use crate::state::{AppState, ConsoleLine, MsgSev, ToastKind};
use crate::views::hw::CableChoice;

/// Operation in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgOp {
    Program,
    Verify,
    Erase,
    BlankCheck,
    Boot,
    Readback,
    BoardProgram,
}

impl CfgOp {
    pub fn done_label(self) -> &'static str {
        match self {
            CfgOp::Program => "Flash programmed",
            CfgOp::Verify => "Flash verified",
            CfgOp::Erase => "Flash erased",
            CfgOp::BlankCheck => "Blank check finished",
            CfgOp::Boot => "Booted from flash",
            CfgOp::Readback => "Flash read back",
            CfgOp::BoardProgram => "openFPGALoader wrote flash",
        }
    }

    pub fn busy_label(self) -> &'static str {
        match self {
            CfgOp::Program => "Programming flash…",
            CfgOp::Verify => "Verifying flash…",
            CfgOp::Erase => "Erasing flash…",
            CfgOp::BlankCheck => "Blank checking…",
            CfgOp::Boot => "Booting from flash…",
            CfgOp::Readback => "Reading flash back…",
            CfgOp::BoardProgram => "openFPGALoader writing flash…",
        }
    }
}

/// Outcome of the last operation, as the panel shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CfgResult {
    pub op: CfgOp,
    pub ok: bool,
    pub report: Option<CfgMemReport>,
    pub line: String,
}

#[derive(Debug)]
enum CfgEvent {
    Done(CfgResult),
}

#[derive(Clone)]
struct CfgJob {
    op: CfgOp,
    rx: Arc<Mutex<mpsc::Receiver<CfgEvent>>>,
}

/// Configuration memory state under the Hardware Manager device.
#[derive(Clone)]
pub struct CfgMemState {
    /// Part picked in the Add dialog (index into [`CFGMEM_PARTS`]).
    pub pick: usize,
    /// Part added to the device (`create_hw_cfgmem`).
    pub part: Option<&'static CfgMemPart>,
    pub interface: CfgMemInterface,
    pub format: CfgMemFormat,
    pub start: TextEdit,
    pub opts: CfgMemProgramOpts,
    pub image: Option<CfgMemImage>,
    /// Where the image was written / loaded from.
    pub image_path: Option<PathBuf>,
    pub session: Option<Arc<Mutex<CfgMemSession>>>,
    pub result: Option<CfgResult>,
    /// Image currently in flash (set by a verified program).
    pub flashed: Option<(u32, usize, u32)>,
    job: Option<CfgJob>,
}

impl Default for CfgMemState {
    fn default() -> Self {
        Self {
            pick: 0,
            part: None,
            interface: CfgMemInterface::SpiX4,
            format: CfgMemFormat::Mcs,
            start: TextEdit::new("0x0"),
            opts: CfgMemProgramOpts::default(),
            image: None,
            image_path: None,
            session: None,
            result: None,
            flashed: None,
            job: None,
        }
    }
}

impl std::fmt::Debug for CfgMemState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CfgMemState")
            .field("part", &self.part.map(|p| p.name))
            .field("image", &self.image.as_ref().map(|i| i.payload.len()))
            .field("job", &self.job.as_ref().map(|j| j.op))
            .finish()
    }
}

impl CfgMemState {
    pub fn busy(&self) -> Option<CfgOp> {
        self.job.as_ref().map(|j| j.op)
    }
}

fn console(s: &mut AppState, text: impl Into<String>, sev: MsgSev) {
    s.console.lines.push(ConsoleLine {
        text: text.into(),
        sev,
    });
}

/// Which flash path the selected cable gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashPath {
    /// Simulation cable: USER1 bridge into the flash model.
    Sim,
    /// Native FTDI: MPSSE JTAG engine → TAP SPI_ACCESS → board flash.
    Native,
    /// openFPGALoader `-f` (program only, no readback).
    Ofl,
}

pub fn flash_path(s: &AppState) -> FlashPath {
    match s.program.cable {
        CableChoice::Sim => FlashPath::Sim,
        CableChoice::Native => FlashPath::Native,
        _ => FlashPath::Ofl,
    }
}

/// `create_hw_cfgmem`: attach the picked part to the device.
pub fn add_device(s: &mut AppState, part: &'static CfgMemPart) -> Result<String, String> {
    if s.program.cfgmem.busy().is_some() {
        return Err("create_hw_cfgmem: a flash operation is running".into());
    }
    let c = &mut s.program.cfgmem;
    c.part = Some(part);
    c.pick = CFGMEM_PARTS.iter().position(|p| p == part).unwrap_or(0);
    if !part.supports(c.interface) {
        c.interface = part.interface;
    }
    c.image = None;
    c.image_path = None;
    c.result = None;
    c.flashed = None;
    c.session = None;
    if part.jedec_id.is_some() {
        let (dev, _) = crate::live::live_device(s);
        let sess = CfgMemSession::new(&dev, part)?;
        s.program.cfgmem.session = Some(Arc::new(Mutex::new(sess)));
    }
    s.program.node = crate::views::hw_tree::HwNode::CfgMem;
    Ok(format!("create_hw_cfgmem {}", part.name))
}

/// Remove the configuration memory device.
pub fn remove_device(s: &mut AppState) {
    if s.program.cfgmem.busy().is_none() {
        s.program.cfgmem = CfgMemState::default();
        if s.program.node == crate::views::hw_tree::HwNode::CfgMem {
            s.program.node = crate::views::hw_tree::HwNode::Device;
        }
    }
}

pub fn start_addr(s: &AppState) -> Result<u32, String> {
    let v = crate::views::hw_debug::parse_num(&s.program.cfgmem.start.text)?;
    u32::try_from(v).map_err(|_| "start address must fit 32 bits".to_string())
}

/// Why Generate Image is unavailable.
pub fn write_block(s: &AppState) -> Option<String> {
    if s.program.cfgmem.part.is_none() {
        return Some("Add a configuration memory device first.".into());
    }
    if crate::views::program::bits_source(s).is_none() {
        return Some("No bitstream yet. Generate Bitstream first.".into());
    }
    if let Some(op) = s.program.cfgmem.busy() {
        return Some(format!("Busy: {}", op.busy_label()));
    }
    None
}

fn image_dir(s: &AppState) -> PathBuf {
    crate::runs::checkpoint_root(s).join("cfgmem")
}

fn current_bits(s: &AppState) -> Result<helion_bits::Bitstream, String> {
    match crate::views::program::bits_source(s) {
        Some((crate::views::hw::BitsSource::Memory(b), _)) => Ok((*b).clone()),
        Some((crate::views::hw::BitsSource::File(p), _)) => {
            let bytes = std::fs::read(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
            helion_bits::Bitstream::from_packets(&bytes)
        }
        None => Err("no bitstream".into()),
    }
}

/// `write_cfgmem`: build the image from the current bitstream and write it
/// next to the run checkpoints (or to `path`).
pub fn write_image(s: &mut AppState, path: Option<PathBuf>) -> Result<String, String> {
    if let Some(why) = write_block(s) {
        return Err(format!("write_cfgmem: {why}"));
    }
    let part = s.program.cfgmem.part.expect("checked");
    let bits = current_bits(s)?;
    let start = start_addr(s)?;
    let c = &s.program.cfgmem;
    let img = helion_hw::write_cfgmem(&bits, part, c.format, c.interface, start)?;
    let path = match path {
        Some(p) => p,
        None => {
            let dir = image_dir(s);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("write_cfgmem: create {}: {e}", dir.display()))?;
            dir.join(format!("{}.{}", crate::live::slug(&s.project.name), img.format.as_str()))
        }
    };
    img.write(&path)?;
    let msg = format!(
        "write_cfgmem -format {} -interface {} -loadbit \"up {:#x}\" {} ({} B, CRC32 {:08X})",
        img.format.as_str(),
        img.interface.as_str(),
        img.start_addr,
        path.display(),
        img.payload.len(),
        img.crc32
    );
    console(s, msg.clone(), MsgSev::Info);
    s.program.cfgmem.image = Some(img);
    s.program.cfgmem.image_path = Some(path);
    Ok(msg)
}

/// Load an existing `.mcs` / `.bin` as the image to program.
pub fn load_image(s: &mut AppState, path: PathBuf) -> Result<String, String> {
    let part = s
        .program
        .cfgmem
        .part
        .ok_or("Add a configuration memory device first.")?;
    let start = start_addr(s)?;
    let img = helion_hw::read_cfgmem_file(&path, part, s.program.cfgmem.interface, start)?;
    let msg = format!(
        "PROGRAM.FILES {} ({} B at {:#x})",
        path.display(),
        img.payload.len(),
        img.start_addr
    );
    console(s, msg.clone(), MsgSev::Info);
    s.program.cfgmem.image = Some(img);
    s.program.cfgmem.image_path = Some(path);
    Ok(msg)
}

/// Why `op` is unavailable right now (`None` = enabled).
pub fn op_block(s: &AppState, op: CfgOp) -> Option<String> {
    let c = &s.program.cfgmem;
    if let Some(b) = c.busy() {
        return Some(format!("Busy: {}", b.busy_label()));
    }
    let part = c.part?;
    if flash_path(s) == FlashPath::Native {
        if part.jedec_id.is_none() {
            return Some(format!(
                "{} is a BPI part: helion-hw has no BPI flash model to program.",
                part.name
            ));
        }
        if crate::views::hw_native::held_by_debug(s) {
            return Some(
                "The programmed debug target holds the native cable. Close Target releases it."
                    .into(),
            );
        }
    } else if flash_path(s) == FlashPath::Ofl {
        if op != CfgOp::Program {
            return Some(
                "Board flash erase/verify/boot are not wired: helion-hw drives board flash only \
                 through openFPGALoader -f. Use the Simulation cable."
                    .into(),
            );
        }
        if !s.program.physical() {
            return Some("No USB programmer detected (USB=0). Run Detect or use Simulation.".into());
        }
        if crate::views::program::bits_source(s).is_none() {
            return Some("No bitstream yet. Generate Bitstream first.".into());
        }
        return None;
    } else if c.session.is_none() {
        return Some(format!(
            "{} is a BPI part: helion-hw has no BPI flash model to program.",
            part.name
        ));
    }
    match op {
        CfgOp::Program | CfgOp::Verify | CfgOp::BlankCheck if c.image.is_none() => {
            Some("Generate or load a configuration image first.".into())
        }
        CfgOp::Program if !(c.opts.erase || c.opts.blank_check || c.opts.program || c.opts.verify) => {
            Some("Select at least one step.".into())
        }
        CfgOp::Boot | CfgOp::Readback if c.flashed.is_none() => {
            Some("Program the flash first.".into())
        }
        _ => None,
    }
}

/// Run one flash operation on an open session (worker side).
fn exec(
    sess: &mut CfgMemSession,
    op: CfgOp,
    image: Option<CfgMemImage>,
    opts: CfgMemProgramOpts,
    flashed: Option<(u32, usize, u32)>,
    readback_to: PathBuf,
) -> CfgResult {
    let path = sess.cable_kind().label();
    match op {
        CfgOp::Program => result(op, sess.program(image.as_ref().expect("checked"), opts)),
        CfgOp::Verify => {
            let img = image.expect("checked");
            result(op, sess.verify(img.start_addr, &img.payload))
        }
        CfgOp::BlankCheck => {
            let img = image.expect("checked");
            result(op, sess.blank_check(img.start_addr, img.payload.len()))
        }
        CfgOp::Erase => match &image {
            Some(img) => result(op, sess.erase(img.start_addr, img.payload.len())),
            None => result(op, sess.erase_all()),
        },
        CfgOp::Readback => {
            let (start, len, _) = flashed.expect("checked");
            let r = sess.readback(start, len).and_then(|(mut rep, data)| {
                if let Some(dir) = readback_to.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("readback: {e}"))?;
                }
                std::fs::write(&readback_to, &data)
                    .map_err(|e| format!("readback: write {}: {e}", readback_to.display()))?;
                rep.steps.push(format!("wrote {}", readback_to.display()));
                Ok(rep)
            });
            result(op, r)
        }
        CfgOp::Boot => {
            let (start, len, _) = flashed.expect("checked");
            match sess.boot(start, len) {
                Ok(st) => CfgResult {
                    op,
                    ok: st.done,
                    report: None,
                    line: format!(
                        "boot from flash {start:#x}: DONE={} GWE={} CRC_ERR={} ({path})",
                        u8::from(st.done),
                        u8::from(st.gwe),
                        u8::from(st.crc_err)
                    ),
                },
                Err(e) => CfgResult {
                    op,
                    ok: false,
                    report: None,
                    line: e,
                },
            }
        }
        CfgOp::BoardProgram => unreachable!("openFPGALoader path runs elsewhere"),
    }
}

/// Where Read Back writes the flash contents.
pub fn readback_path(s: &AppState) -> PathBuf {
    image_dir(s).join("readback.bin")
}

fn spawn(s: &mut AppState, op: CfgOp, body: impl FnOnce() -> CfgResult + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("helion-cfgmem".into())
        .spawn(move || {
            let _ = tx.send(CfgEvent::Done(body()));
        });
    match spawned {
        Ok(_) => {
            s.program.cfgmem.job = Some(CfgJob {
                op,
                rx: Arc::new(Mutex::new(rx)),
            });
        }
        Err(e) => console(s, format!("ERROR: cfgmem worker: {e}"), MsgSev::Error),
    }
}

fn result(op: CfgOp, r: Result<CfgMemReport, String>) -> CfgResult {
    match r {
        Ok(rep) => CfgResult {
            op,
            ok: rep.verified != Some(false) && rep.nonblank_bytes.unwrap_or(0) == 0,
            line: rep.steps.join(" · "),
            report: Some(rep),
        },
        Err(e) => CfgResult {
            op,
            ok: false,
            report: None,
            line: e,
        },
    }
}

/// Run `op` on the worker.
pub fn run(s: &mut AppState, op: CfgOp) -> Result<String, String> {
    if let Some(why) = op_block(s, op) {
        return Err(why);
    }
    let c = &s.program.cfgmem;
    let part = c.part.expect("checked");
    if flash_path(s) == FlashPath::Ofl {
        // Board: openFPGALoader -f with the bitstream (never claims a verify
        // the tool did not report).
        let Some((bits_src, _)) = crate::views::program::bits_source(s) else {
            return Err("no bitstream".into());
        };
        let path = match bits_src {
            crate::views::hw::BitsSource::File(p) => p,
            crate::views::hw::BitsSource::Memory(b) => {
                let dir = image_dir(s);
                std::fs::create_dir_all(&dir).map_err(|e| format!("cfgmem: {e}"))?;
                let p = dir.join("board_flash.hbits");
                std::fs::write(&p, &b.packets).map_err(|e| format!("cfgmem: {e}"))?;
                p
            }
        };
        let cable = s.program.resolved().ok_or("cable not resolved (run Detect)")?;
        let part_name = crate::views::program::program_part(s);
        console(
            s,
            format!("program_hw_cfgmem {} via {} (openFPGALoader -f)", part.name, cable.id),
            MsgSev::Info,
        );
        spawn(s, CfgOp::BoardProgram, move || {
            let r = helion_device::Device::load_part(&part_name).and_then(|dev| {
                helion_hw::program_hbits_with_cable(&dev, &path, &cable, true)
            });
            match r {
                Ok(o) => CfgResult {
                    op: CfgOp::BoardProgram,
                    ok: true,
                    report: None,
                    line: o.summary_line("program_hw_cfgmem", &part_name),
                },
                Err(e) => CfgResult {
                    op: CfgOp::BoardProgram,
                    ok: false,
                    report: None,
                    line: e,
                },
            }
        });
        return Ok(format!("program_hw_cfgmem {} (board)", part.name));
    }
    let image = c.image.clone();
    let opts = c.opts;
    let flashed = c.flashed;
    let verb = match op {
        CfgOp::Program => "program_hw_cfgmem",
        CfgOp::Verify => "verify_hw_cfgmem",
        CfgOp::Erase => "erase_hw_cfgmem",
        CfgOp::BlankCheck => "blank_check_hw_cfgmem",
        CfgOp::Boot => "boot_hw_device",
        CfgOp::Readback => "readback_hw_cfgmem",
        CfgOp::BoardProgram => "program_hw_cfgmem",
    };
    let out = readback_path(s);
    if flash_path(s) == FlashPath::Native {
        // Each operation opens the native cable, runs, and releases it.
        let cfg = s.program.native.open_cfg();
        console(
            s,
            format!("{verb} {} via {}", part.name, s.program.native.kind().label()),
            MsgSev::Info,
        );
        spawn(s, op, move || {
            let sess = crate::views::hw_native::open_cable(&cfg)
                .and_then(|(cable, _)| CfgMemSession::over_cable(part, cable));
            match sess {
                Ok(mut sess) => exec(&mut sess, op, image, opts, flashed, out),
                Err(e) => CfgResult {
                    op,
                    ok: false,
                    report: None,
                    line: e,
                },
            }
        });
        return Ok(format!("{verb} {} (native)", part.name));
    }
    let sess = c.session.clone().expect("checked");
    console(s, format!("{verb} {}", part.name), MsgSev::Info);
    spawn(s, op, move || {
        let mut sess = sess.lock().unwrap_or_else(|e| e.into_inner());
        exec(&mut sess, op, image, opts, flashed, out)
    });
    Ok(format!("{verb} {}", part.name))
}

fn deliver(s: &mut AppState, r: CfgResult) {
    let c = &mut s.program.cfgmem;
    c.job = None;
    match (r.op, r.ok) {
        (CfgOp::Program, true) => {
            if let (Some(img), true) = (&c.image, c.opts.program) {
                c.flashed = Some((img.start_addr, img.payload.len(), img.crc32));
            }
        }
        (CfgOp::Erase, true) => c.flashed = None,
        _ => {}
    }
    let sev = if r.ok { MsgSev::Info } else { MsgSev::Error };
    let line = if r.ok {
        r.line.clone()
    } else {
        format!("ERROR: {}", r.line)
    };
    console(s, line, sev);
    if r.ok {
        s.toasts
            .push(ToastKind::Success, "Configuration memory", r.op.done_label());
    } else {
        let detail: String = r.line.chars().take(140).collect();
        s.toasts.push(ToastKind::Error, "Configuration memory failed", &detail);
    }
    s.program.cfgmem.result = Some(r);
}

pub fn poll(s: &mut AppState) -> bool {
    let Some(job) = s.program.cfgmem.job.clone() else {
        return false;
    };
    let next = job.rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
    match next {
        Ok(CfgEvent::Done(r)) => {
            deliver(s, r);
            true
        }
        Err(mpsc::TryRecvError::Empty) => false,
        Err(mpsc::TryRecvError::Disconnected) => {
            s.program.cfgmem.job = None;
            console(s, "ERROR: cfgmem worker exited", MsgSev::Error);
            true
        }
    }
}

pub fn wait_idle(s: &mut AppState, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while s.program.cfgmem.job.is_some() {
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
    "get_cfgmem_parts",
    "create_hw_cfgmem",
    "write_cfgmem",
    "program_hw_cfgmem",
    "readback_hw_cfgmem",
    "boot_hw_device",
];

pub fn help(verb: &str) -> Option<(&'static str, &'static str)> {
    Some(match verb {
        "get_cfgmem_parts" => ("get_cfgmem_parts ?pattern?", "list configuration flash parts"),
        "create_hw_cfgmem" => (
            "create_hw_cfgmem part",
            "add a configuration memory device to the hardware device",
        ),
        "write_cfgmem" => (
            "write_cfgmem -format mcs|bin -interface SPIx1|SPIx2|SPIx4|BPIx16 ?-loadbit {up addr}? ?file?",
            "build a flash image from the current bitstream",
        ),
        "program_hw_cfgmem" => (
            "program_hw_cfgmem ?-file f? ?-noerase? ?-blank_check? ?-noprogram? ?-noverify? | -erase_only | -verify_only",
            "erase / blank check / program / verify the configuration flash",
        ),
        "readback_hw_cfgmem" => (
            "readback_hw_cfgmem",
            "read the programmed flash range back to readback.bin",
        ),
        "boot_hw_device" => ("boot_hw_device", "configure the device from flash"),
        _ => return None,
    })
}

fn flag(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a.eq_ignore_ascii_case(f))
}

fn opt<'a>(args: &'a [String], f: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a.eq_ignore_ascii_case(f))
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn finish(s: &mut AppState, started: Result<String, String>) -> Result<String, String> {
    started?;
    wait_idle(s, std::time::Duration::from_secs(120));
    match &s.program.cfgmem.result {
        Some(r) if r.ok => Ok(r.line.clone()),
        Some(r) => Err(r.line.clone()),
        None => Err("no result".into()),
    }
}

pub fn tcl_verb(s: &mut AppState, verb: &str, args: &[String]) -> Result<String, String> {
    match verb {
        "get_cfgmem_parts" => {
            let pat = args.first().map(|p| p.trim_matches('*').to_ascii_lowercase());
            Ok(CFGMEM_PARTS
                .iter()
                .filter(|p| pat.as_ref().is_none_or(|q| p.name.contains(q.as_str())))
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join(" "))
        }
        "create_hw_cfgmem" => {
            let name = args
                .iter()
                .rev()
                .find(|a| !a.starts_with('-') && !a.starts_with('['))
                .ok_or("create_hw_cfgmem: missing part")?;
            let part = helion_hw::find_cfgmem_part(name)
                .ok_or_else(|| format!("create_hw_cfgmem: unknown part {name}"))?;
            add_device(s, part)
        }
        "write_cfgmem" => {
            if let Some(f) = opt(args, "-format") {
                s.program.cfgmem.format = CfgMemFormat::parse(f)?;
            }
            if let Some(i) = opt(args, "-interface") {
                s.program.cfgmem.interface = CfgMemInterface::parse(i)?;
            }
            if let Some(l) = opt(args, "-loadbit") {
                let addr = l
                    .split_whitespace()
                    .map(|w| w.trim_matches(|c| c == '{' || c == '}' || c == '"'))
                    .find(|w| w.starts_with("0x") || w.chars().all(|c| c.is_ascii_digit()))
                    .ok_or("write_cfgmem: -loadbit {up <addr> <file>}")?;
                s.program.cfgmem.start = TextEdit::new(addr);
            }
            let mut i = 0;
            let mut file = None;
            while i < args.len() {
                if args[i].starts_with('-') {
                    i += if args[i] == "-force" { 1 } else { 2 };
                } else {
                    file = Some(PathBuf::from(&args[i]));
                    i += 1;
                }
            }
            write_image(s, file)
        }
        "program_hw_cfgmem" => {
            if let Some(f) = opt(args, "-file") {
                load_image(s, PathBuf::from(f))?;
            }
            if flag(args, "-erase_only") {
                return {
                let r = run(s, CfgOp::Erase);
                finish(s, r)
            };
            }
            if flag(args, "-verify_only") {
                return {
                let r = run(s, CfgOp::Verify);
                finish(s, r)
            };
            }
            let o = &mut s.program.cfgmem.opts;
            o.erase = !flag(args, "-noerase");
            o.blank_check = flag(args, "-blank_check");
            o.program = !flag(args, "-noprogram");
            o.verify = !flag(args, "-noverify");
            {
                let r = run(s, CfgOp::Program);
                finish(s, r)
            }
        }
        "boot_hw_device" => {
            let r = run(s, CfgOp::Boot);
            finish(s, r)
        }
        "readback_hw_cfgmem" => {
            let r = run(s, CfgOp::Readback);
            finish(s, r)
        }
        other => Err(format!("{other}: not a cfgmem verb")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        let mut s = AppState::demo();
        s.program.cable = CableChoice::Sim;
        s.runs.checkpoint_root = Some(
            std::env::temp_dir().join(format!("helion-rune-cfgmem-{}", std::process::id())),
        );
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let d = helion_ir::Design::structural_counter();
        let packed = helion_pack::pack(&d, &dev).unwrap();
        let placed = helion_place::place(&packed, &dev).unwrap();
        let routed = helion_route::route(&placed, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &routed).unwrap();
        s.tcl.session.bitstream = Some(bits);
        s
    }

    #[test]
    fn tcl_flow_write_program_verify_boot_erase() {
        let mut s = state();
        assert!(op_block(&s, CfgOp::Program).is_none() || s.program.cfgmem.part.is_none());
        crate::views::hw_cfgmem::tcl_verb(&mut s, "create_hw_cfgmem", &["mt25ql128".into()])
            .unwrap();
        assert!(op_block(&s, CfgOp::Program).unwrap().contains("image"));
        let w = tcl_verb(
            &mut s,
            "write_cfgmem",
            &["-format".into(), "mcs".into(), "-interface".into(), "SPIx4".into()],
        )
        .unwrap();
        assert!(w.contains("write_cfgmem -format mcs"), "{w}");
        let path = s.program.cfgmem.image_path.clone().unwrap();
        assert!(path.starts_with(std::env::temp_dir()) && path.is_file());
        let p = tcl_verb(&mut s, "program_hw_cfgmem", &[]).unwrap();
        assert!(p.contains("verify:"), "{p}");
        assert!(s.program.cfgmem.flashed.is_some());
        let b = tcl_verb(&mut s, "boot_hw_device", &[]).unwrap();
        assert!(b.contains("DONE=1"), "{b}");
        tcl_verb(&mut s, "program_hw_cfgmem", &["-erase_only".into()]).unwrap();
        assert!(s.program.cfgmem.flashed.is_none());
        let v = tcl_verb(&mut s, "program_hw_cfgmem", &["-verify_only".into()]);
        assert!(v.is_err(), "erased flash must not verify: {v:?}");
    }

    #[test]
    fn bpi_and_board_paths_are_refused_with_reasons() {
        let mut s = state();
        tcl_verb(&mut s, "create_hw_cfgmem", &["mt28ew128a".into()]).unwrap();
        assert!(op_block(&s, CfgOp::Erase).unwrap().contains("BPI"));
        let mut s = state();
        tcl_verb(&mut s, "create_hw_cfgmem", &["mt25ql128".into()]).unwrap();
        s.program.cable = CableChoice::Usb;
        assert!(op_block(&s, CfgOp::Erase).unwrap().contains("openFPGALoader"));
        assert!(op_block(&s, CfgOp::Program).unwrap().contains("USB"));
        s.program.cable = CableChoice::Native;
        assert!(op_block(&s, CfgOp::Erase).is_none(), "native FTDI erases");
    }

    #[test]
    fn native_flow_program_verify_readback_boot_over_mock_ftdi() {
        let mut s = state();
        let part = helion_hw::find_cfgmem_part("mt25ql128").unwrap();
        crate::views::hw_native::use_mock_board(
            &mut s,
            crate::views::hw_native::MockBoard::Helion {
                part: "HL10T-C32-1".into(),
                flash: Some((part.jedec_id.unwrap(), part.capacity_bytes())),
            },
        )
        .unwrap();
        tcl_verb(&mut s, "create_hw_cfgmem", &["mt25ql128".into()]).unwrap();
        tcl_verb(&mut s, "write_cfgmem", &["-format".into(), "bin".into(), "-interface".into(), "SPIx1".into()]).unwrap();
        let out = tcl_verb(&mut s, "program_hw_cfgmem", &[]).unwrap();
        assert!(out.contains("verify:") && out.contains("match"), "{out}");
        let rep = s.program.cfgmem.result.clone().unwrap().report.unwrap();
        assert!(rep.jtag_dr_scans > 0 && rep.verified == Some(true));
        let rb = tcl_verb(&mut s, "readback_hw_cfgmem", &[]).unwrap();
        assert!(rb.contains("read back"), "{rb}");
        let img = s.program.cfgmem.image.clone().unwrap();
        assert_eq!(std::fs::read(readback_path(&s)).unwrap(), img.payload);
        let boot = tcl_verb(&mut s, "boot_hw_device", &[]).unwrap();
        assert!(boot.contains("DONE=1") && boot.contains("mock device"), "{boot}");
        // The flash bytes live on the fixture board, not in the UI.
        let on_board = s.program.native.mock.as_ref().unwrap().with_board(|m| {
            m.helion_tap().unwrap().config_flash().unwrap().mem[..img.payload.len()].to_vec()
        });
        assert_eq!(on_board, Some(img.payload));
    }
}
