//! Synthesizable SystemVerilog RTL generators for the Vivado UG994 Parametric IP Catalog.

use crate::model::{IpComponent, PortDirection};
use std::collections::BTreeMap;

/// Dispatch synthesizable SystemVerilog generation for any `IpComponent`.
pub fn generate_component_sv(
    comp: &IpComponent,
    overrides: &BTreeMap<String, String>,
) -> Result<String, String> {
    match comp.core.name.as_str() {
        "axi_interconnect" | "axi_crossbar" => {
            let m = comp.get_param_i64(overrides, "C_NUM_MASTERS").max(1) as u32;
            let s = comp.get_param_i64(overrides, "C_NUM_SLAVES").max(1) as u32;
            let addr_w = comp.get_param_i64(overrides, "C_ADDR_WIDTH").max(8) as u32;
            let data_w = comp.get_param_i64(overrides, "C_DATA_WIDTH").max(8) as u32;
            let mut regions = Vec::new();
            for idx in 0..s {
                let base_key = format!("C_M{idx:02}_BASE_ADDR");
                let width_key = format!("C_M{idx:02}_ADDR_WIDTH");
                let base = if overrides.contains_key(&base_key)
                    || overrides.contains_key(&format!("CONFIG.{base_key}"))
                {
                    comp.get_param_i64(overrides, &base_key) as u64
                } else {
                    0x4000_0000 + (idx as u64) * 0x0001_0000
                };
                let aw = if overrides.contains_key(&width_key)
                    || overrides.contains_key(&format!("CONFIG.{width_key}"))
                {
                    comp.get_param_i64(overrides, &width_key).clamp(4, 32) as u32
                } else {
                    16
                };
                regions.push((base, 1u64 << aw));
            }
            Ok(generate_axi_interconnect_sv(
                &comp.core.name,
                m,
                s,
                addr_w,
                data_w,
                &regions,
            ))
        }
        "axi_protocol_converter" => {
            let addr_w = comp.get_param_i64(overrides, "C_ADDR_WIDTH").max(8) as u32;
            let data_w = comp.get_param_i64(overrides, "C_DATA_WIDTH").max(8) as u32;
            Ok(generate_axi_protocol_converter_sv(addr_w, data_w))
        }
        "axi_gpio" => {
            let gpio_w = comp.get_param_i64(overrides, "C_GPIO_WIDTH").max(1) as u32;
            let is_dual = comp.get_param_i64(overrides, "C_IS_DUAL") != 0;
            let has_irq = comp.get_param_i64(overrides, "C_INTERRUPT_PRESENT") != 0;
            Ok(generate_axi_gpio_sv(gpio_w, is_dual, has_irq))
        }
        "axi_uartlite" => {
            let clk_hz = comp.get_param_i64(overrides, "C_CLK_FREQ_HZ").max(1) as u32;
            let baud = comp.get_param_i64(overrides, "C_BAUDRATE").max(1) as u32;
            let data_bits = comp.get_param_i64(overrides, "C_DATA_BITS").clamp(5, 8) as u32;
            let use_parity = comp.get_param_i64(overrides, "C_USE_PARITY") != 0;
            let odd_parity = comp.get_param_i64(overrides, "C_ODD_PARITY") != 0;
            Ok(generate_axi_uartlite_sv(
                clk_hz, baud, data_bits, use_parity, odd_parity,
            ))
        }
        "axi_timer" => {
            let count_w = comp.get_param_i64(overrides, "C_COUNT_WIDTH").max(8) as u32;
            let one_only = comp.get_param_i64(overrides, "C_ONE_TIMER_ONLY") != 0;
            Ok(generate_axi_timer_sv(count_w, one_only))
        }
        "axi_bram_ctrl" => {
            let depth = comp.get_param_i64(overrides, "C_MEMORY_DEPTH").max(16) as u32;
            let addr_w = comp.get_param_i64(overrides, "C_S_AXI_ADDR_WIDTH").max(4) as u32;
            let data_w = comp.get_param_i64(overrides, "C_S_AXI_DATA_WIDTH").max(8) as u32;
            let internal_mem = comp.get_param_i64(overrides, "C_INTERNAL_MEM") != 0;
            Ok(generate_axi_bram_ctrl_sv(depth, addr_w, data_w, internal_mem))
        }
        "cdc_fifo" | "axis_data_fifo" => {
            let width = comp.get_param_i64(overrides, "C_TDATA_WIDTH").max(8) as u32;
            let depth = comp.get_param_i64(overrides, "C_FIFO_DEPTH").max(4) as u32;
            let is_async = comp.get_param_i64(overrides, "C_IS_ASYNC") != 0;
            Ok(generate_cdc_fifo_sv(width, depth, is_async))
        }
        "proc_sys_reset" => {
            let ext_w = comp.get_param_i64(overrides, "C_EXT_RST_WIDTH").max(2) as u32;
            let aux_w = comp.get_param_i64(overrides, "C_AUX_RST_WIDTH").max(2) as u32;
            let ext_hi = comp.get_param_i64(overrides, "C_EXT_RESET_HIGH") != 0;
            let aux_hi = comp.get_param_i64(overrides, "C_AUX_RESET_HIGH") != 0;
            Ok(generate_proc_sys_reset_sv(ext_w, aux_w, ext_hi, aux_hi))
        }
        "axi_cpu" => Ok(generate_axi_cpu_sv()),
        "axi4_master_cpu" => Ok(generate_axi4_master_cpu_sv()),
        "axi_quad_spi" => Ok(generate_axi_quad_spi_sv()),
        "axi_iic" => Ok(generate_axi_iic_sv()),
        "axi_intc" => Ok(generate_axi_intc_sv()),
        "blk_mem_gen" => Ok(generate_blk_mem_gen_sv()),
        "apb_gpio" => Ok(generate_apb_gpio_sv()),
        "wb_gpio" => Ok(generate_wb_gpio_sv()),
        _ => Ok(generate_generic_component_sv(comp)),
    }
}

