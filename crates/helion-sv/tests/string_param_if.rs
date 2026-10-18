use helion_sv::synth_sv;

fn cell_count(sv: &str) -> usize {
    let design = synth_sv(sv, "test.sv").expect("synth");
    design.cells.len()
}

#[test]
fn string_param_if_folds_to_else_branch() {
    // S="MINI", so (S == "NONE") is false, else branch taken.
    // The else branch has the shift register, must produce FFs.
    let sv = r#"
module str_test #(parameter S = "MINI") (
  input wire clk,
  input wire a,
  output reg [31:0] r
);
  always @(posedge clk) begin
    if (S == "NONE") begin
      r <= 32'd1;
    end else begin
      r <= {a, r[31:1]};
    end
  end
endmodule
"#;
    let n = cell_count(sv);
    assert!(n > 0, "expected nonzero cells, got {}", n);
}

#[test]
fn string_param_if_folds_to_then_branch() {
    // S="NONE", so (S == "NONE") is true, then branch taken.
    let sv = r#"
module str_test2 #(parameter S = "NONE") (
  input wire clk,
  input wire a,
  output reg [31:0] r
);
  always @(posedge clk) begin
    if (S == "NONE") begin
      r <= {a, r[31:1]};
    end else begin
      r <= 32'd1;
    end
  end
endmodule
"#;
    let n = cell_count(sv);
    assert!(n > 0, "expected nonzero cells, got {}", n);
}

#[test]
fn string_param_ne_folds() {
    // S="MINI", so (S != "NONE") is true.
    let sv = r#"
module str_test3 #(parameter S = "MINI") (
  input wire clk,
  input wire a,
  output reg [31:0] r
);
  always @(posedge clk) begin
    if (S != "NONE") begin
      r <= {a, r[31:1]};
    end else begin
      r <= 32'd1;
    end
  end
endmodule
"#;
    let n = cell_count(sv);
    assert!(n > 0, "expected nonzero cells, got {}", n);
}
