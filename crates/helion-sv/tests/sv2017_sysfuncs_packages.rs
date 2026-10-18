use helion_ir::CellKind;
use helion_sv::synth_sv;
use std::path::PathBuf;

fn synth(sv: &str) -> helion_ir::Design {
    synth_sv(sv, "sv2017_sysfuncs.sv").expect("SV2017 synthesis must succeed")
}

// ============================================================================
// 1. `$bits`, `$clog2`, `$signed`, `$unsigned` in const and runtime contexts
// ============================================================================

#[test]
fn sv2017_bits_clog2_signed_unsigned_elaboration_and_runtime() {
    let sv = r#"
package types_pkg;
    typedef struct packed {
        logic [3:0] hi;
        logic [3:0] lo;
    } byte_pair_t;
endpackage

module bits_clog2_top (
    input logic clk,
    input logic [7:0] a,
    input logic [7:0] b,
    output logic [$bits(types_pkg::byte_pair_t)-1:0] sum_q,
    output logic [$clog2(16)-1:0] idx_q
);
    localparam int W_A = $bits(a);
    localparam int W_BYTE = $bits(byte);
    localparam int W_CONCAT = $bits({a, b});
    localparam int SIGNED_CONST = $unsigned($signed(8'd5));

    always_ff @(posedge clk) begin
        if (W_A == 8 && W_BYTE == 8 && W_CONCAT == 16 && SIGNED_CONST == 5) begin
            sum_q <= $unsigned($signed(a) + $signed(b));
            idx_q <= $clog2(W_CONCAT);
        end
    end
endmodule
"#;
    let d = synth(sv);
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(
        ffs,
        8 + 4,
        "sum_q (8 bits from $bits(byte_pair_t)) + idx_q (4 bits from $clog2(16)) must infer 12 HFFs, got {ffs}"
    );
}

// ============================================================================
// 2. `$onehot`, `$onehot0`, `$countones`, `$countbits`, `$isunknown`
// ============================================================================

#[test]
fn sv2017_onehot_onehot0_countones_countbits_isunknown_const_and_runtime() {
    let sv = r#"
module sysfunc_bitops_top (
    input logic clk,
    input logic [3:0] req,
    output logic oh_q,
    output logic oh0_q,
    output logic [2:0] ones_q,
    output logic [2:0] zeros_q,
    output logic const_ok_q
);
    localparam bit C_OH_PASS  = $onehot(4'b0100);
    localparam bit C_OH_FAIL  = $onehot(4'b0110);
    localparam bit C_OH0_ZERO = $onehot0(4'b0000);
    localparam int C_ONES     = $countones(8'b1011_0101);
    localparam int C_ZEROS    = $countbits(4'b1010, '0);
    localparam bit C_UNK_YES  = $isunknown(4'b10?0);
    localparam bit C_UNK_NO   = $isunknown(4'b1010);

    always_ff @(posedge clk) begin
        oh_q    <= $onehot(req);
        oh0_q   <= $onehot0(req);
        ones_q  <= $countones(req);
        zeros_q <= $countbits(req, '0);
        if (C_OH_PASS && !C_OH_FAIL && C_OH0_ZERO && C_ONES == 5 && C_ZEROS == 2 && C_UNK_YES && !C_UNK_NO) begin
            const_ok_q <= !$isunknown(req);
        end
    end
endmodule
"#;
    let d = synth(sv);
    assert_ne!(
        d.attrs.get("FLATTEN_CAP"),
        Some("1"),
        "$onehot / $countones / $countbits must lower as pure boolean cones without FLATTEN_CAP"
    );
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert_eq!(
        ffs,
        1 + 1 + 3 + 3 + 1,
        "oh_q(1) + oh0_q(1) + ones_q(3) + zeros_q(3) + const_ok_q(1) must infer 9 HFFs, got {ffs}"
    );
    assert!(luts >= 8, "bit-ops system functions must map LUT6 cones, got {luts}");
}

// ============================================================================
// 3. Cross-package functions (`pkg::func(...)` and `import pkg::*;`)
// ============================================================================

#[test]
fn sv2017_cross_package_functions_in_const_and_runtime_expressions() {
    let sv = r#"
package math_pkg;
    localparam logic [7:0] MASK = 8'hA5;

    function automatic int double_w(input int w);
        return w + w;
    endfunction

    function automatic logic [7:0] mix_byte(input logic [7:0] x, input logic [7:0] y);
        return (x ^ y) ^ MASK;
    endfunction
endpackage

module pkg_fn_top
    import math_pkg::*;
(
    input logic clk,
    input logic [7:0] a,
    input logic [7:0] b,
    output logic [math_pkg::double_w(4)-1:0] out_explicit_q,
    output logic [double_w(4)-1:0] out_imported_q
);
    always_ff @(posedge clk) begin
        out_explicit_q <= math_pkg::mix_byte(a, b);
        out_imported_q <= mix_byte(a, b);
    end
endmodule
"#;
    let d = synth(sv);
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert_eq!(
        ffs, 16,
        "out_explicit_q (8) + out_imported_q (8) must infer 16 HFFs, got {ffs}"
    );
    assert!(
        luts >= 8,
        "inlined package function mix_byte must synthesize LUT6 cones, got {luts}"
    );
}

// ============================================================================
// 4. `$readmemh` / `$readmemb` BRAM and LUT-ROM initialization
// ============================================================================

#[test]
fn sv2017_readmemh_and_readmemb_bram_and_lutrom_initialization() {
    let mut dir: PathBuf = std::env::temp_dir();
    dir.push(format!("helion_sv2017_readmem_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");

    std::fs::write(dir.join("rom_hex.mem"), "@0\nA5\n3C\nF0\n0F\n").expect("write hex mem");
    std::fs::write(dir.join("rom_bin.mem"), "1010_0101\n0011_1100\n1111_0000\n0000_1111\n")
        .expect("write bin mem");

    let sv_path = dir.join("rom_top.sv");
    let sv = r#"
module rom_top #(
    parameter HEX_FILE = "rom_hex.mem"
) (
    input logic clk,
    input logic [1:0] addr,
    output logic [7:0] hex_q,
    output logic [7:0] bin_q
);
    logic [7:0] rom_h [0:3];
    logic [7:0] rom_b [0:3];

    initial begin
        $readmemh(HEX_FILE, rom_h, 0, 3);
        $readmemb("rom_bin.mem", rom_b);
    end

    always_ff @(posedge clk) begin
        hex_q <= rom_h[addr];
        bin_q <= rom_b[addr];
    end
endmodule
"#;
    std::fs::write(&sv_path, sv).expect("write sv");

    let d = helion_sv::synth_sv_path(&sv_path).expect("synth rom_top");
    let brams: Vec<_> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Bram18 | CellKind::Bram18e1 { .. }))
        .collect();
    assert_eq!(brams.len(), 2, "rom_h and rom_b must infer 2 BRAM18 cells");
    for b in &brams {
        let init = b.attrs.get("INIT").unwrap_or("");
        assert!(
            init.contains("a5") && init.contains("3c") && init.contains("f0") && init.contains("f"),
            "BRAM INIT must contain loaded words, got {init}"
        );
    }
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(ffs, 16, "hex_q (8) + bin_q (8) LUT-ROM readback must infer 16 HFFs, got {ffs}");

    let _ = std::fs::remove_dir_all(&dir);
}