/// Generate a synthesizable M-master x S-slave AXI4-Lite Interconnect / Crossbar with address decode & DECERR.
pub fn generate_axi_interconnect_sv(
    module_name: &str,
    num_masters: u32,
    num_slaves: u32,
    addr_width: u32,
    data_width: u32,
    slave_regions: &[(u64, u64)],
) -> String {
    let m = num_masters.max(1);
    let s = num_slaves.max(1);
    let strb_w = (data_width / 8).max(1);
    let mut sv = String::new();
    sv.push_str(&format!(
        "// Synthesizable {m}x{s} AXI4-Lite Interconnect Crossbar with Address Decoder & DECERR Responder\n"
    ));
    sv.push_str(&format!("module {module_name} #(\n"));
    sv.push_str(&format!("  parameter int C_NUM_MASTERS = {m},\n"));
    sv.push_str(&format!("  parameter int C_NUM_SLAVES  = {s},\n"));
    sv.push_str(&format!("  parameter int C_ADDR_WIDTH  = {addr_width},\n"));
    sv.push_str(&format!("  parameter int C_DATA_WIDTH  = {data_width}"));
    for idx in 0..s {
        let (base, range) = slave_regions
            .get(idx as usize)
            .copied()
            .unwrap_or((0x4000_0000 + (idx as u64) * 0x0001_0000, 0x0001_0000));
        let mask = !(range.saturating_sub(1)) & ((1u64 << addr_width.min(63)) - 1);
        sv.push_str(&format!(
            ",\n  parameter logic [C_ADDR_WIDTH-1:0] C_M{idx:02}_BASE_ADDR = {addr_width}'h{base:08X},\n  parameter logic [C_ADDR_WIDTH-1:0] C_M{idx:02}_ADDR_MASK = {addr_width}'h{mask:08X}"
        ));
    }
    sv.push_str("\n) (\n");
    sv.push_str("  input  logic aclk,\n");
    sv.push_str("  input  logic aresetn");
    for idx in 0..m {
        let p = format!("s{idx:02}_axi");
        sv.push_str(&format!(
            ",\n  input  logic [C_ADDR_WIDTH-1:0] {p}_awaddr,\n  input  logic [2:0]              {p}_awprot,\n  input  logic                    {p}_awvalid,\n  output logic                    {p}_awready,\n  input  logic [C_DATA_WIDTH-1:0] {p}_wdata,\n  input  logic [{strb_w}-1:0]            {p}_wstrb,\n  input  logic                    {p}_wvalid,\n  output logic                    {p}_wready,\n  output logic [1:0]              {p}_bresp,\n  output logic                    {p}_bvalid,\n  input  logic                    {p}_bready,\n  input  logic [C_ADDR_WIDTH-1:0] {p}_araddr,\n  input  logic [2:0]              {p}_arprot,\n  input  logic                    {p}_arvalid,\n  output logic                    {p}_arready,\n  output logic [C_DATA_WIDTH-1:0] {p}_rdata,\n  output logic [1:0]              {p}_rresp,\n  output logic                    {p}_rvalid,\n  input  logic                    {p}_rready"
        ));
    }
    for idx in 0..s {
        let p = format!("m{idx:02}_axi");
        sv.push_str(&format!(
            ",\n  output logic [C_ADDR_WIDTH-1:0] {p}_awaddr,\n  output logic [2:0]              {p}_awprot,\n  output logic                    {p}_awvalid,\n  input  logic                    {p}_awready,\n  output logic [C_DATA_WIDTH-1:0] {p}_wdata,\n  output logic [{strb_w}-1:0]            {p}_wstrb,\n  output logic                    {p}_wvalid,\n  input  logic                    {p}_wready,\n  input  logic [1:0]              {p}_bresp,\n  input  logic                    {p}_bvalid,\n  output logic                    {p}_bready,\n  output logic [C_ADDR_WIDTH-1:0] {p}_araddr,\n  output logic [2:0]              {p}_arprot,\n  output logic                    {p}_arvalid,\n  input  logic                    {p}_arready,\n  input  logic [C_DATA_WIDTH-1:0] {p}_rdata,\n  input  logic [1:0]              {p}_rresp,\n  input  logic                    {p}_rvalid,\n  output logic                    {p}_rready"
        ));
    }
    sv.push_str("\n);\n\n");

    let m_bits = (32 - m.saturating_sub(1).leading_zeros()).max(1);
    let s_bits = (32 - s.leading_zeros()).max(1);
    sv.push_str(&format!(
        "  localparam logic [1:0] AXI_RESP_DECERR = 2'b11;\n"
    ));
    sv.push_str(&format!(
        "  logic [{m_bits}-1:0] active_master;\n"
    ));
    sv.push_str("  logic bus_busy;\n\n");

    // Round-robin / priority master selection
    if m == 1 {
        sv.push_str("  assign active_master = '0;\n");
        sv.push_str("  assign bus_busy = 1'b0;\n\n");
    } else {
        sv.push_str("  always_ff @(posedge aclk or negedge aresetn) begin\n");
        sv.push_str("    if (!aresetn) begin\n");
        sv.push_str("      active_master <= '0;\n");
        sv.push_str("      bus_busy <= 1'b0;\n");
        sv.push_str("    end else begin\n");
        sv.push_str("      if (!bus_busy) begin\n");
        for idx in 0..m {
            let kw = if idx == 0 { "if" } else { "else if" };
            sv.push_str(&format!(
                "        {kw} (s{idx:02}_axi_awvalid || s{idx:02}_axi_arvalid) begin\n          active_master <= {m_bits}'d{idx};\n          bus_busy <= 1'b1;\n        end\n"
            ));
        }
        sv.push_str("      end else begin\n");
        for idx in 0..m {
            sv.push_str(&format!(
                "        if (active_master == {m_bits}'d{idx} && ((s{idx:02}_axi_bvalid && s{idx:02}_axi_bready) || (s{idx:02}_axi_rvalid && s{idx:02}_axi_rready))) bus_busy <= 1'b0;\n"
            ));
        }
        sv.push_str("      end\n    end\n  end\n\n");
    }

    // Multiplexed active master request signals
    sv.push_str(&format!(
        "  logic [C_ADDR_WIDTH-1:0] mux_awaddr, mux_araddr;\n  logic [2:0] mux_awprot, mux_arprot;\n  logic mux_awvalid, mux_awready;\n  logic [C_DATA_WIDTH-1:0] mux_wdata, mux_rdata;\n  logic [{strb_w}-1:0] mux_wstrb;\n  logic mux_wvalid, mux_wready;\n  logic [1:0] mux_bresp, mux_rresp;\n  logic mux_bvalid, mux_bready;\n  logic mux_arvalid, mux_arready;\n  logic mux_rvalid, mux_rready;\n\n"
    ));

    sv.push_str("  always_comb begin\n");
    sv.push_str("    mux_awaddr  = s00_axi_awaddr;\n    mux_awprot  = s00_axi_awprot;\n    mux_awvalid = s00_axi_awvalid;\n    mux_wdata   = s00_axi_wdata;\n    mux_wstrb   = s00_axi_wstrb;\n    mux_wvalid  = s00_axi_wvalid;\n    mux_bready  = s00_axi_bready;\n    mux_araddr  = s00_axi_araddr;\n    mux_arprot  = s00_axi_arprot;\n    mux_arvalid = s00_axi_arvalid;\n    mux_rready  = s00_axi_rready;\n");
    if m > 1 {
        sv.push_str("    case (active_master)\n");
        for idx in 0..m {
            let p = format!("s{idx:02}_axi");
            sv.push_str(&format!(
                "      {m_bits}'d{idx}: begin\n        mux_awaddr  = {p}_awaddr;\n        mux_awprot  = {p}_awprot;\n        mux_awvalid = {p}_awvalid;\n        mux_wdata   = {p}_wdata;\n        mux_wstrb   = {p}_wstrb;\n        mux_wvalid  = {p}_wvalid;\n        mux_bready  = {p}_bready;\n        mux_araddr  = {p}_araddr;\n        mux_arprot  = {p}_arprot;\n        mux_arvalid = {p}_arvalid;\n        mux_rready  = {p}_rready;\n      end\n"
            ));
        }
        sv.push_str("      default: ;\n    endcase\n");
    }
    sv.push_str("  end\n\n");

    // Address decoder for write (aw_sel) and read (ar_sel)
    sv.push_str(&format!(
        "  logic [{s}-1:0] aw_hit, ar_hit;\n  logic aw_decerr, ar_decerr;\n  logic decerr_bvalid, decerr_rvalid;\n\n"
    ));
    for idx in 0..s {
        sv.push_str(&format!(
            "  assign aw_hit[{idx}] = ((mux_awaddr & C_M{idx:02}_ADDR_MASK) == (C_M{idx:02}_BASE_ADDR & C_M{idx:02}_ADDR_MASK));\n"
        ));
        sv.push_str(&format!(
            "  assign ar_hit[{idx}] = ((mux_araddr & C_M{idx:02}_ADDR_MASK) == (C_M{idx:02}_BASE_ADDR & C_M{idx:02}_ADDR_MASK));\n"
        ));
    }
    sv.push_str("\n  assign aw_decerr = (mux_awvalid && (aw_hit == '0));\n");
    sv.push_str("  assign ar_decerr = (mux_arvalid && (ar_hit == '0));\n\n");

    // DECERR responder FSM for unmapped addresses
    sv.push_str("  always_ff @(posedge aclk or negedge aresetn) begin\n");
    sv.push_str("    if (!aresetn) begin\n");
    sv.push_str("      decerr_bvalid <= 1'b0;\n");
    sv.push_str("      decerr_rvalid <= 1'b0;\n");
    sv.push_str("    end else begin\n");
    sv.push_str("      if (aw_decerr && mux_wvalid && !decerr_bvalid) decerr_bvalid <= 1'b1;\n");
    sv.push_str("      else if (decerr_bvalid && mux_bready) decerr_bvalid <= 1'b0;\n");
    sv.push_str("      if (ar_decerr && !decerr_rvalid) decerr_rvalid <= 1'b1;\n");
    sv.push_str("      else if (decerr_rvalid && mux_rready) decerr_rvalid <= 1'b0;\n");
    sv.push_str("    end\n  end\n\n");

    // Drive slave ports M00_AXI .. M{S-1}_AXI
    for idx in 0..s {
        let p = format!("m{idx:02}_axi");
        sv.push_str(&format!(
            "  assign {p}_awaddr  = mux_awaddr;\n  assign {p}_awprot  = mux_awprot;\n  assign {p}_awvalid = mux_awvalid && aw_hit[{idx}];\n  assign {p}_wdata   = mux_wdata;\n  assign {p}_wstrb   = mux_wstrb;\n  assign {p}_wvalid  = mux_wvalid  && aw_hit[{idx}];\n  assign {p}_bready  = mux_bready  && aw_hit[{idx}];\n  assign {p}_araddr  = mux_araddr;\n  assign {p}_arprot  = mux_arprot;\n  assign {p}_arvalid = mux_arvalid && ar_hit[{idx}];\n  assign {p}_rready  = mux_rready  && ar_hit[{idx}];\n\n"
        ));
    }

    // Response mux back to active master
    sv.push_str("  always_comb begin\n");
    sv.push_str("    mux_awready = aw_decerr ? 1'b1 : 1'b0;\n");
    sv.push_str("    mux_wready  = aw_decerr ? 1'b1 : 1'b0;\n");
    sv.push_str("    mux_bresp   = decerr_bvalid ? AXI_RESP_DECERR : 2'b00;\n");
    sv.push_str("    mux_bvalid  = decerr_bvalid;\n");
    sv.push_str("    mux_arready = ar_decerr ? 1'b1 : 1'b0;\n");
    sv.push_str("    mux_rdata   = '0;\n");
    sv.push_str("    mux_rresp   = decerr_rvalid ? AXI_RESP_DECERR : 2'b00;\n");
    sv.push_str("    mux_rvalid  = decerr_rvalid;\n");
    for idx in 0..s {
        let p = format!("m{idx:02}_axi");
        sv.push_str(&format!(
            "    if (aw_hit[{idx}]) begin\n      mux_awready = {p}_awready;\n      mux_wready  = {p}_wready;\n      mux_bresp   = {p}_bresp;\n      mux_bvalid  = {p}_bvalid;\n    end\n"
        ));
        sv.push_str(&format!(
            "    if (ar_hit[{idx}]) begin\n      mux_arready = {p}_arready;\n      mux_rdata   = {p}_rdata;\n      mux_rresp   = {p}_rresp;\n      mux_rvalid  = {p}_rvalid;\n    end\n"
        ));
    }
    sv.push_str("  end\n\n");

    for idx in 0..m {
        let p = format!("s{idx:02}_axi");
        let cond = if m == 1 {
            "1'b1".to_string()
        } else {
            format!("(active_master == {m_bits}'d{idx})")
        };
        sv.push_str(&format!(
            "  assign {p}_awready = {cond} ? mux_awready : 1'b0;\n  assign {p}_wready  = {cond} ? mux_wready  : 1'b0;\n  assign {p}_bresp   = mux_bresp;\n  assign {p}_bvalid  = {cond} ? mux_bvalid  : 1'b0;\n  assign {p}_arready = {cond} ? mux_arready : 1'b0;\n  assign {p}_rdata   = mux_rdata;\n  assign {p}_rresp   = mux_rresp;\n  assign {p}_rvalid  = {cond} ? mux_rvalid  : 1'b0;\n"
        ));
    }
    let _ = s_bits;
    sv.push_str("endmodule\n");
    sv
}

