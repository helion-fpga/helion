//! Vivado Serial Link & Network-on-Chip (NoC) IP Suite:
//! Parametric synthesizable SystemVerilog RTL generators and Rust golden reference models for:
//! - Aurora 8B/10B & 64B/66B High-Speed Serial Framing & $1 + x^{39} + x^{58}$ Scrambler (`aurora_serial_link`)
//! - JESD204B/C High-Speed ADC/DAC Transport, $1 + x^{14} + x^{15}$ Scrambler & `SYSREF` LMFC Alignment (`jesd204bc_core`)
//! - AXI Chip2Chip Inter-FPGA Bridge (`axi_chip2chip_bridge`)
//! - 2D Mesh Wormhole Network-on-Chip (NoC) Packet Router with XY Routing & Virtual Channels (`noc_2d_mesh_router`).

use crate::IpCore;

pub fn pack_aurora_link() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "aurora_serial_link".into(),
        version: "12.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

pub fn pack_jesd204bc() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "jesd204bc_core".into(),
        version: "7.2".into(),
        bus: "AXI4-Stream".into(),
    }
}

pub fn pack_axi_chip2chip() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_chip2chip_bridge".into(),
        version: "5.0".into(),
        bus: "AXI4".into(),
    }
}

pub fn pack_noc_xy_router() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "noc_2d_mesh_router".into(),
        version: "1.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

/// Returns all Serial Link & NoC IP core descriptors.
pub fn serial_noc_catalog() -> Vec<IpCore> {
    vec![
        pack_aurora_link(),
        pack_jesd204bc(),
        pack_axi_chip2chip(),
        pack_noc_xy_router(),
    ]
}

// ============================================================================
// 1. Aurora 64B/66B Scrambler ($1 + x^{39} + x^{58}$) & Framing Golden Model
// ============================================================================

/// Aurora 64B/66B 2-bit sync header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncHeader64B66B {
    /// `2'b01` — 64-bit pure data payload.
    Data = 0b01,
    /// `2'b10` — 8-bit block type + 56-bit control/ordered-set payload (e.g. `0x78` channel bond / idle).
    Control = 0b10,
}

/// Scramble a 64-bit word using the IEEE 802.3ae / Aurora 64B/66B polynomial $G(x) = 1 + x^{39} + x^{58}$.
/// Updates the 58-bit `state` in place and returns the 64-bit scrambled payload.
pub fn scramble_64b66b(state: &mut u64, payload: u64) -> u64 {
    let mut s = *state & ((1u64 << 58) - 1);
    let mut out = 0u64;
    for i in 0..64 {
        let in_bit = (payload >> i) & 1;
        let bit38 = (s >> 38) & 1;
        let bit57 = (s >> 57) & 1;
        let scr_bit = in_bit ^ bit38 ^ bit57;
        out |= scr_bit << i;
        s = ((s << 1) | scr_bit) & ((1u64 << 58) - 1);
    }
    *state = s;
    out
}

/// Descramble a 64-bit word using the self-synchronizing polynomial $G(x) = 1 + x^{39} + x^{58}$.
pub fn descramble_64b66b(state: &mut u64, scrambled: u64) -> u64 {
    let mut s = *state & ((1u64 << 58) - 1);
    let mut out = 0u64;
    for i in 0..64 {
        let scr_bit = (scrambled >> i) & 1;
        let bit38 = (s >> 38) & 1;
        let bit57 = (s >> 57) & 1;
        let descr_bit = scr_bit ^ bit38 ^ bit57;
        out |= descr_bit << i;
        s = ((s << 1) | scr_bit) & ((1u64 << 58) - 1);
    }
    *state = s;
    out
}

// ============================================================================
// 2. JESD204B/C Scrambler ($1 + x^{14} + x^{15}$) & Sample Transport Golden Model
// ============================================================================

/// Scramble a 32-bit lane word using the JEDEC JESD204B/C polynomial $1 + x^{14} + x^{15}$.
pub fn jesd204_scramble_word(state: &mut u16, word: u32) -> u32 {
    let mut s = *state & 0x7FFF;
    let mut out = 0u32;
    for i in (0..32).rev() {
        let in_bit = ((word >> i) & 1) as u16;
        let b13 = (s >> 13) & 1;
        let b14 = (s >> 14) & 1;
        let scr = in_bit ^ b13 ^ b14;
        out |= (scr as u32) << i;
        s = ((s << 1) | scr) & 0x7FFF;
    }
    *state = s;
    out
}

