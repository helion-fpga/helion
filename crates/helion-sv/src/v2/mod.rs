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
pub mod parse;
pub mod pre;

use helion_ir::Design;

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
    let (flat, mut warnings) = elab::Elab::new(src).and_then(|e| e.elaborate(top)).map_err(fix)?;
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
    let mut defines = defines.to_vec();
    if !defines.iter().any(|d| d.0 == "SYNTHESIS") {
        defines.push(("SYNTHESIS".into(), String::new()));
    }
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || run(&sources, top.as_deref(), &defines, dirs))
        .map_err(|e| format!("cannot start the synthesis thread: {e}"))?
        .join()
        .map_err(|_| "internal error: synthesis panicked".to_string())?
}