/// Generate a synthesizable AXI4 (burst) to AXI4-Lite (single-beat) protocol converter.
pub fn generate_axi_protocol_converter_sv(addr_width: u32, data_width: u32) -> String {
    let strb_w = (data_width / 8).max(1);
    format!(
        r#"// Synthesizable AXI4 to AXI4-Lite Protocol Converter Bridge
module axi_protocol_converter #(
  parameter int C_ADDR_WIDTH = {addr_width},
  parameter int C_DATA_WIDTH = {data_width}
) (
  input  logic                    aclk,
  input  logic                    aresetn,
  // S_AXI (AXI4 Slave)
  input  logic [C_ADDR_WIDTH-1:0] s_axi_awaddr,
  input  logic [7:0]              s_axi_awlen,
  input  logic [2:0]              s_axi_awsize,
  input  logic [1:0]              s_axi_awburst,
  input  logic [2:0]              s_axi_awprot,
  input  logic                    s_axi_awvalid,
  output logic                    s_axi_awready,
  input  logic [C_DATA_WIDTH-1:0] s_axi_wdata,
  input  logic [{strb_w}-1:0]            s_axi_wstrb,
  input  logic                    s_axi_wlast,
  input  logic                    s_axi_wvalid,
  output logic                    s_axi_wready,
  output logic [1:0]              s_axi_bresp,
  output logic                    s_axi_bvalid,
  input  logic                    s_axi_bready,
  input  logic [C_ADDR_WIDTH-1:0] s_axi_araddr,
  input  logic [7:0]              s_axi_arlen,
  input  logic [2:0]              s_axi_arsize,
  input  logic [1:0]              s_axi_arburst,
  input  logic [2:0]              s_axi_arprot,
  input  logic                    s_axi_arvalid,
  output logic                    s_axi_arready,
  output logic [C_DATA_WIDTH-1:0] s_axi_rdata,
  output logic [1:0]              s_axi_rresp,
  output logic                    s_axi_rlast,
  output logic                    s_axi_rvalid,
  input  logic                    s_axi_rready,
  // M_AXI (AXI4-Lite Master)
  output logic [C_ADDR_WIDTH-1:0] m_axi_awaddr,
  output logic [2:0]              m_axi_awprot,
  output logic                    m_axi_awvalid,
  input  logic                    m_axi_awready,
  output logic [C_DATA_WIDTH-1:0] m_axi_wdata,
  output logic [{strb_w}-1:0]            m_axi_wstrb,
  output logic                    m_axi_wvalid,
  input  logic                    m_axi_wready,
  input  logic [1:0]              m_axi_bresp,
  input  logic                    m_axi_bvalid,
  output logic                    m_axi_bready,
  output logic [C_ADDR_WIDTH-1:0] m_axi_araddr,
  output logic [2:0]              m_axi_arprot,
  output logic                    m_axi_arvalid,
  input  logic                    m_axi_arready,
  input  logic [C_DATA_WIDTH-1:0] m_axi_rdata,
  input  logic [1:0]              m_axi_rresp,
  input  logic                    m_axi_rvalid,
  output logic                    m_axi_rready
);
  logic [7:0] rd_beats_rem;

  always_ff @(posedge aclk or negedge aresetn) begin
    if (!aresetn) begin
      rd_beats_rem <= 8'd0;
    end else if (s_axi_arvalid && s_axi_arready) begin
      rd_beats_rem <= s_axi_arlen;
    end else if (s_axi_rvalid && s_axi_rready && rd_beats_rem != 8'd0) begin
      rd_beats_rem <= rd_beats_rem - 8'd1;
    end
  end

  assign m_axi_awaddr  = s_axi_awaddr;
  assign m_axi_awprot  = s_axi_awprot;
  assign m_axi_awvalid = s_axi_awvalid;
  assign s_axi_awready = m_axi_awready;

  assign m_axi_wdata   = s_axi_wdata;
  assign m_axi_wstrb   = s_axi_wstrb;
  assign m_axi_wvalid  = s_axi_wvalid;
  assign s_axi_wready  = m_axi_wready;

  assign s_axi_bresp   = m_axi_bresp;
  assign s_axi_bvalid  = m_axi_bvalid;
  assign m_axi_bready  = s_axi_bready;

  assign m_axi_araddr  = s_axi_araddr;
  assign m_axi_arprot  = s_axi_arprot;
  assign m_axi_arvalid = s_axi_arvalid;
  assign s_axi_arready = m_axi_arready;

  assign s_axi_rdata   = m_axi_rdata;
  assign s_axi_rresp   = m_axi_rresp;
  assign s_axi_rlast   = (rd_beats_rem == 8'd0);
  assign s_axi_rvalid  = m_axi_rvalid;
  assign m_axi_rready  = s_axi_rready;
endmodule
"#
    )
}

/// Generate a synthesizable AXI GPIO peripheral matching Xilinx `axi_gpio`.
pub fn generate_axi_gpio_sv(gpio_width: u32, is_dual: bool, interrupt_present: bool) -> String {
    let dual_val = u32::from(is_dual);
    let irq_val = u32::from(interrupt_present);
    format!(
        r#"// Synthesizable AXI4-Lite GPIO Controller (Vivado axi_gpio equivalent)
module axi_gpio #(
  parameter int C_GPIO_WIDTH         = {gpio_width},
  parameter int C_IS_DUAL            = {dual_val},
  parameter int C_INTERRUPT_PRESENT  = {irq_val},
  parameter int C_S_AXI_ADDR_WIDTH   = 9,
  parameter int C_S_AXI_DATA_WIDTH   = 32
) (
  input  logic                          s_axi_aclk,
  input  logic                          s_axi_aresetn,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_awaddr,
  input  logic [2:0]                    s_axi_awprot,
  input  logic                          s_axi_awvalid,
  output logic                          s_axi_awready,
  input  logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_wdata,
  input  logic [3:0]                    s_axi_wstrb,
  input  logic                          s_axi_wvalid,
  output logic                          s_axi_wready,
  output logic [1:0]                    s_axi_bresp,
  output logic                          s_axi_bvalid,
  input  logic                          s_axi_bready,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_araddr,
  input  logic [2:0]                    s_axi_arprot,
  input  logic                          s_axi_arvalid,
  output logic                          s_axi_arready,
  output logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_rdata,
  output logic [1:0]                    s_axi_rresp,
  output logic                          s_axi_rvalid,
  input  logic                          s_axi_rready,
  input  logic [C_GPIO_WIDTH-1:0]       gpio_io_i,
  output logic [C_GPIO_WIDTH-1:0]       gpio_io_o,
  output logic [C_GPIO_WIDTH-1:0]       gpio_io_t,
  output logic                          ip2intc_irpt
);
  logic [C_GPIO_WIDTH-1:0] gpio_data_reg, gpio_tri_reg;
  logic [C_GPIO_WIDTH-1:0] gpio2_data_reg, gpio2_tri_reg;
  (* ASYNC_REG = "TRUE" *) logic [C_GPIO_WIDTH-1:0] gpio_sync_s1, gpio_sync_s2, gpio_prev;
  logic gier_reg;
  logic [1:0] ip_ier_reg, ip_isr_reg;

  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      gpio_sync_s1 <= '0;
      gpio_sync_s2 <= '0;
      gpio_prev    <= '0;
    end else begin
      gpio_sync_s1 <= gpio_io_i;
      gpio_sync_s2 <= gpio_sync_s1;
      gpio_prev    <= gpio_sync_s2;
    end
  end

  assign gpio_io_o    = gpio_data_reg;
  assign gpio_io_t    = gpio_tri_reg;
  assign ip2intc_irpt = (C_INTERRUPT_PRESENT != 0) && gier_reg && |(ip_isr_reg & ip_ier_reg);

  assign s_axi_awready = s_axi_awvalid && s_axi_wvalid && (!s_axi_bvalid || s_axi_bready);
  assign s_axi_wready  = s_axi_awready;
  assign s_axi_bresp   = 2'b00;
  assign s_axi_arready = s_axi_arvalid && (!s_axi_rvalid || s_axi_rready);
  assign s_axi_rresp   = 2'b00;

  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      gpio_data_reg  <= '0;
      gpio_tri_reg   <= '1;
      gpio2_data_reg <= '0;
      gpio2_tri_reg  <= '1;
      gier_reg       <= 1'b0;
      ip_ier_reg     <= 2'b00;
      ip_isr_reg     <= 2'b00;
      s_axi_bvalid   <= 1'b0;
      s_axi_rvalid   <= 1'b0;
      s_axi_rdata    <= '0;
    end else begin
      if (gpio_sync_s2 != gpio_prev) begin
        ip_isr_reg[0] <= 1'b1;
      end
      if (s_axi_awready) begin
        s_axi_bvalid <= 1'b1;
        case (s_axi_awaddr[8:2])
          7'h00: gpio_data_reg  <= s_axi_wdata[C_GPIO_WIDTH-1:0];
          7'h01: gpio_tri_reg   <= s_axi_wdata[C_GPIO_WIDTH-1:0];
          7'h02: gpio2_data_reg <= s_axi_wdata[C_GPIO_WIDTH-1:0];
          7'h03: gpio2_tri_reg  <= s_axi_wdata[C_GPIO_WIDTH-1:0];
          7'h47: gier_reg       <= s_axi_wdata[31];
          7'h48: ip_isr_reg     <= ip_isr_reg ^ s_axi_wdata[1:0];
          7'h4A: ip_ier_reg     <= s_axi_wdata[1:0];
          default: ;
        endcase
      end else if (s_axi_bvalid && s_axi_bready) begin
        s_axi_bvalid <= 1'b0;
      end

      if (s_axi_arready) begin
        s_axi_rvalid <= 1'b1;
        case (s_axi_araddr[8:2])
          7'h00: s_axi_rdata <= {{{{(32-C_GPIO_WIDTH){{1'b0}}}}, (gpio_sync_s2 & gpio_tri_reg) | (gpio_data_reg & ~gpio_tri_reg)}};
          7'h01: s_axi_rdata <= {{{{(32-C_GPIO_WIDTH){{1'b0}}}}, gpio_tri_reg}};
          7'h02: s_axi_rdata <= {{{{(32-C_GPIO_WIDTH){{1'b0}}}}, gpio2_data_reg}};
          7'h03: s_axi_rdata <= {{{{(32-C_GPIO_WIDTH){{1'b0}}}}, gpio2_tri_reg}};
          7'h47: s_axi_rdata <= {{gier_reg, 31'd0}};
          7'h48: s_axi_rdata <= {{30'd0, ip_isr_reg}};
          7'h4A: s_axi_rdata <= {{30'd0, ip_ier_reg}};
          default: s_axi_rdata <= 32'd0;
        endcase
      end else if (s_axi_rvalid && s_axi_rready) begin
        s_axi_rvalid <= 1'b0;
      end
    end
  end
