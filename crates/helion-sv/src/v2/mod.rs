//! Second-generation SystemVerilog synthesis.
//!
//! Parse, elaborate and symbolically execute the synthesizable subset into a
//! word-level graph with exact widths and signedness, then bit-blast and map
//! to LUTs. Anything outside the subset is an error naming the construct and
//! its line; nothing is dropped or approximated.
//!
//! `initial` blocks run at elaboration (`exec.rs`, `Mode::Init`): what they
//! leave in a variable is its power-up value (flop INIT, LUT memory and
//! block RAM contents, or a constant when nothing else drives it).
//! `$readmemh` / `$readmemb` (`memfile.rs`) read files next to the source
//! file, then in the include directories. A value that is not constant at
//! elaboration is an error with its line.

pub mod ast;
pub mod dag;
pub mod elab;
pub mod exec;
pub mod lex;
pub mod lower;
pub mod memfile;
pub mod parse;
pub mod pre;

use helion_ir::Design;
use std::cell::Cell;

thread_local! {
    static HARD: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Opt in to (`Some(true)`) or out of (`Some(false)`) block RAM and
/// multiplier inference for synthesis started from the calling thread;
/// `None` goes back to the default, which is `HELION_V2_HARD=1` in the
/// environment, else off. With it on:
///
/// - a multiplication whose operands are 16-bit values (zero-extended, or
///   both sign-extended) becomes a `Mac27` (`P = A * B`, `SIGNED` for
///   signed operands; a result wider than 32 bits is P extended);
///   a product of wider (or mixed-sign) variables takes one `Mac27` per
///   pair of 16-bit operand pieces whose product reaches the result (at
///   most 16), summed with adders: an operand zero-extended from n bits is
///   ceil(n/16) unsigned pieces, one sign-extended from n bits ceil(n/16)
///   pieces of which the top one is signed, so a signed n x m product
///   takes ceil(n/16) * ceil(m/16) blocks; a block of an unsigned and a
///   signed piece is `SIGNED` with the unsigned piece's bit 15 times the
///   signed one added in logic;
/// - a product by a constant is never a block: it is shift-and-add in
///   logic (as without the option);
/// - `a * b + c` (a one-block product and an addend of its width, at most
///   32 bits) puts `c` on the block's `C`;
/// - a register of at most 32 bits, powering up at 0, without
///   asynchronous reset, whose next value is a one-block product (or
///   product plus addend), 0 or itself, becomes the block's `P` register
///   (`PREG=1`; the conditions for the product are `CE`, those for 0
///   `RST`);
/// - a memory of a power of two words, indexed from 0 with addresses as
///   wide as its depth, written by one nonblocking `mem[waddr] <= data`
///   (under any condition) and read only by registers `q <= mem[raddr]`
///   (under any condition), all in one rising-edge process family without
///   reset, becomes `Bram18`s: per read register one set of blocks (all
///   written alike), each set one block per 1024 words deep (the high
///   address bits enable the write, and the read's high bits, registered
///   with it, choose the output) and per 16 bits wide; at most 64 blocks.
///   With one register, 1024 words and 16 bits it is one block whose `DO`
///   is the register.
///   A ROM (a memory nothing writes, its contents from an initializer,
///   an `initial` loop or `$readmemh`) read the same way becomes blocks
///   too, their write enable off.
///
/// Anything else is built from LUTs and flops as before. The cells need a
/// part with BRAM / DSP sites; v2 does not know the part, hence the opt-in.
pub fn set_hard_blocks(on: Option<bool>) {
    HARD.with(|h| h.set(on));
}

/// Whether the synthesis running on this thread infers hard blocks.
pub(crate) fn hard_blocks() -> bool {
    HARD.with(|h| h.get()).unwrap_or(false)
}

fn hard_requested() -> bool {
    HARD.with(|h| h.get()).unwrap_or_else(|| std::env::var("HELION_V2_HARD").is_ok_and(|v| v == "1"))
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
    let inc_dirs = dirs.clone();
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
    let (flat, mut warnings) = elab::Elab::new(src)
        .and_then(|mut e| {
            e.src_files = files.clone();
            e.inc_dirs = inc_dirs;
            e.elaborate(top)
        })
        .map_err(fix)?;
    let (mut design, st, undriven) = lower::lower(&flat)?;
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
    let hard = hard_requested();
    let mut defines = defines.to_vec();
    if !defines.iter().any(|d| d.0 == "SYNTHESIS") {
        defines.push(("SYNTHESIS".into(), String::new()));
    }
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            set_hard_blocks(Some(hard));
            run(&sources, top.as_deref(), &defines, dirs)
        })
        .map_err(|e| format!("cannot start the synthesis thread: {e}"))?
        .join()
        .map_err(|_| "internal error: synthesis panicked".to_string())?
}