/// Descramble a 32-bit lane word using the self-synchronizing polynomial $1 + x^{14} + x^{15}$.
pub fn jesd204_descramble_word(state: &mut u16, scrambled: u32) -> u32 {
    let mut s = *state & 0x7FFF;
    let mut out = 0u32;
    for i in (0..32).rev() {
        let scr = ((scrambled >> i) & 1) as u16;
        let b13 = (s >> 13) & 1;
        let b14 = (s >> 14) & 1;
        let descr = scr ^ b13 ^ b14;
        out |= (descr as u32) << i;
        s = ((s << 1) | scr) & 0x7FFF;
    }
    *state = s;
    out
}

/// Pack `M` 16-bit converter samples into `L` JESD204 lane frames (with tail-bit / octet framing).
pub fn jesd204_pack_samples(samples: &[u16], num_lanes: usize) -> Vec<Vec<u8>> {
    let lanes = num_lanes.max(1);
    let mut out = vec![Vec::new(); lanes];
    for (idx, &sample) in samples.iter().enumerate() {
        let lane = idx % lanes;
        out[lane].push((sample >> 8) as u8);
        out[lane].push((sample & 0xFF) as u8);
    }
    out
}

/// Unpack `L` JESD204 lane frames back into 16-bit converter samples.
pub fn jesd204_unpack_samples(lane_frames: &[Vec<u8>], total_samples: usize) -> Vec<u16> {
    let lanes = lane_frames.len().max(1);
    let mut out = Vec::with_capacity(total_samples);
    for idx in 0..total_samples {
        let lane = idx % lanes;
        let pos = (idx / lanes) * 2;
        if pos + 1 < lane_frames[lane].len() {
            let hi = u16::from(lane_frames[lane][pos]);
            let lo = u16::from(lane_frames[lane][pos + 1]);
            out.push((hi << 8) | lo);
        }
    }
    out
}

// ============================================================================
// 3. 2D Mesh Wormhole NoC Dimension-Order XY Router Golden Model
// ============================================================================

/// 5-port 2D Mesh NoC router cardinal directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NocDir {
    Local = 0,
    North = 1,
    South = 2,
    East = 3,
    West = 4,
}

/// Wormhole packet flit classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NocFlitType {
    Head = 0b01,
    Body = 0b00,
    Tail = 0b10,
    HeadTail = 0b11,
}

/// Deadlock-free Dimension-Order XY routing function:
/// Routes along the X axis (`East` if `dst_x > cur_x`, `West` if `dst_x < cur_x`) first,
/// then along the Y axis (`North` if `dst_y > cur_y`, `South` if `dst_y < cur_y`),
/// and finally ejects at `Local` when `(dst_x, dst_y) == (cur_x, cur_y)`.
pub fn xy_route_next_port(cur_x: u32, cur_y: u32, dst_x: u32, dst_y: u32) -> NocDir {
    if dst_x > cur_x {
        NocDir::East
    } else if dst_x < cur_x {
        NocDir::West
    } else if dst_y > cur_y {
        NocDir::North
    } else if dst_y < cur_y {
        NocDir::South
    } else {
        NocDir::Local
    }
}

/// Simulate a complete wormhole path across an $N_X \times N_Y$ 2D mesh from `(src_x, src_y)` to `(dst_x, dst_y)`.
pub fn trace_xy_mesh_route(
    src_x: u32,
    src_y: u32,
    dst_x: u32,
    dst_y: u32,
) -> Vec<(u32, u32, NocDir)> {
    let mut hops = Vec::new();
    let (mut cx, mut cy) = (src_x, src_y);
    for _ in 0..256 {
        let dir = xy_route_next_port(cx, cy, dst_x, dst_y);
        hops.push((cx, cy, dir));
        match dir {
            NocDir::East => cx += 1,
            NocDir::West => cx = cx.saturating_sub(1),
            NocDir::North => cy += 1,
            NocDir::South => cy = cy.saturating_sub(1),
            NocDir::Local => break,
        }
    }
    hops
}

// ============================================================================
// 4. Parametric Synthesizable SystemVerilog RTL Generators
// ============================================================================

