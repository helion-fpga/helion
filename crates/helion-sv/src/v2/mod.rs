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

use helion_ir::Design;

/// Result of a synthesis run.
pub struct Synth {
    pub design: Design,
    pub luts: usize,
    pub ffs: usize,
    /// LUT levels on the longest register-to-register or pad path.
    pub depth: u32,
    pub warnings: Vec<String>,
}

fn run(text: &str, top: Option<&str>) -> Result<Synth, String> {
    let src = parse::parse(text)?;
    let (flat, mut warnings) = elab::Elab::new(src)?.elaborate(top)?;
    let (mut design, st, undriven) = lower::lower(&flat)?;
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
    Ok(Synth { design, luts: st.luts, ffs: st.ffs, depth: st.depth, warnings })
}

/// Synthesize preprocessed SystemVerilog. `top` names the top module; by
/// default it is the last module nothing else instantiates.
///
/// Deep expressions recurse deeply, so the work runs on its own thread
/// with a large stack.
pub fn synth(text: &str, top: Option<&str>) -> Result<Synth, String> {
    let text = text.to_string();
    let top = top.map(|s| s.to_string());
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || run(&text, top.as_deref()))
        .map_err(|e| format!("cannot start the synthesis thread: {e}"))?
        .join()
        .map_err(|_| "internal error: synthesis panicked".to_string())?
}
