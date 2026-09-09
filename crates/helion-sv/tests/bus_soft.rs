use helion_ir::CellKind;
use helion_sv::synth_sv;
use std::fs;

#[test]
fn bus_arbiter_fold() {
    let src = r#"
module arb (
  input  wire host_req_i [1],
  output logic host_sel_req
);
  localparam int unsigned NumBitsHostSel = 1;
  always_comb begin
    host_sel_req = '0;
    for (integer host = 1 - 1; host >= 0; host = host - 1) begin
      if (host_req_i[host]) begin
        host_sel_req = NumBitsHostSel'(host);
      end
    end
  end
endmodule
"#;
    let d = synth_sv(src, "arb.sv").expect("arb");
    eprintln!("ARB cells={} anl={:?} wide={:?}", d.cells.len(), d.attrs.get("ASSIGN_NOT_LOWERED"), d.attrs.get("WIDE_CONE"));
    assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
    assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })));
}

#[test]
fn bus_time_with_index() {
    let src = r#"
module tidx2 (
  input  wire        host_req_i [1],
  input  wire [31:0] host_addr_i [1],
  input  wire [31:0] cfg_device_addr_mask [3],
  input  wire [31:0] cfg_device_addr_base [3],
  input  logic       host_sel_req,
  output logic       time_en
);
  always_comb begin
    time_en = host_req_i[host_sel_req] && ((host_addr_i[host_sel_req] & cfg_device_addr_mask[2]) == cfg_device_addr_base[2]);
  end
endmodule
"#;
    let d = synth_sv(src, "tidx2.sv").expect("tidx2");
    eprintln!("VARIDX cells={} anl={:?} wide={:?}", d.cells.len(), d.attrs.get("ASSIGN_NOT_LOWERED"), d.attrs.get("WIDE_CONE"));
    assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
    assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
    assert!(d.cells.len() > 10);
}

#[test]
fn bus_n3_soft_diag() {
    let src = fs::read_to_string("/tmp/bus_n3.sv").expect("src");
    let d = synth_sv(&src, "bus_n3.sv").expect("bus");
    eprintln!("BUS name={} cells={}", d.name, d.cells.len());
    for k in ["WIDE_CONE", "ASSIGN_NOT_LOWERED"] {
        if let Some(v) = d.attrs.get(k) {
            eprintln!("ATTR {k}={v}");
        }
    }
    let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
    eprintln!("luts={luts}");
    assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"), "bus should map device_sel");
    assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"), "bus assigns should lower");
    assert!(luts > 20, "expected mapped decode LUTs, got {luts}");
}

#[test]
fn bus_minimal_time_en() {
    let src = r#"
module bus_mini (
  input  wire        host_req_i,
  input  wire [31:0] host_addr_i,
  input  wire [31:0] cfg_device_addr_base [3],
  input  wire [31:0] cfg_device_addr_mask [3],
  output logic [1:0] device_sel_req,
  output logic       time_en,
  output logic       sim_en,
  output logic       ysyx_en
);
  always_comb begin
    time_en = host_req_i && ((host_addr_i & cfg_device_addr_mask[2]) == cfg_device_addr_base[2]);
    sim_en = host_req_i && ((host_addr_i & cfg_device_addr_mask[2]) != cfg_device_addr_base[2]) &&
              (((host_addr_i & cfg_device_addr_mask[0]) == cfg_device_addr_base[1]));
    ysyx_en = host_req_i & !time_en & !sim_en;
  end
  always_comb begin
    device_sel_req = 2'b0;
    if((host_addr_i & cfg_device_addr_mask[0]) != cfg_device_addr_base[0])
      device_sel_req = 2'(0);
    else if((host_addr_i & cfg_device_addr_mask[1]) == cfg_device_addr_base[1])
      device_sel_req = 2'(1);
    else if((host_addr_i & cfg_device_addr_mask[2]) == cfg_device_addr_base[2])
      device_sel_req = 2'(2);
  end
endmodule
"#;
    let d = synth_sv(src, "bus_mini.sv").expect("mini");
    assert_ne!(d.attrs.get("WIDE_CONE"), Some("1"));
    assert_ne!(d.attrs.get("ASSIGN_NOT_LOWERED"), Some("1"));
}
