//! HDL + PCF -> iCE40-HX8K bitstream (`.asc`, and `.bin` through icepack).
//!
//! ```text
//! cargo run --release -p helion-ice40 --example icebit -- \
//!     design.sv [more.sv ...] --pcf board.pcf [--top name] [-o out/design] [--no-bin]
//! ```
//!
//! Synthesis is the v2 front end (`helion_sv::synth_sv_v2_files`); the
//! PCF's `set_io` lines place the ports (`clk`, `led[3]`, ...). Writes
//! `<out>.asc` and, unless `--no-bin`, `<out>.bin` with IceStorm's icepack
//! (found through `HELION_ICEPACK` or `PATH`; missing icepack is an error).

use helion_backend::Backend;
use helion_ice40::{io::bind_pcf, pcf::parse_pcf, Ice40Backend};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn find_tool(name: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(format!("HELION_{}", name.to_ascii_uppercase())) {
        return Some(PathBuf::from(p));
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut files: Vec<PathBuf> = Vec::new();
    let (mut pcf, mut top, mut out, mut bin) = (None, None, None, true);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pcf" => pcf = it.next().cloned(),
            "--top" => top = it.next().cloned(),
            "-o" | "--out" => out = it.next().cloned(),
            "--no-bin" => bin = false,
            "-h" | "--help" => {
                return Err("usage: icebit <design.sv>... --pcf <board.pcf> [--top name] [-o out] [--no-bin]".into());
            }
            f if f.starts_with('-') => return Err(format!("unknown option {f}")),
            f => files.push(PathBuf::from(f)),
        }
    }
    let pcf = pcf.ok_or("missing --pcf <board.pcf>")?;
    if files.is_empty() {
        return Err("no HDL files given".into());
    }
    let out = PathBuf::from(out.unwrap_or_else(|| files[0].file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "design".into())));

    let refs: Vec<&Path> = files.iter().map(|p| p.as_path()).collect();
    let synth = helion_sv::synth_sv_v2_files(&refs, top.as_deref())?;
    for w in &synth.warnings {
        eprintln!("warning: {w}");
    }
    eprintln!("synth: {} LUTs, {} flops, {} LUT levels", synth.luts, synth.ffs, synth.depth);
    let mut design = synth.design;

    let pcf_text = std::fs::read_to_string(&pcf).map_err(|e| format!("{pcf}: {e}"))?;
    let cons = parse_pcf(&pcf_text, &pcf)?;
    let binding = bind_pcf(&mut design, &cons)?;
    for c in &binding.unused {
        eprintln!("warning: {pcf}:{}: set_io {} {} matches no pad of the design", c.line, c.port, c.pin);
    }
    eprintln!("io: {} pads, global clocks: {}", binding.pads.len(), if binding.clocks.is_empty() { "none".into() } else { binding.clocks.join(", ") });

    let b = Ice40Backend::new("ice40-hx8k");
    let packed = b.pack(&design)?;
    eprintln!("pack: {} LUT4, {} FF, {} IO, {} RAM", packed.luts.len(), packed.ffs.len(), packed.ios.len(), packed.rams.len());
    let placed = b.place(&packed)?;
    eprintln!("place: {} LCs", placed.luts.len());
    let routed = b.route(&placed)?;
    eprintln!("route: {} nets", routed.nets.len());
    let asc = b.bitstream(&routed)?;

    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let asc_path = out.with_extension("asc");
    std::fs::write(&asc_path, &asc).map_err(|e| format!("{}: {e}", asc_path.display()))?;
    eprintln!("wrote {}", asc_path.display());
    if bin {
        let icepack = find_tool("icepack").ok_or("icepack not found (install Project IceStorm or set HELION_ICEPACK); use --no-bin for the .asc only")?;
        let bin_path = out.with_extension("bin");
        let st = Command::new(&icepack).arg(&asc_path).arg(&bin_path).status().map_err(|e| format!("{}: {e}", icepack.display()))?;
        if !st.success() {
            return Err(format!("icepack failed on {}", asc_path.display()));
        }
        eprintln!("wrote {}", bin_path.display());
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("icebit: {e}");
            ExitCode::FAILURE
        }
    }
}
