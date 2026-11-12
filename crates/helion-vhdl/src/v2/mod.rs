//! Helion's VHDL front end: parse, elaborate and symbolically execute the
//! synthesizable subset of VHDL-2008, then hand flat SystemVerilog to
//! `helion_sv`. Anything outside the supported subset is a hard error with
//! a line number; nothing is silently dropped.

pub mod ast;
pub mod dag;
pub mod elab;
pub mod emit;
pub mod exec;
pub mod expr;
pub mod lex;
pub mod parse;

pub use dag::set_carry_arith;

/// Translate VHDL source to flat SystemVerilog. `top` picks the top-level
/// entity; by default the last entity that no other entity instantiates.
pub fn translate(src: &str, top: Option<&str>) -> Result<String, String> {
    Ok(translate_full(src, top)?.0)
}

/// Like [`translate`], also returning (VHDL name, SV name) pairs for
/// top-level names that had to be renamed because they are SV keywords.
pub fn translate_full(
    src: &str,
    top: Option<&str>,
) -> Result<(String, Vec<(String, String)>), String> {
    let units = parse::parse(src)?;
    let mut e = elab::Elab::new();
    e.load(units)?;
    let t = e.pick_top(top)?;
    e.elab_top(&t)?;
    let sv = e.emit_sv()?;
    Ok((sv, e.renames))
}
