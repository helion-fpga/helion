//! The Helion UVM library: a documented subset of IEEE 1800.2 written in
//! SystemVerilog (see the header of `uvm_pkg.sv` for what is and is not
//! implemented). It is compiled ahead of any testbench that mentions
//! `uvm_pkg` / `uvm_macros.svh`.

/// `uvm_macros.svh`.
pub const UVM_MACROS: &str = include_str!("uvm_macros.svh");
/// `uvm_pkg.sv`.
pub const UVM_PKG: &str = include_str!("uvm_pkg.sv");