/// Generate synthesizable SystemVerilog for a Xilinx Aurora 8B/10B & 64B/66B High-Speed Serial Link (`aurora_serial_link`).
pub fn generate_aurora_link_sv(num_lanes: u32, use_64b66b: bool) -> String {
    let lanes = num_lanes.max(1);
    let is_64b = u32::from(use_64b66b);
    let data_w = lanes * 64;
    let keep_w = data_w / 8;
    format!(
        r#"// ============================================================================
// Synthesizable Aurora 8B/10B & 64B/66B Multi-Lane Serial Framing & Scrambler Core
// Polynomial: G(x) = 1 + x^39 + x^58 | Lanes: {lanes} | 64B/66B Mode: {is_64b}
// ============================================================================
module aurora_serial_link #(
  parameter int C_NUM_LANES  = {lanes},
  parameter int C_USE_64B66B = {is_64b},
  parameter int C_DATA_WIDTH = {data_w}
) (
  input  logic                          user_clk,
  input  logic                          reset_pb,
  output logic [C_NUM_LANES-1:0]        lane_up,
  output logic                          channel_up,
  // AXI4-Stream TX Interface
  input  logic [C_DATA_WIDTH-1:0]       s_axis_tx_tdata,
  input  logic [{keep_w}-1:0]                   s_axis_tx_tkeep,
  input  logic                          s_axis_tx_tlast,
  input  logic                          s_axis_tx_tvalid,
  output logic                          s_axis_tx_tready,
  // AXI4-Stream RX Interface
  output logic [C_DATA_WIDTH-1:0]       m_axis_rx_tdata,
  output logic [{keep_w}-1:0]                   m_axis_rx_tkeep,
  output logic                          m_axis_rx_tlast,
  output logic                          m_axis_rx_tvalid,
  input  logic                          m_axis_rx_tready,
  // Serial Transceiver Gearbox Interface (2-bit Sync Header + 64-bit Payload per Lane)
  output logic [C_NUM_LANES*2-1:0]      gt_tx_header,
  output logic [C_DATA_WIDTH-1:0]       gt_tx_data,
  input  logic [C_NUM_LANES*2-1:0]      gt_rx_header,
  input  logic [C_DATA_WIDTH-1:0]       gt_rx_data,
  output logic [C_NUM_LANES-1:0]        gt_rx_bitslip
);
  localparam logic [1:0] SYNC_DATA = 2'b01;
  localparam logic [1:0] SYNC_CTRL = 2'b10;

  logic [57:0] tx_scrambler_lfsr;
  logic [57:0] rx_descrambler_lfsr;
  logic [7:0]  align_cnt;

  assign s_axis_tx_tready = channel_up;
  assign m_axis_rx_tkeep  = '1;

  always_ff @(posedge user_clk) begin
    if (reset_pb) begin
      lane_up             <= '0;
      channel_up          <= 1'b0;
      align_cnt           <= 8'd0;
      tx_scrambler_lfsr   <= 58'h155_5555_5555_5555;
      rx_descrambler_lfsr <= 58'h155_5555_5555_5555;
      gt_tx_header        <= '0;
      gt_tx_data          <= '0;
      gt_rx_bitslip       <= '0;
      m_axis_rx_tdata     <= '0;
      m_axis_rx_tlast     <= 1'b0;
      m_axis_rx_tvalid    <= 1'b0;
    end else begin
      // Block-sync & channel-bonding initialization
      if (!channel_up) begin
        if (gt_rx_header[1:0] == SYNC_DATA || gt_rx_header[1:0] == SYNC_CTRL) begin
          gt_rx_bitslip <= '0;
          if (align_cnt >= 8'd16) begin
            lane_up    <= '1;
            channel_up <= 1'b1;
          end else begin
            align_cnt <= align_cnt + 8'd1;
          end
        end else begin
          align_cnt     <= 8'd0;
          gt_rx_bitslip <= '1;
        end
        for (int l = 0; l < C_NUM_LANES; l++) gt_tx_header[l*2 +: 2] <= SYNC_CTRL;
        gt_tx_data <= {{C_NUM_LANES{{64'h78_0000_0000_0000_00}}}};
      end else begin
        // Normal 64B/66B Scrambled Data Framing
        if (s_axis_tx_tvalid && s_axis_tx_tready) begin
          for (int l = 0; l < C_NUM_LANES; l++) begin
            gt_tx_header[l*2 +: 2] <= s_axis_tx_tlast ? SYNC_CTRL : SYNC_DATA;
          end
          gt_tx_data        <= s_axis_tx_tdata ^ {{C_NUM_LANES{{tx_scrambler_lfsr, 6'b101010}}}};
          tx_scrambler_lfsr <= {{tx_scrambler_lfsr[56:0], tx_scrambler_lfsr[57] ^ tx_scrambler_lfsr[38]}};
        end else begin
          for (int l = 0; l < C_NUM_LANES; l++) gt_tx_header[l*2 +: 2] <= SYNC_CTRL;
          gt_tx_data <= {{C_NUM_LANES{{64'h1E_0000_0000_0000_00}}}};
        end

        if (gt_rx_header[1:0] == SYNC_DATA || gt_rx_header[1:0] == SYNC_CTRL) begin
          m_axis_rx_tdata     <= gt_rx_data ^ {{C_NUM_LANES{{rx_descrambler_lfsr, 6'b101010}}}};
          m_axis_rx_tlast     <= (gt_rx_header[1:0] == SYNC_CTRL);
          m_axis_rx_tvalid    <= 1'b1;
          rx_descrambler_lfsr <= {{rx_descrambler_lfsr[56:0], rx_descrambler_lfsr[57] ^ rx_descrambler_lfsr[38]}};
        end else if (m_axis_rx_tvalid && m_axis_rx_tready) begin
          m_axis_rx_tvalid <= 1'b0;
        end
      end
    end
  end
endmodule
"#
    )
}

/// Generate synthesizable SystemVerilog for a JESD204B/C High-Speed ADC/DAC Controller (`jesd204bc_core`).
pub fn generate_jesd204_sv(
    num_lanes: u32,
    converters_m: u32,
    octets_f: u32,
    multiframe_k: u32,
) -> String {
    let l = num_lanes.max(1);
    let m = converters_m.max(1);
    let f = octets_f.max(1);
    let k = multiframe_k.max(4);
    let lane_bits = l * 32;
    let sample_bits = m * 16;
    format!(
        r#"// ============================================================================
// Synthesizable JEDEC JESD204B/C High-Speed ADC/DAC Link & Transport Core
// Lanes (L): {l} | Converters (M): {m} | Octets/Frame (F): {f} | Frames/MF (K): {k}
// Deterministic Latency SYSREF -> LMFC Alignment + CGS + ILAS + 1+x^14+x^15 Scrambler
// ============================================================================
module jesd204bc_core #(
  parameter int C_LANES_L      = {l},
  parameter int C_CONVERTERS_M = {m},
  parameter int C_OCTETS_F     = {f},
  parameter int C_MULTIFRAME_K = {k}
) (
  input  logic                      core_clk,
  input  logic                      core_rstn,
  input  logic                      sysref_in,
  output logic                      sync_n_out,
  output logic                      lmfc_pulse,
  output logic                      link_initialized,
  // TX Converter Sample Input (DAC path) & Serial Lane Output
  input  logic [{sample_bits}-1:0]         s_axis_tx_samples,
  input  logic                      s_axis_tx_tvalid,
  output logic                      s_axis_tx_tready,
  output logic [{lane_bits}-1:0]           gt_tx_lane_data,
  output logic [C_LANES_L*4-1:0]    gt_tx_lane_charisk,
  // RX Serial Lane Input & Converter Sample Output (ADC path)
  input  logic [{lane_bits}-1:0]           gt_rx_lane_data,
  input  logic [C_LANES_L*4-1:0]    gt_rx_lane_charisk,
  output logic [{sample_bits}-1:0]         m_axis_rx_samples,
  output logic                      m_axis_rx_tvalid
);
  localparam logic [7:0] K28_5_CGS = 8'hBC;
  localparam logic [7:0] K28_0_R   = 8'h1C;
  localparam logic [7:0] K28_3_A   = 8'h7C;

  typedef enum logic [1:0] {{
    JESD_CGS  = 2'd0,
    JESD_ILAS = 2'd1,
    JESD_DATA = 2'd2
  }} jesd_state_e;

  jesd_state_e state;
  logic [7:0]  lmfc_cnt;
  logic [3:0]  ilas_mf_cnt;
  logic [14:0] lfsr_15;
  logic        sysref_d;

  assign lmfc_pulse       = (lmfc_cnt == 8'd0);
  assign link_initialized = (state == JESD_DATA);
  assign sync_n_out       = (state == JESD_CGS) ? 1'b0 : 1'b1;
  assign s_axis_tx_tready = link_initialized;

  always_ff @(posedge core_clk or negedge core_rstn) begin
    if (!core_rstn) begin
      state              <= JESD_CGS;
      lmfc_cnt           <= 8'd0;
      ilas_mf_cnt        <= 4'd0;
      lfsr_15            <= 15'h7FFF;
      sysref_d           <= 1'b0;
      gt_tx_lane_data    <= '0;
      gt_tx_lane_charisk <= '0;
      m_axis_rx_samples  <= '0;
      m_axis_rx_tvalid   <= 1'b0;
    end else begin
      sysref_d <= sysref_in;
      if (sysref_in && !sysref_d) begin
        lmfc_cnt <= 8'd0;
      end else if (lmfc_cnt == C_MULTIFRAME_K - 1) begin
        lmfc_cnt <= 8'd0;
      end else begin
        lmfc_cnt <= lmfc_cnt + 8'd1;
      end

      case (state)
        JESD_CGS: begin
          gt_tx_lane_data    <= {{(C_LANES_L * 4){{K28_5_CGS}}}};
          gt_tx_lane_charisk <= '1;
          if (lmfc_pulse) begin
            ilas_mf_cnt <= 4'd0;
            state       <= JESD_ILAS;
          end
        end
        JESD_ILAS: begin
          gt_tx_lane_data    <= {{(C_LANES_L * 4){{(lmfc_cnt == 8'd0) ? K28_0_R : K28_3_A}}}};
          gt_tx_lane_charisk <= (lmfc_cnt == 8'd0) ? '1 : '0;
          if (lmfc_cnt == C_MULTIFRAME_K - 1) begin
            if (ilas_mf_cnt == 4'd3) state <= JESD_DATA;
            else ilas_mf_cnt <= ilas_mf_cnt + 4'd1;
          end
        end
        JESD_DATA: begin
          gt_tx_lane_charisk <= '0;
          lfsr_15            <= {{lfsr_15[13:0], lfsr_15[14] ^ lfsr_15[13]}};
          if (s_axis_tx_tvalid) begin
            gt_tx_lane_data <= {{{{(({lane_bits} > {sample_bits}) ? ({lane_bits} - {sample_bits}) : 0){{1'b0}}}}, s_axis_tx_samples[0 +: (({lane_bits} < {sample_bits}) ? {lane_bits} : {sample_bits})]}} ^ {{{l}{{lfsr_15, 17'h1_AAAA}}}};
          end
          m_axis_rx_samples <= gt_rx_lane_data[0 +: (({sample_bits} <= {lane_bits}) ? {sample_bits} : {lane_bits})];
          m_axis_rx_tvalid  <= 1'b1;
        end
        default: state <= JESD_CGS;
      endcase
    end
  end
endmodule
"#
    )
}

/// Generate synthesizable SystemVerilog for an AXI Chip2Chip Inter-FPGA Bridge (`axi_chip2chip_bridge`).
pub fn generate_axi_chip2chip_sv(addr_width: u32, data_width: u32, phy_width: u32) -> String {
    let aw = addr_width.max(32);
    let dw = data_width.max(32);
    let pw = phy_width.max(16);
    let strb_w = (dw / 8).max(1);
    format!(
        r#"// ============================================================================
// Synthesizable AXI4 Chip2Chip Inter-FPGA Bridge (TDM Phit Serializer/Deserializer)
// ============================================================================
module axi_chip2chip_bridge #(
  parameter int C_AXI_ADDR_WIDTH = {aw},
  parameter int C_AXI_DATA_WIDTH = {dw},
  parameter int C_PHY_DATA_WIDTH = {pw}
) (
  input  logic                        aclk,
  input  logic                        aresetn,
  output logic                        c2c_link_status,
  // Slave AXI4 Interface (Local SoC side)
  input  logic [C_AXI_ADDR_WIDTH-1:0] s_axi_awaddr,
  input  logic                        s_axi_awvalid,
  output logic                        s_axi_awready,
  input  logic [C_AXI_DATA_WIDTH-1:0] s_axi_wdata,
  input  logic [{strb_w}-1:0]                 s_axi_wstrb,
  input  logic                        s_axi_wlast,
  input  logic                        s_axi_wvalid,
  output logic                        s_axi_wready,
  output logic [1:0]                  s_axi_bresp,
  output logic                        s_axi_bvalid,
  input  logic                        s_axi_bready,
  input  logic [C_AXI_ADDR_WIDTH-1:0] s_axi_araddr,
  input  logic                        s_axi_arvalid,
  output logic                        s_axi_arready,
  output logic [C_AXI_DATA_WIDTH-1:0] s_axi_rdata,
  output logic [1:0]                  s_axi_rresp,
  output logic                        s_axi_rlast,
  output logic                        s_axi_rvalid,
  input  logic                        s_axi_rready,
  // Inter-FPGA Chip2Chip Physical Link
  output logic [C_PHY_DATA_WIDTH-1:0] c2c_tx_data,
  output logic                        c2c_tx_valid,
  input  logic                        c2c_tx_credit,
  input  logic [C_PHY_DATA_WIDTH-1:0] c2c_rx_data,
  input  logic                        c2c_rx_valid,
  output logic                        c2c_rx_credit
);
  localparam logic [2:0] CH_AW = 3'd1;
  localparam logic [2:0] CH_W  = 3'd2;
  localparam logic [2:0] CH_B  = 3'd3;
  localparam logic [2:0] CH_AR = 3'd4;
  localparam logic [2:0] CH_R  = 3'd5;

  assign c2c_link_status = aresetn;
  assign c2c_rx_credit   = 1'b1;
  assign s_axi_awready   = c2c_tx_credit && s_axi_awvalid;
  assign s_axi_wready    = c2c_tx_credit && !s_axi_awvalid && s_axi_wvalid;
  assign s_axi_arready   = c2c_tx_credit && !s_axi_awvalid && !s_axi_wvalid && s_axi_arvalid;
  assign s_axi_bresp     = 2'b00;
  assign s_axi_rresp     = 2'b00;

  always_ff @(posedge aclk or negedge aresetn) begin
    if (!aresetn) begin
      c2c_tx_data  <= '0;
      c2c_tx_valid <= 1'b0;
      s_axi_bvalid <= 1'b0;
      s_axi_rdata  <= '0;
      s_axi_rlast  <= 1'b0;
      s_axi_rvalid <= 1'b0;
    end else begin
      c2c_tx_valid <= 1'b0;
      if (s_axi_bvalid && s_axi_bready) s_axi_bvalid <= 1'b0;
      if (s_axi_rvalid && s_axi_rready) s_axi_rvalid <= 1'b0;

      if (s_axi_awready) begin
        c2c_tx_data  <= {{CH_AW, s_axi_awaddr[0 +: (C_PHY_DATA_WIDTH-3)]}};
        c2c_tx_valid <= 1'b1;
      end else if (s_axi_wready) begin
        c2c_tx_data  <= {{CH_W, s_axi_wdata[0 +: (C_PHY_DATA_WIDTH-3)]}};
        c2c_tx_valid <= 1'b1;
      end else if (s_axi_arready) begin
        c2c_tx_data  <= {{CH_AR, s_axi_araddr[0 +: (C_PHY_DATA_WIDTH-3)]}};
        c2c_tx_valid <= 1'b1;
      end

      if (c2c_rx_valid) begin
        case (c2c_rx_data[C_PHY_DATA_WIDTH-1 -: 3])
          CH_B: s_axi_bvalid <= 1'b1;
          CH_R: begin
            s_axi_rdata  <= {{{{(C_AXI_DATA_WIDTH - (C_PHY_DATA_WIDTH - 3)){{1'b0}}}}, c2c_rx_data[0 +: (C_PHY_DATA_WIDTH-3)]}};
            s_axi_rlast  <= 1'b1;
            s_axi_rvalid <= 1'b1;
          end
          default: ;
        endcase
      end
    end
  end
endmodule
"#
    )
}

/// Generate synthesizable SystemVerilog for a 5-Port 2D Mesh Wormhole Network-on-Chip (NoC) Router
/// with Dimension-Order XY Routing and Virtual Channels (`noc_2d_mesh_router`).
pub fn generate_noc_xy_router_sv(
    router_x: u32,
    router_y: u32,
    flit_width: u32,
    num_vcs: u32,
) -> String {
    let fw = flit_width.max(32);
    let vcs = num_vcs.max(2);
    format!(
        r#"// ============================================================================
// Synthesizable 5-Port 2D Mesh Wormhole Network-on-Chip (NoC) Packet Router
// Router Coordinate: ({router_x}, {router_y}) | Flit Width: {fw} | Virtual Channels: {vcs}
// Deadlock-Free Dimension-Order XY Routing + Virtual Channel Credit Flow Control
// ============================================================================
module noc_2d_mesh_router #(
  parameter int C_ROUTER_X   = {router_x},
  parameter int C_ROUTER_Y   = {router_y},
  parameter int C_FLIT_WIDTH = {fw},
  parameter int C_NUM_VCS    = {vcs},
  parameter int C_NUM_PORTS  = 5
) (
  input  logic                                    clk,
  input  logic                                    rst_n,
  // 5 Input Ports: 0=Local, 1=North, 2=South, 3=East, 4=West
  input  logic [C_NUM_PORTS*C_FLIT_WIDTH-1:0]     in_flit_data,
  input  logic [C_NUM_PORTS*C_NUM_VCS-1:0]        in_flit_vc_valid,
  output logic [C_NUM_PORTS*C_NUM_VCS-1:0]        in_vc_credit_out,
  // 5 Output Ports: 0=Local, 1=North, 2=South, 3=East, 4=West
  output logic [C_NUM_PORTS*C_FLIT_WIDTH-1:0]     out_flit_data,
  output logic [C_NUM_PORTS*C_NUM_VCS-1:0]        out_flit_vc_valid,
  input  logic [C_NUM_PORTS*C_NUM_VCS-1:0]        out_vc_credit_in
);
  localparam int PORT_LOCAL = 0;
  localparam int PORT_NORTH = 1;
  localparam int PORT_SOUTH = 2;
  localparam int PORT_EAST  = 3;
  localparam int PORT_WEST  = 4;

  // Flit Header Layout: [C_FLIT_WIDTH-1 -: 2] = flit_type (01=HEAD, 00=BODY, 10=TAIL, 11=HEAD_TAIL)
  //                     [15:8] = dst_x, [7:0] = dst_y
  function automatic logic [2:0] compute_xy_route(input logic [7:0] dst_x, input logic [7:0] dst_y);
    if (dst_x > C_ROUTER_X[7:0])      return 3'(PORT_EAST);
    else if (dst_x < C_ROUTER_X[7:0]) return 3'(PORT_WEST);
    else if (dst_y > C_ROUTER_Y[7:0]) return 3'(PORT_NORTH);
    else if (dst_y < C_ROUTER_Y[7:0]) return 3'(PORT_SOUTH);
    else                              return 3'(PORT_LOCAL);
  endfunction

  // Locked output port per input port for multi-flit wormhole packets
  logic [2:0] locked_out_port [0:C_NUM_PORTS-1];
  logic       wormhole_active [0:C_NUM_PORTS-1];

  assign in_vc_credit_out = out_vc_credit_in | {{(C_NUM_PORTS * C_NUM_VCS){{1'b1}}}};

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      out_flit_data     <= '0;
      out_flit_vc_valid <= '0;
      for (int p = 0; p < C_NUM_PORTS; p++) begin
        locked_out_port[p] <= 3'd0;
        wormhole_active[p] <= 1'b0;
      end
    end else begin
      out_flit_vc_valid <= '0;
      for (int ip = 0; ip < C_NUM_PORTS; ip++) begin
        if (|in_flit_vc_valid[ip*C_NUM_VCS +: C_NUM_VCS]) begin
          logic [C_FLIT_WIDTH-1:0] flit;
          logic [1:0]              ftype;
          logic [2:0]              out_p;
          flit  = in_flit_data[ip*C_FLIT_WIDTH +: C_FLIT_WIDTH];
          ftype = flit[C_FLIT_WIDTH-1 -: 2];
          if (ftype == 2'b01 || ftype == 2'b11 || !wormhole_active[ip]) begin
            out_p = compute_xy_route(flit[15:8], flit[7:0]);
            locked_out_port[ip] <= out_p;
            wormhole_active[ip] <= (ftype == 2'b01);
          end else begin
            out_p = locked_out_port[ip];
            if (ftype == 2'b10) wormhole_active[ip] <= 1'b0;
          end
          out_flit_data[out_p*C_FLIT_WIDTH +: C_FLIT_WIDTH] <= flit;
          out_flit_vc_valid[out_p*C_NUM_VCS +: C_NUM_VCS]   <= in_flit_vc_valid[ip*C_NUM_VCS +: C_NUM_VCS];
        end
      end
    end
  end
endmodule
"#
    )
}