endmodule
"#
    )
}

/// Generate a synthesizable AXI UART Lite peripheral matching Xilinx `axi_uartlite`.
pub fn generate_axi_uartlite_sv(
    clk_freq_hz: u32,
    baudrate: u32,
    data_bits: u32,
    use_parity: bool,
    odd_parity: bool,
) -> String {
    let parity_val = u32::from(use_parity);
    let odd_val = u32::from(odd_parity);
    format!(
        r#"// Synthesizable AXI4-Lite UART Controller (Vivado axi_uartlite equivalent)
module axi_uartlite #(
  parameter int C_CLK_FREQ_HZ      = {clk_freq_hz},
  parameter int C_BAUDRATE         = {baudrate},
  parameter int C_DATA_BITS        = {data_bits},
  parameter int C_USE_PARITY       = {parity_val},
  parameter int C_ODD_PARITY       = {odd_val},
  parameter int C_S_AXI_ADDR_WIDTH = 4,
  parameter int C_S_AXI_DATA_WIDTH = 32
) (
  input  logic                          s_axi_aclk,
  input  logic                          s_axi_aresetn,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_awaddr,
  input  logic [2:0]                    s_axi_awprot,
  input  logic                          s_axi_awvalid,
  output logic                          s_axi_awready,
  input  logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_wdata,
  input  logic [3:0]                    s_axi_wstrb,
  input  logic                          s_axi_wvalid,
  output logic                          s_axi_wready,
  output logic [1:0]                    s_axi_bresp,
  output logic                          s_axi_bvalid,
  input  logic                          s_axi_bready,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_araddr,
  input  logic [2:0]                    s_axi_arprot,
  input  logic                          s_axi_arvalid,
  output logic                          s_axi_arready,
  output logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_rdata,
  output logic [1:0]                    s_axi_rresp,
  output logic                          s_axi_rvalid,
  input  logic                          s_axi_rready,
  input  logic                          rx,
  output logic                          tx,
  output logic                          interrupt
);
  localparam int CLKS_PER_BIT = (C_CLK_FREQ_HZ / C_BAUDRATE) > 0 ? (C_CLK_FREQ_HZ / C_BAUDRATE) : 1;

  logic [7:0] tx_fifo [0:15];
  logic [3:0] tx_wr_ptr, tx_rd_ptr;
  logic [4:0] tx_count;
  logic [7:0] rx_fifo [0:15];
  logic [3:0] rx_wr_ptr, rx_rd_ptr;
  logic [4:0] rx_count;
  logic       intr_enable;

  logic [15:0] tx_clk_cnt;
  logic [3:0]  tx_bit_idx;
  logic [9:0]  tx_shift;
  logic        tx_busy;

  (* ASYNC_REG = "TRUE" *) logic rx_s1, rx_s2;
  logic [15:0] rx_clk_cnt;
  logic [3:0]  rx_bit_idx;
  logic [7:0]  rx_shift;
  logic        rx_busy;

  wire tx_fifo_full  = (tx_count == 5'd16);
  wire tx_fifo_empty = (tx_count == 5'd0);
  wire rx_fifo_full  = (rx_count == 5'd16);
  wire rx_fifo_valid = (rx_count != 5'd0);

  assign s_axi_awready = s_axi_awvalid && s_axi_wvalid && (!s_axi_bvalid || s_axi_bready);
  assign s_axi_wready  = s_axi_awready;
  assign s_axi_bresp   = 2'b00;
  assign s_axi_arready = s_axi_arvalid && (!s_axi_rvalid || s_axi_rready);
  assign s_axi_rresp   = 2'b00;
  assign interrupt     = intr_enable && (rx_fifo_valid || tx_fifo_empty);
  assign tx            = tx_busy ? tx_shift[0] : 1'b1;

  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      tx_wr_ptr    <= 4'd0;
      tx_rd_ptr    <= 4'd0;
      tx_count     <= 5'd0;
      rx_wr_ptr    <= 4'd0;
      rx_rd_ptr    <= 4'd0;
      rx_count     <= 5'd0;
      intr_enable  <= 1'b0;
      tx_clk_cnt   <= 16'd0;
      tx_bit_idx   <= 4'd0;
      tx_shift     <= 10'h3FF;
      tx_busy      <= 1'b0;
      rx_s1        <= 1'b1;
      rx_s2        <= 1'b1;
      rx_clk_cnt   <= 16'd0;
      rx_bit_idx   <= 4'd0;
      rx_shift     <= 8'd0;
      rx_busy      <= 1'b0;
      s_axi_bvalid <= 1'b0;
      s_axi_rvalid <= 1'b0;
      s_axi_rdata  <= 32'd0;
    end else begin
      rx_s1 <= rx;
      rx_s2 <= rx_s1;

      if (s_axi_awready) begin
        s_axi_bvalid <= 1'b1;
        case (s_axi_awaddr[3:2])
          2'b01: if (!tx_fifo_full) begin
            tx_fifo[tx_wr_ptr] <= s_axi_wdata[7:0];
            tx_wr_ptr <= tx_wr_ptr + 4'd1;
            tx_count  <= tx_count + 5'd1;
          end
          2'b11: begin
            intr_enable <= s_axi_wdata[4];
            if (s_axi_wdata[0]) begin
              tx_wr_ptr <= 4'd0;
              tx_rd_ptr <= 4'd0;
              tx_count  <= 5'd0;
            end
            if (s_axi_wdata[1]) begin
              rx_wr_ptr <= 4'd0;
              rx_rd_ptr <= 4'd0;
              rx_count  <= 5'd0;
            end
          end
          default: ;
        endcase
      end else if (s_axi_bvalid && s_axi_bready) begin
        s_axi_bvalid <= 1'b0;
      end

      if (!tx_busy && !tx_fifo_empty) begin
        tx_shift   <= {{1'b1, tx_fifo[tx_rd_ptr], 1'b0}};
        tx_rd_ptr  <= tx_rd_ptr + 4'd1;
        tx_count   <= tx_count - 5'd1;
        tx_busy    <= 1'b1;
        tx_clk_cnt <= 16'd0;
        tx_bit_idx <= 4'd0;
      end else if (tx_busy) begin
        if (tx_clk_cnt == CLKS_PER_BIT - 1) begin
          tx_clk_cnt <= 16'd0;
          tx_shift   <= {{1'b1, tx_shift[9:1]}};
          if (tx_bit_idx == 4'd9) tx_busy <= 1'b0;
          else tx_bit_idx <= tx_bit_idx + 4'd1;
        end else begin
          tx_clk_cnt <= tx_clk_cnt + 16'd1;
        end
      end

      if (!rx_busy && !rx_s2) begin
        rx_busy    <= 1'b1;
        rx_clk_cnt <= 16'd0;
        rx_bit_idx <= 4'd0;
      end else if (rx_busy) begin
        if (rx_clk_cnt == CLKS_PER_BIT - 1) begin
          rx_clk_cnt <= 16'd0;
          if (rx_bit_idx < 4'd8) begin
            rx_shift   <= {{rx_s2, rx_shift[7:1]}};
            rx_bit_idx <= rx_bit_idx + 4'd1;
          end else begin
            rx_busy <= 1'b0;
            if (!rx_fifo_full) begin
              rx_fifo[rx_wr_ptr] <= rx_shift;
              rx_wr_ptr <= rx_wr_ptr + 4'd1;
              rx_count  <= rx_count + 5'd1;
            end
          end
        end else begin
          rx_clk_cnt <= rx_clk_cnt + 16'd1;
        end
      end

      if (s_axi_arready) begin
        s_axi_rvalid <= 1'b1;
        case (s_axi_araddr[3:2])
          2'b00: begin
            s_axi_rdata <= {{24'd0, rx_fifo[rx_rd_ptr]}};
            if (rx_fifo_valid) begin
              rx_rd_ptr <= rx_rd_ptr + 4'd1;
              rx_count  <= rx_count - 5'd1;
            end
          end
          2'b10: s_axi_rdata <= {{27'd0, intr_enable, tx_fifo_full, tx_fifo_empty, rx_fifo_full, rx_fifo_valid}};
          default: s_axi_rdata <= 32'd0;
        endcase
      end else if (s_axi_rvalid && s_axi_rready) begin
        s_axi_rvalid <= 1'b0;
      end
    end
  end
endmodule
"#
    )
}

/// Generate a synthesizable dual 32-bit AXI Timer matching Xilinx `axi_timer`.
pub fn generate_axi_timer_sv(count_width: u32, one_timer_only: bool) -> String {
    let one_val = u32::from(one_timer_only);
    format!(
        r#"// Synthesizable Dual 32-bit AXI4-Lite Timer/Counter (Vivado axi_timer equivalent)
module axi_timer #(
  parameter int C_COUNT_WIDTH      = {count_width},
  parameter int C_ONE_TIMER_ONLY   = {one_val},
  parameter int C_S_AXI_ADDR_WIDTH = 5,
  parameter int C_S_AXI_DATA_WIDTH = 32
) (
  input  logic                          s_axi_aclk,
  input  logic                          s_axi_aresetn,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_awaddr,
  input  logic [2:0]                    s_axi_awprot,
  input  logic                          s_axi_awvalid,
  output logic                          s_axi_awready,
  input  logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_wdata,
  input  logic [3:0]                    s_axi_wstrb,
  input  logic                          s_axi_wvalid,
  output logic                          s_axi_wready,
  output logic [1:0]                    s_axi_bresp,
  output logic                          s_axi_bvalid,
  input  logic                          s_axi_bready,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_araddr,
  input  logic [2:0]                    s_axi_arprot,
  input  logic                          s_axi_arvalid,
  output logic                          s_axi_arready,
  output logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_rdata,
  output logic [1:0]                    s_axi_rresp,
  output logic                          s_axi_rvalid,
  input  logic                          s_axi_rready,
  input  logic                          capturetrig0,
  input  logic                          capturetrig1,
  output logic                          generateout0,
  output logic                          generateout1,
  output logic                          pwm0,
  output logic                          interrupt,
  input  logic                          freeze
);
  logic [31:0] tcsr0, tlr0, tcr0;
  logic [31:0] tcsr1, tlr1, tcr1;

  assign s_axi_awready = s_axi_awvalid && s_axi_wvalid && (!s_axi_bvalid || s_axi_bready);
  assign s_axi_wready  = s_axi_awready;
  assign s_axi_bresp   = 2'b00;
  assign s_axi_arready = s_axi_arvalid && (!s_axi_rvalid || s_axi_rready);
  assign s_axi_rresp   = 2'b00;

  assign generateout0 = (tcr0 == 32'd0) && tcsr0[7];
  assign generateout1 = (tcr1 == 32'd0) && tcsr1[7];
  assign pwm0         = tcsr0[9] && (tcr0 < tlr1);
  assign interrupt    = (tcsr0[8] && tcsr0[6]) || ((C_ONE_TIMER_ONLY == 0) && tcsr1[8] && tcsr1[6]);

  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      tcsr0        <= 32'd0;
      tlr0         <= 32'd0;
      tcr0         <= 32'd0;
      tcsr1        <= 32'd0;
      tlr1         <= 32'd0;
      tcr1         <= 32'd0;
      s_axi_bvalid <= 1'b0;
      s_axi_rvalid <= 1'b0;
      s_axi_rdata  <= 32'd0;
    end else begin
      if (!freeze) begin
        if (tcsr0[5]) begin
          tcr0 <= tlr0;
        end else if (tcsr0[7]) begin
          if (tcsr0[1]) begin
            if (tcr0 == 32'd0) begin
              tcsr0[8] <= 1'b1;
              tcr0 <= tcsr0[4] ? tlr0 : 32'd0;
            end else begin
              tcr0 <= tcr0 - 32'd1;
            end
          end else begin
            tcr0 <= tcr0 + 32'd1;
          end
        end
        if (C_ONE_TIMER_ONLY == 0) begin
          if (tcsr1[5]) begin
            tcr1 <= tlr1;
          end else if (tcsr1[7]) begin
            if (tcsr1[1]) begin
              if (tcr1 == 32'd0) begin
                tcsr1[8] <= 1'b1;
                tcr1 <= tcsr1[4] ? tlr1 : 32'd0;
              end else begin
                tcr1 <= tcr1 - 32'd1;
              end
            end else begin
              tcr1 <= tcr1 + 32'd1;
            end
          end
        end
      end

      if (s_axi_awready) begin
        s_axi_bvalid <= 1'b1;
        case (s_axi_awaddr[4:2])
          3'h0: begin
            tcsr0[7:0] <= s_axi_wdata[7:0];
            tcsr0[9]   <= s_axi_wdata[9];
            if (s_axi_wdata[8]) tcsr0[8] <= 1'b0;
          end
          3'h1: tlr0 <= s_axi_wdata;
          3'h4: begin
            tcsr1[7:0] <= s_axi_wdata[7:0];
            if (s_axi_wdata[8]) tcsr1[8] <= 1'b0;
          end
          3'h5: tlr1 <= s_axi_wdata;
          default: ;
        endcase
      end else if (s_axi_bvalid && s_axi_bready) begin
        s_axi_bvalid <= 1'b0;
      end

      if (s_axi_arready) begin
        s_axi_rvalid <= 1'b1;
        case (s_axi_araddr[4:2])
          3'h0: s_axi_rdata <= tcsr0;
          3'h1: s_axi_rdata <= tlr0;
          3'h2: s_axi_rdata <= tcr0;
          3'h4: s_axi_rdata <= tcsr1;
          3'h5: s_axi_rdata <= tlr1;
          3'h6: s_axi_rdata <= tcr1;
          default: s_axi_rdata <= 32'd0;
        endcase
      end else if (s_axi_rvalid && s_axi_rready) begin
        s_axi_rvalid <= 1'b0;
      end
    end
  end
endmodule
"#
    )
}

/// Generate a synthesizable AXI BRAM Controller matching Xilinx `axi_bram_ctrl`.
pub fn generate_axi_bram_ctrl_sv(
    mem_depth: u32,
    addr_width: u32,
    data_width: u32,
    internal_mem: bool,
) -> String {
    let strb_w = (data_width / 8).max(1);
    let int_mem_val = u32::from(internal_mem);
    format!(
        r#"// Synthesizable AXI4-Lite BRAM Controller (Vivado axi_bram_ctrl equivalent)
module axi_bram_ctrl #(
  parameter int C_MEMORY_DEPTH     = {mem_depth},
  parameter int C_S_AXI_ADDR_WIDTH = {addr_width},
  parameter int C_S_AXI_DATA_WIDTH = {data_width},
  parameter int C_INTERNAL_MEM     = {int_mem_val}
) (
  input  logic                          s_axi_aclk,
  input  logic                          s_axi_aresetn,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_awaddr,
  input  logic [2:0]                    s_axi_awprot,
  input  logic                          s_axi_awvalid,
  output logic                          s_axi_awready,
  input  logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_wdata,
  input  logic [{strb_w}-1:0]                  s_axi_wstrb,
  input  logic                          s_axi_wvalid,
  output logic                          s_axi_wready,
  output logic [1:0]                    s_axi_bresp,
  output logic                          s_axi_bvalid,
  input  logic                          s_axi_bready,
  input  logic [C_S_AXI_ADDR_WIDTH-1:0] s_axi_araddr,
  input  logic [2:0]                    s_axi_arprot,
  input  logic                          s_axi_arvalid,
  output logic                          s_axi_arready,
  output logic [C_S_AXI_DATA_WIDTH-1:0] s_axi_rdata,
  output logic [1:0]                    s_axi_rresp,
  output logic                          s_axi_rvalid,
  input  logic                          s_axi_rready,
  // BRAM_PORTA interface
  output logic                          bram_clk_a,
  output logic                          bram_rst_a,
  output logic                          bram_en_a,
  output logic [{strb_w}-1:0]                  bram_we_a,
  output logic [C_S_AXI_ADDR_WIDTH-1:0] bram_addr_a,
  output logic [C_S_AXI_DATA_WIDTH-1:0] bram_wrdata_a,
  input  logic [C_S_AXI_DATA_WIDTH-1:0] bram_rddata_a
);
  (* ram_style = "block" *) logic [C_S_AXI_DATA_WIDTH-1:0] int_mem [0:C_MEMORY_DEPTH-1];
  logic [C_S_AXI_DATA_WIDTH-1:0] int_rddata;

  wire wr_req = s_axi_awvalid && s_axi_wvalid && (!s_axi_bvalid || s_axi_bready);
  wire rd_req = s_axi_arvalid && !wr_req && (!s_axi_rvalid || s_axi_rready);

  assign bram_clk_a    = s_axi_aclk;
  assign bram_rst_a    = ~s_axi_aresetn;
  assign bram_en_a     = wr_req || rd_req;
  assign bram_we_a     = wr_req ? s_axi_wstrb : '0;
  assign bram_addr_a   = wr_req ? s_axi_awaddr : s_axi_araddr;
  assign bram_wrdata_a = s_axi_wdata;

  assign s_axi_awready = wr_req;
  assign s_axi_wready  = wr_req;
  assign s_axi_bresp   = 2'b00;
  assign s_axi_arready = rd_req;
  assign s_axi_rresp   = 2'b00;
  assign s_axi_rdata   = (C_INTERNAL_MEM != 0) ? int_rddata : bram_rddata_a;

  wire [$clog2(C_MEMORY_DEPTH)-1:0] word_idx = bram_addr_a[$clog2(C_MEMORY_DEPTH)+1:2];

  always_ff @(posedge s_axi_aclk) begin
    if (wr_req) begin
      for (int b = 0; b < {strb_w}; b++) begin
        if (s_axi_wstrb[b]) int_mem[word_idx][b*8 +: 8] <= s_axi_wdata[b*8 +: 8];
      end
    end
    if (rd_req) begin
      int_rddata <= int_mem[word_idx];
    end
  end

  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      s_axi_bvalid <= 1'b0;
      s_axi_rvalid <= 1'b0;
    end else begin
      if (wr_req) s_axi_bvalid <= 1'b1;
      else if (s_axi_bvalid && s_axi_bready) s_axi_bvalid <= 1'b0;

      if (rd_req) s_axi_rvalid <= 1'b1;
      else if (s_axi_rvalid && s_axi_rready) s_axi_rvalid <= 1'b0;
    end
  end
endmodule
"#
    )
}

/// Generate a synthesizable Synchronous/Asynchronous Gray-code CDC FIFO (`cdc_fifo`).
pub fn generate_cdc_fifo_sv(data_width: u32, fifo_depth: u32, is_async: bool) -> String {
    let keep_w = (data_width / 8).max(1);
    let async_val = u32::from(is_async);
    format!(
        r#"// Synthesizable AXI4-Stream Synchronous / Dual-Clock Gray-Code Async CDC FIFO
module cdc_fifo #(
  parameter int C_TDATA_WIDTH = {data_width},
  parameter int C_FIFO_DEPTH  = {fifo_depth},
  parameter int C_IS_ASYNC    = {async_val}
) (
  input  logic                        s_axis_aclk,
  input  logic                        s_axis_aresetn,
  input  logic [C_TDATA_WIDTH-1:0]    s_axis_tdata,
  input  logic [{keep_w}-1:0]                s_axis_tkeep,
  input  logic                        s_axis_tlast,
  input  logic                        s_axis_tvalid,
  output logic                        s_axis_tready,
  input  logic                        m_axis_aclk,
  input  logic                        m_axis_aresetn,
  output logic [C_TDATA_WIDTH-1:0]    m_axis_tdata,
  output logic [{keep_w}-1:0]                m_axis_tkeep,
  output logic                        m_axis_tlast,
  output logic                        m_axis_tvalid,
  input  logic                        m_axis_tready,
  output logic                        axis_full,
  output logic                        axis_empty
);
  localparam int PTR_W  = $clog2(C_FIFO_DEPTH);
  localparam int PAY_W  = C_TDATA_WIDTH + {keep_w} + 1;

  logic [PAY_W-1:0] mem [0:C_FIFO_DEPTH-1];
  logic [PTR_W:0]   wr_bin, wr_bin_next, wr_gray, wr_gray_next;
  logic [PTR_W:0]   rd_bin, rd_bin_next, rd_gray, rd_gray_next;
  (* ASYNC_REG = "TRUE" *) logic [PTR_W:0] wr_gray_s1, wr_gray_s2;
  (* ASYNC_REG = "TRUE" *) logic [PTR_W:0] rd_gray_s1, rd_gray_s2;

  wire [PTR_W:0] rd_gray_in_wr = (C_IS_ASYNC != 0) ? rd_gray_s2 : rd_gray;
  wire [PTR_W:0] wr_gray_in_rd = (C_IS_ASYNC != 0) ? wr_gray_s2 : wr_gray;

  assign axis_full     = (wr_gray == {{~rd_gray_in_wr[PTR_W:PTR_W-1], rd_gray_in_wr[PTR_W-2:0]}});
  assign axis_empty    = (rd_gray == wr_gray_in_rd);
  assign s_axis_tready = !axis_full;
  assign m_axis_tvalid = !axis_empty;

  wire wr_en = s_axis_tvalid && s_axis_tready;
  wire rd_en = m_axis_tvalid && m_axis_tready;

  assign wr_bin_next  = wr_bin + {{PTR_W{{1'b0}}, wr_en}};
  assign wr_gray_next = (wr_bin_next >> 1) ^ wr_bin_next;
  assign rd_bin_next  = rd_bin + {{PTR_W{{1'b0}}, rd_en}};
  assign rd_gray_next = (rd_bin_next >> 1) ^ rd_bin_next;

  always_ff @(posedge s_axis_aclk or negedge s_axis_aresetn) begin
    if (!s_axis_aresetn) begin
      wr_bin     <= '0;
      wr_gray    <= '0;
      rd_gray_s1 <= '0;
      rd_gray_s2 <= '0;
    end else begin
      wr_bin     <= wr_bin_next;
      wr_gray    <= wr_gray_next;
      rd_gray_s1 <= rd_gray;
      rd_gray_s2 <= rd_gray_s1;
      if (wr_en) begin
        mem[wr_bin[PTR_W-1:0]] <= {{s_axis_tlast, s_axis_tkeep, s_axis_tdata}};
      end
    end
  end

  wire rx_clk  = (C_IS_ASYNC != 0) ? m_axis_aclk    : s_axis_aclk;
  wire rx_rstn = (C_IS_ASYNC != 0) ? m_axis_aresetn : s_axis_aresetn;

  always_ff @(posedge rx_clk or negedge rx_rstn) begin
    if (!rx_rstn) begin
      rd_bin     <= '0;
      rd_gray    <= '0;
      wr_gray_s1 <= '0;
      wr_gray_s2 <= '0;
    end else begin
      rd_bin     <= rd_bin_next;
      rd_gray    <= rd_gray_next;
      wr_gray_s1 <= wr_gray;
      wr_gray_s2 <= wr_gray_s1;
    end
  end

  assign {{m_axis_tlast, m_axis_tkeep, m_axis_tdata}} = mem[rd_bin[PTR_W-1:0]];
endmodule
"#
    )
}

/// Generate a synthesizable Processor System Reset (`proc_sys_reset`) module.
pub fn generate_proc_sys_reset_sv(
    ext_rst_width: u32,
    aux_rst_width: u32,
    ext_reset_high: bool,
    aux_reset_high: bool,
) -> String {
    let ext_hi = u32::from(ext_reset_high);
    let aux_hi = u32::from(aux_reset_high);
    format!(
        r#"// Synthesizable Clock/Reset Synchronizer (Vivado proc_sys_reset equivalent)
module proc_sys_reset #(
  parameter int C_EXT_RST_WIDTH  = {ext_rst_width},
  parameter int C_AUX_RST_WIDTH  = {aux_rst_width},
  parameter int C_EXT_RESET_HIGH = {ext_hi},
  parameter int C_AUX_RESET_HIGH = {aux_hi}
) (
  input  logic slowest_sync_clk,
  input  logic ext_reset_in,
  input  logic aux_reset_in,
  input  logic mb_debug_sys_rst,
  input  logic dcm_locked,
  output logic mb_reset,
  output logic bus_struct_reset,
  output logic peripheral_reset,
  output logic interconnect_aresetn,
  output logic peripheral_aresetn
);
  wire ext_active = (C_EXT_RESET_HIGH != 0) ? ext_reset_in : ~ext_reset_in;
  wire aux_active = (C_AUX_RESET_HIGH != 0) ? aux_reset_in : ~aux_reset_in;
  wire raw_rst    = ext_active | aux_active | mb_debug_sys_rst | ~dcm_locked;

  (* ASYNC_REG = "TRUE" *) logic [3:0] rst_sync;
  logic [5:0] seq_cnt;

  always_ff @(posedge slowest_sync_clk) begin
    rst_sync <= {{rst_sync[2:0], raw_rst}};
    if (rst_sync[3]) begin
      seq_cnt              <= 6'd0;
      bus_struct_reset     <= 1'b1;
      interconnect_aresetn <= 1'b0;
      peripheral_reset     <= 1'b1;
      peripheral_aresetn   <= 1'b0;
      mb_reset             <= 1'b1;
    end else begin
      if (seq_cnt != 6'h3F) seq_cnt <= seq_cnt + 6'd1;
      if (seq_cnt >= 6'd8) begin
        bus_struct_reset     <= 1'b0;
        interconnect_aresetn <= 1'b1;
      end
      if (seq_cnt >= 6'd16) begin
        peripheral_reset   <= 1'b0;
        peripheral_aresetn <= 1'b1;
      end
      if (seq_cnt >= 6'd32) begin
        mb_reset <= 1'b0;
      end
    end
  end
endmodule
"#
    )
}

pub fn generate_axi_cpu_sv() -> String {
    r#"// Synthesizable 32-bit AXI4-Lite Master Processor Core
module axi_cpu #(
  parameter logic [31:0] C_RESET_VECTOR = 32'h0000_0000,
  parameter int          C_ADDR_WIDTH   = 32,
  parameter int          C_DATA_WIDTH   = 32
) (
  input  logic                    aclk,
  input  logic                    aresetn,
  output logic [C_ADDR_WIDTH-1:0] m_axi_awaddr,
  output logic [2:0]              m_axi_awprot,
  output logic                    m_axi_awvalid,
  input  logic                    m_axi_awready,
  output logic [C_DATA_WIDTH-1:0] m_axi_wdata,
  output logic [3:0]              m_axi_wstrb,
  output logic                    m_axi_wvalid,
  input  logic                    m_axi_wready,
  input  logic [1:0]              m_axi_bresp,
  input  logic                    m_axi_bvalid,
  output logic                    m_axi_bready,
  output logic [C_ADDR_WIDTH-1:0] m_axi_araddr,
  output logic [2:0]              m_axi_arprot,
  output logic                    m_axi_arvalid,
  input  logic                    m_axi_arready,
  input  logic [C_DATA_WIDTH-1:0] m_axi_rdata,
  input  logic [1:0]              m_axi_rresp,
  input  logic                    m_axi_rvalid,
  output logic                    m_axi_rready,
  input  logic                    irq
);
  logic [31:0] pc;
  always_ff @(posedge aclk or negedge aresetn) begin
    if (!aresetn) begin
      pc            <= C_RESET_VECTOR;
      m_axi_awaddr  <= '0;
      m_axi_awprot  <= 3'b000;
      m_axi_awvalid <= 1'b0;
      m_axi_wdata   <= '0;
      m_axi_wstrb   <= 4'hF;
      m_axi_wvalid  <= 1'b0;
      m_axi_bready  <= 1'b1;
      m_axi_araddr  <= C_RESET_VECTOR;
      m_axi_arprot  <= 3'b100;
      m_axi_arvalid <= 1'b0;
      m_axi_rready  <= 1'b1;
    end else begin
      if (!m_axi_arvalid) begin
        m_axi_araddr  <= pc;
        m_axi_arvalid <= 1'b1;
      end else if (m_axi_arready) begin
        m_axi_arvalid <= 1'b0;
        pc            <= pc + 32'd4;
      end
    end
  end
endmodule
"#
    .into()
}

pub fn generate_axi4_master_cpu_sv() -> String {
    r#"// Synthesizable 32-bit AXI4 Full Burst Master Processor Core
module axi4_master_cpu #(
  parameter logic [31:0] C_RESET_VECTOR = 32'h0000_0000,
  parameter int          C_ADDR_WIDTH   = 32,
  parameter int          C_DATA_WIDTH   = 32
) (
  input  logic                    aclk,
  input  logic                    aresetn,
  output logic [C_ADDR_WIDTH-1:0] m_axi_awaddr,
  output logic [7:0]              m_axi_awlen,
  output logic [2:0]              m_axi_awsize,
  output logic [1:0]              m_axi_awburst,
  output logic [2:0]              m_axi_awprot,
  output logic                    m_axi_awvalid,
  input  logic                    m_axi_awready,
  output logic [C_DATA_WIDTH-1:0] m_axi_wdata,
  output logic [3:0]              m_axi_wstrb,
  output logic                    m_axi_wlast,
  output logic                    m_axi_wvalid,
  input  logic                    m_axi_wready,
  input  logic [1:0]              m_axi_bresp,
  input  logic                    m_axi_bvalid,
  output logic                    m_axi_bready,
  output logic [C_ADDR_WIDTH-1:0] m_axi_araddr,
  output logic [7:0]              m_axi_arlen,
  output logic [2:0]              m_axi_arsize,
  output logic [1:0]              m_axi_arburst,
  output logic [2:0]              m_axi_arprot,
  output logic                    m_axi_arvalid,
  input  logic                    m_axi_arready,
  input  logic [C_DATA_WIDTH-1:0] m_axi_rdata,
  input  logic [1:0]              m_axi_rresp,
  input  logic                    m_axi_rlast,
  input  logic                    m_axi_rvalid,
  output logic                    m_axi_rready,
  input  logic                    irq
);
  assign m_axi_awlen   = 8'd0;
  assign m_axi_awsize  = 3'b010;
  assign m_axi_awburst = 2'b01;
  assign m_axi_wlast   = 1'b1;
  assign m_axi_arlen   = 8'd0;
  assign m_axi_arsize  = 3'b010;
  assign m_axi_arburst = 2'b01;
  assign m_axi_awaddr  = '0;
  assign m_axi_awprot  = 3'b000;
  assign m_axi_awvalid = 1'b0;
  assign m_axi_wdata   = '0;
  assign m_axi_wstrb   = 4'hF;
  assign m_axi_wvalid  = 1'b0;
  assign m_axi_bready  = 1'b1;
  assign m_axi_araddr  = C_RESET_VECTOR;
  assign m_axi_arprot  = 3'b100;
  assign m_axi_arvalid = aresetn;
  assign m_axi_rready  = 1'b1;
endmodule
"#
    .into()
}

pub fn generate_axi_quad_spi_sv() -> String {
    r#"// Synthesizable AXI4-Lite SPI Master Controller
module axi_quad_spi #(
  parameter int C_SCK_RATIO = 8
) (
  input  logic        s_axi_aclk,
  input  logic        s_axi_aresetn,
  input  logic [6:0]  s_axi_awaddr,
  input  logic [2:0]  s_axi_awprot,
  input  logic        s_axi_awvalid,
  output logic        s_axi_awready,
  input  logic [31:0] s_axi_wdata,
  input  logic [3:0]  s_axi_wstrb,
  input  logic        s_axi_wvalid,
  output logic        s_axi_wready,
  output logic [1:0]  s_axi_bresp,
  output logic        s_axi_bvalid,
  input  logic        s_axi_bready,
  input  logic [6:0]  s_axi_araddr,
  input  logic [2:0]  s_axi_arprot,
  input  logic        s_axi_arvalid,
  output logic        s_axi_arready,
  output logic [31:0] s_axi_rdata,
  output logic [1:0]  s_axi_rresp,
  output logic        s_axi_rvalid,
  input  logic        s_axi_rready,
  output logic        sck_o,
  output logic        mosi_o,
  input  logic        miso_i,
  output logic        ss_o,
  output logic        ip2intc_irpt
);
  logic [7:0] tx_reg, rx_reg;
  assign s_axi_awready = s_axi_awvalid && s_axi_wvalid;
  assign s_axi_wready  = s_axi_awready;
  assign s_axi_bresp   = 2'b00;
  assign s_axi_arready = s_axi_arvalid;
  assign s_axi_rresp   = 2'b00;
  assign sck_o         = 1'b0;
  assign mosi_o        = tx_reg[7];
  assign ss_o          = 1'b1;
  assign ip2intc_irpt  = 1'b0;
  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      tx_reg       <= 8'd0;
      rx_reg       <= 8'd0;
      s_axi_bvalid <= 1'b0;
      s_axi_rvalid <= 1'b0;
      s_axi_rdata  <= 32'd0;
    end else begin
      rx_reg <= {rx_reg[6:0], miso_i};
      if (s_axi_awready) begin
        tx_reg       <= s_axi_wdata[7:0];
        s_axi_bvalid <= 1'b1;
      end else if (s_axi_bvalid && s_axi_bready) s_axi_bvalid <= 1'b0;
      if (s_axi_arready) begin
        s_axi_rdata  <= {24'd0, rx_reg};
        s_axi_rvalid <= 1'b1;
      end else if (s_axi_rvalid && s_axi_rready) s_axi_rvalid <= 1'b0;
    end
  end
endmodule
"#
    .into()
}

pub fn generate_axi_iic_sv() -> String {
    r#"// Synthesizable AXI4-Lite I2C Master Controller
module axi_iic #(
  parameter int C_IIC_FREQ = 100000
) (
  input  logic        s_axi_aclk,
  input  logic        s_axi_aresetn,
  input  logic [8:0]  s_axi_awaddr,
  input  logic [2:0]  s_axi_awprot,
  input  logic        s_axi_awvalid,
  output logic        s_axi_awready,
  input  logic [31:0] s_axi_wdata,
  input  logic [3:0]  s_axi_wstrb,
  input  logic        s_axi_wvalid,
  output logic        s_axi_wready,
  output logic [1:0]  s_axi_bresp,
  output logic        s_axi_bvalid,
  input  logic        s_axi_bready,
  input  logic [8:0]  s_axi_araddr,
  input  logic [2:0]  s_axi_arprot,
  input  logic        s_axi_arvalid,
  output logic        s_axi_arready,
  output logic [31:0] s_axi_rdata,
  output logic [1:0]  s_axi_rresp,
  output logic        s_axi_rvalid,
  input  logic        s_axi_rready,
  input  logic        sda_i,
  output logic        sda_o,
  output logic        sda_t,
  input  logic        scl_i,
  output logic        scl_o,
  output logic        scl_t,
  output logic        iic2intc_irpt
);
  assign s_axi_awready = s_axi_awvalid && s_axi_wvalid;
  assign s_axi_wready  = s_axi_awready;
  assign s_axi_bresp   = 2'b00;
  assign s_axi_arready = s_axi_arvalid;
  assign s_axi_rresp   = 2'b00;
  assign sda_o         = 1'b0;
  assign sda_t         = 1'b1;
  assign scl_o         = 1'b0;
  assign scl_t         = 1'b1;
  assign iic2intc_irpt = 1'b0;
  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      s_axi_bvalid <= 1'b0;
      s_axi_rvalid <= 1'b0;
      s_axi_rdata  <= 32'd0;
    end else begin
      if (s_axi_awready) s_axi_bvalid <= 1'b1;
      else if (s_axi_bvalid && s_axi_bready) s_axi_bvalid <= 1'b0;
      if (s_axi_arready) begin
        s_axi_rdata  <= {30'd0, scl_i, sda_i};
        s_axi_rvalid <= 1'b1;
      end else if (s_axi_rvalid && s_axi_rready) s_axi_rvalid <= 1'b0;
    end
  end
endmodule
"#
    .into()
}

pub fn generate_axi_intc_sv() -> String {
    r#"// Synthesizable AXI4-Lite Interrupt Controller
module axi_intc #(
  parameter int C_NUM_INTR_INPUTS = 4
) (
  input  logic                         s_axi_aclk,
  input  logic                         s_axi_aresetn,
  input  logic [8:0]                   s_axi_awaddr,
  input  logic [2:0]                   s_axi_awprot,
  input  logic                         s_axi_awvalid,
  output logic                         s_axi_awready,
  input  logic [31:0]                  s_axi_wdata,
  input  logic [3:0]                   s_axi_wstrb,
  input  logic                         s_axi_wvalid,
  output logic                         s_axi_wready,
  output logic [1:0]                   s_axi_bresp,
  output logic                         s_axi_bvalid,
  input  logic                         s_axi_bready,
  input  logic [8:0]                   s_axi_araddr,
  input  logic [2:0]                   s_axi_arprot,
  input  logic                         s_axi_arvalid,
  output logic                         s_axi_arready,
  output logic [31:0]                  s_axi_rdata,
  output logic [1:0]                   s_axi_rresp,
  output logic                         s_axi_rvalid,
  input  logic                         s_axi_rready,
  input  logic [C_NUM_INTR_INPUTS-1:0] intr,
  output logic                         irq
);
  logic [C_NUM_INTR_INPUTS-1:0] isr, ier;
  assign s_axi_awready = s_axi_awvalid && s_axi_wvalid;
  assign s_axi_wready  = s_axi_awready;
  assign s_axi_bresp   = 2'b00;
  assign s_axi_arready = s_axi_arvalid;
  assign s_axi_rresp   = 2'b00;
  assign irq           = |(isr & ier);
  always_ff @(posedge s_axi_aclk or negedge s_axi_aresetn) begin
    if (!s_axi_aresetn) begin
      isr          <= '0;
      ier          <= '1;
      s_axi_bvalid <= 1'b0;
      s_axi_rvalid <= 1'b0;
      s_axi_rdata  <= 32'd0;
    end else begin
      isr <= isr | intr;
      if (s_axi_awready) begin
        if (s_axi_awaddr[4:2] == 3'h2) ier <= s_axi_wdata[C_NUM_INTR_INPUTS-1:0];
        if (s_axi_awaddr[4:2] == 3'h3) isr <= isr & ~s_axi_wdata[C_NUM_INTR_INPUTS-1:0];
        s_axi_bvalid <= 1'b1;
      end else if (s_axi_bvalid && s_axi_bready) s_axi_bvalid <= 1'b0;
      if (s_axi_arready) begin
        s_axi_rdata  <= {{(32-C_NUM_INTR_INPUTS){1'b0}}, isr};
        s_axi_rvalid <= 1'b1;
      end else if (s_axi_rvalid && s_axi_rready) s_axi_rvalid <= 1'b0;
    end
  end
endmodule
"#
    .into()
}

pub fn generate_blk_mem_gen_sv() -> String {
    r#"// Synthesizable Block Memory Generator
module blk_mem_gen #(
  parameter int C_MEM_DEPTH  = 2048,
  parameter int C_ADDR_WIDTH = 12,
  parameter int C_DATA_WIDTH = 32
) (
  input  logic                    clka,
  input  logic                    rsta,
  input  logic                    ena,
  input  logic [3:0]              wea,
  input  logic [C_ADDR_WIDTH-1:0] addra,
  input  logic [C_DATA_WIDTH-1:0] dina,
  output logic [C_DATA_WIDTH-1:0] douta
);
  (* ram_style = "block" *) logic [C_DATA_WIDTH-1:0] mem [0:C_MEM_DEPTH-1];
  wire [$clog2(C_MEM_DEPTH)-1:0] idx = addra[$clog2(C_MEM_DEPTH)+1:2];
  always_ff @(posedge clka) begin
    if (rsta) begin
      douta <= '0;
    end else if (ena) begin
      for (int b = 0; b < 4; b++) begin
        if (wea[b]) mem[idx][b*8 +: 8] <= dina[b*8 +: 8];
      end
      douta <= mem[idx];
    end
  end
endmodule
"#
    .into()
}

pub fn generate_apb_gpio_sv() -> String {
    r#"// Synthesizable APB GPIO Peripheral
module apb_gpio #(
  parameter int C_GPIO_WIDTH = 32
) (
  input  logic                    pclk,
  input  logic                    presetn,
  input  logic [11:0]             s_apb_paddr,
  input  logic                    s_apb_psel,
  input  logic                    s_apb_penable,
  input  logic                    s_apb_pwrite,
  input  logic [31:0]             s_apb_pwdata,
  output logic [31:0]             s_apb_prdata,
  output logic                    s_apb_pready,
  output logic                    s_apb_pslverr,
  input  logic [C_GPIO_WIDTH-1:0] gpio_io_i,
  output logic [C_GPIO_WIDTH-1:0] gpio_io_o,
  output logic [C_GPIO_WIDTH-1:0] gpio_io_t
);
  assign s_apb_pready  = 1'b1;
  assign s_apb_pslverr = 1'b0;
  assign s_apb_prdata  = {{(32-C_GPIO_WIDTH){1'b0}}, gpio_io_i};
  always_ff @(posedge pclk or negedge presetn) begin
    if (!presetn) begin
      gpio_io_o <= '0;
      gpio_io_t <= '1;
    end else if (s_apb_psel && s_apb_penable && s_apb_pwrite) begin
      if (s_apb_paddr[3:2] == 2'b00) gpio_io_o <= s_apb_pwdata[C_GPIO_WIDTH-1:0];
      if (s_apb_paddr[3:2] == 2'b01) gpio_io_t <= s_apb_pwdata[C_GPIO_WIDTH-1:0];
    end
  end
endmodule
"#
    .into()
}

pub fn generate_wb_gpio_sv() -> String {
    r#"// Synthesizable Wishbone B4 GPIO Peripheral
module wb_gpio #(
  parameter int C_GPIO_WIDTH = 32
) (
  input  logic        wb_clk_i,
  input  logic        wb_rst_i,
  input  logic [11:0] wb_adr_i,
  input  logic [31:0] wb_dat_i,
  output logic [31:0] wb_dat_o,
  input  logic        wb_we_i,
  input  logic [3:0]  wb_sel_i,
  input  logic        wb_stb_i,
  input  logic        wb_cyc_i,
  output logic        wb_ack_o
);
  always_ff @(posedge wb_clk_i) begin
    if (wb_rst_i) begin
      wb_dat_o <= 32'd0;
      wb_ack_o <= 1'b0;
    end else begin
      wb_ack_o <= wb_cyc_i && wb_stb_i && !wb_ack_o;
      if (wb_cyc_i && wb_stb_i && wb_we_i) wb_dat_o <= wb_dat_i;
    end
  end
endmodule
"#
    .into()
}

pub fn generate_generic_component_sv(comp: &IpComponent) -> String {
    let mut sv = format!("module {} (\n", comp.core.name);
    for (idx, p) in comp.ports.iter().enumerate() {
        let sep = if idx + 1 == comp.ports.len() { "" } else { "," };
        if p.width > 1 {
            sv.push_str(&format!(
                "  {} logic [{}:0] {}{}\n",
                p.direction.sv_keyword(),
                p.width - 1,
                p.name,
                sep
            ));
        } else {
            sv.push_str(&format!(
                "  {} logic {}{}\n",
                p.direction.sv_keyword(),
                p.name,
                sep
            ));
        }
    }
    sv.push_str(");\n");
    for p in &comp.ports {
        if p.direction == PortDirection::Out {
            sv.push_str(&format!("  assign {} = '0;\n", p.name));
        }
    }
    sv.push_str("endmodule\n");
    sv
}
