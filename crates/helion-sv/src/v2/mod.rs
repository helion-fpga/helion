//! Second-generation SystemVerilog synthesis.
//!
//! Parse, elaborate and symbolically execute the synthesizable subset into a
//! word-level graph with exact widths and signedness, then bit-blast and map
//! to LUTs. Anything outside the subset is an error naming the construct and
//! its line; nothing is dropped or approximated.

pub mod ast;
pub mod dag;
pub mod elab;
pub mod exec;
pub mod lex;
pub mod lower;
pub mod mem;
pub mod parse;
pub mod pre;
pub mod srl;

use helion_ir::Design;
use std::sync::atomic::{AtomicBool, Ordering};

static HARD: AtomicBool = AtomicBool::new(false);

/// Build hard blocks for the Xilinx 7-series back end: block RAM from
/// memories, device primitives (`RAMB36E1`, `DSP48E1`, ...) from their
/// instances (see [`mem`]) and shift registers as SRLs (see [`srl`]). Off by default (the own-fabric flow has none);
/// `HELION_V2_HARD=1` turns it on too, `HELION_V2_HARD=0` keeps it off.
pub fn set_hard(on: bool) {
    HARD.store(on, Ordering::Relaxed);
}

pub fn hard() -> bool {
    match std::env::var("HELION_V2_HARD") {
        Ok(v) => v != "0",
        Err(_) => HARD.load(Ordering::Relaxed),
    }
}

/// Result of a synthesis run.
pub struct Synth {
    pub design: Design,
    pub luts: usize,
    pub ffs: usize,
    /// LUT levels on the longest register-to-register or pad path.
    pub depth: u32,
    pub warnings: Vec<String>,
    /// Every flop with the source register bit it holds.
    pub regmap: Vec<lower::RegMap>,
}

fn run(sources: &[(String, String)], top: Option<&str>, defines: &[(String, String)], dirs: Vec<std::path::PathBuf>) -> Result<Synth, String> {
    let mut pre = pre::Pre::new(dirs, defines);
    let mut text = String::new();
    for (name, src) in sources {
        if !text.is_empty() {
            text.push_str(&format!("\n`line 1 {}\n", pre.files.len()));
        }
        pre.run(src, name, &mut text)?;
    }
    if let Ok(path) = std::env::var("HELION_V2_DUMP") {
        let _ = std::fs::write(path, &text);
    }
    let files = pre.files.clone();
    let fix = |e: String| pre::decode_lines(&e, &files);
    let src = parse::parse(&text).map_err(fix)?;
    let dirs: Vec<std::path::PathBuf> = sources.iter().filter_map(|(n, _)| std::path::Path::new(n).parent().map(|p| p.to_path_buf())).collect();
    let mut no_bram = std::collections::HashSet::new();
    let mut flops = Vec::new();
    let (flat, mut warnings) = loop {
        let mut e = elab::Elab::new(src.clone()).map_err(fix)?;
        e.mem.on = hard();
        e.mem.no_bram = no_bram.clone();
        e.src_dirs = dirs.clone();
        match e.elaborate(top) {
            // A memory that cannot be block RAM: again, with it in flops.
            Err(m) if m.starts_with(mem::RETRY) => {
                let (name, why) = m[mem::RETRY.len()..].split_once('\u{1}').unwrap_or((&m, ""));
                flops.push(format!("memory '{name}' is built from flops, not block RAM: {why}"));
                no_bram.insert(name.to_string());
            }
            r => break r.map_err(fix)?,
        }
    };
    warnings.extend(flops);
    let (mut design, mut st, undriven) = lower::lower(&flat)?;
    if hard() && srl::on() {
        srl::infer(&mut design, &mut st);
    }
    // A module whose outputs are all left undriven has no logic: that is
    // a mistake in the source, not a design to build.
    let out_bits: Vec<String> = flat
        .ports
        .iter()
        .filter(|p| p.dir == helion_ir::PortDir::Out)
        .flat_map(|p| (0..p.w).map(move |i| lower::bit_name(&p.name, p.w, i)))
        .collect();
    if !out_bits.is_empty() && out_bits.iter().all(|b| undriven.contains(b)) {
        return Err(format!("module '{}' drives none of its outputs: there is no logic to build", flat.top));
    }
    for w in warnings.iter_mut() {
        *w = fix(std::mem::take(w));
    }
    if !undriven.is_empty() {
        let shown: Vec<&str> = undriven.iter().take(8).map(|s| s.as_str()).collect();
        warnings.push(format!(
            "{} signal bit(s) are read but never driven and read 0: {}{}",
            undriven.len(),
            shown.join(", "),
            if undriven.len() > shown.len() { ", ..." } else { "" }
        ));
    }
    design.attrs.set("FRONTEND", "v2");
    for (name, module, conns) in &flat.instances {
        design.instances.push(helion_ir::Instance {
            name: helion_ir::intern(name),
            module: helion_ir::intern(module),
            conns: conns.iter().map(|(p, n)| (helion_ir::intern(p), helion_ir::intern(n))).collect(),
            attrs: helion_ir::Attrs::default(),
        });
    }
    Ok(Synth { design, luts: st.luts, ffs: st.ffs, depth: st.depth, warnings, regmap: st.regmap })
}

/// Synthesize SystemVerilog sources (name, text). `top` names the top
/// module; by default it is the uninstantiated module with the largest
/// hierarchy. `defines` are macros defined before the first file;
/// `SYNTHESIS` always is. `dirs` are searched for include files after the
/// including file's own directory.
///
/// Deep expressions recurse deeply, so the work runs on its own thread
/// with a large stack.
pub fn synth(sources: &[(String, String)], top: Option<&str>, defines: &[(String, String)], dirs: Vec<std::path::PathBuf>) -> Result<Synth, String> {
    let sources = sources.to_vec();
    let top = top.map(|s| s.to_string());
    let mut defines = defines.to_vec();
    if !defines.iter().any(|d| d.0 == "SYNTHESIS") {
        defines.push(("SYNTHESIS".into(), String::new()));
    }
    let carry = dag::carry_arith();
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            dag::set_carry_arith(carry);
            run(&sources, top.as_deref(), &defines, dirs)
        })
        .map_err(|e| format!("cannot start the synthesis thread: {e}"))?
        .join()
        .map_err(|_| "internal error: synthesis panicked".to_string())?
}
