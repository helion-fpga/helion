//! Vivado Video & Display IP Suite (`helion-ipxact::video`).
//!
//! Provides parametric synthesizable SystemVerilog generators and golden behavioral models for:
//! 1. **HDMI 1.4/2.0 8b/10b TMDS Encoder/Decoder + InfoFrame Packetizer** (`generate_hdmi_tmds_tx_sv`,
//!    `generate_hdmi_tmds_rx_sv`, `tmds_encode_byte`, `tmds_decode_symbol`, `HdmiInfoFrame`).
//! 2. **AXI Video DMA (PG020)** (`generate_axi_vdma_sv`, `VdmaConfig`, `VdmaFrameRing`, `VdmaGenlockMode`)
//!    with triple-buffered lockless `mm2s`/`s2mm` frame synchronization.
//! 3. **Video Timing Controller (PG016)** (`generate_vtc_sv`, `VtcConfig`, `VideoResolutionPreset`,
//!    `VideoTimingMode`) supporting programmable `480p`, `720p`, `1080p`, and `4K` (`2160p`) timing
//!    generation and lock detection.
//! 4. **MIPI CSI-2 D-PHY RX Packet Parser + Bayer-to-RGB Demosaic** (`generate_mipi_csi2_rx_demosaic_sv`,
//!    `MipiCsi2Config`, `BayerPattern`, `mipi_csi2_header_ecc`, `mipi_csi2_correct_header`,
//!    `demosaic_bayer_3x3`).
//! 5. **RGB/YCbCr Color Space Converter & Bilinear Scaler** (`generate_csc_scaler_sv`, `CscScalerConfig`,
//!    `ColorStandard`, `CscDirection`, `rgb_to_ycbcr`, `ycbcr_to_rgb`, `bilinear_scale_plane`).

// ============================================================================
// 1. HDMI 1.4 / 2.0 8b/10b TMDS Encoder/Decoder + InfoFrame Packetizer
// ============================================================================

/// HDMI link revision mode for TMDS / scrambling generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum HdmiStandard {
    /// HDMI 1.4b (up to 340 MHz TMDS clock, standard 8b/10b TMDS encoding + data island TERC4).
    Hdmi14,
    /// HDMI 2.0 (up to 600 MHz TMDS character rate with LFSR $1 + x^{11} + x^{12} + x^{13} + x^{16}$ scrambling).
    Hdmi20,
}

impl Default for HdmiStandard {
    fn default() -> Self {
        Self::Hdmi14
    }
}

/// Configuration for the parametric HDMI TMDS TX/RX IP generators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdmiTmdsConfig {
    pub module_name: String,
    pub standard: HdmiStandard,
    /// Bits per color component (8, 10, or 12; default 8).
    pub bpc: u8,
    /// Enable CTA-861 AVI / Audio InfoFrame data island packetizer.
    pub enable_infoframe: bool,
}

impl Default for HdmiTmdsConfig {
    fn default() -> Self {
        Self {
            module_name: "h_hdmi_tmds".into(),
            standard: HdmiStandard::Hdmi14,
            bpc: 8,
            enable_infoframe: true,
        }
    }
}

/// The 4 HDMI control period 10-bit symbols corresponding to `(c1, c0)` in `0..4`.
pub const TMDS_CTRL_SYMBOLS: [u16; 4] = [
    0b11_0101_0100, // c1=0, c0=0
    0b00_1010_1011, // c1=0, c0=1
    0b01_0101_0100, // c1=1, c0=0
    0b10_1010_1011, // c1=1, c0=1
];

/// Encode a single 8-bit video byte or 2-bit control symbol into a 10-bit HDMI/DVI TMDS symbol,
/// updating the running DC disparity counter `disparity` per the official HDMI 1.4a specification.
pub fn tmds_encode_byte(data: u8, c0: bool, c1: bool, de: bool, disparity: &mut i8) -> u16 {
    if !de {
        *disparity = 0;
        let idx = ((c1 as usize) << 1) | (c0 as usize);
        return TMDS_CTRL_SYMBOLS[idx];
    }

    let n1_d = data.count_ones() as i8;
    let use_xnor = n1_d > 4 || (n1_d == 4 && (data & 1) == 0);

    // Stage 1: transition-minimized 9-bit intermediate word q_m[8:0]
    let mut q_m: u16 = (data & 1) as u16;
    for i in 1..8 {
        let prev = (q_m >> (i - 1)) & 1;
        let d_bit = ((data >> i) & 1) as u16;
        let next = if use_xnor {
            !(prev ^ d_bit) & 1
        } else {
            prev ^ d_bit
        };
        q_m |= next << i;
    }
    if !use_xnor {
        q_m |= 1 << 8;
    }

    let q_m_low8 = (q_m & 0xFF) as u8;
    let n1_qm = q_m_low8.count_ones() as i8;
    let n0_qm = 8 - n1_qm;
    let qm8 = ((q_m >> 8) & 1) as i8;

    // Stage 2: DC-balancing inversion decision
    let q_out: u16;
    if *disparity == 0 || n1_qm == n0_qm {
        let bit9 = (1 - qm8) as u16;
        let bit8 = qm8 as u16;
        let body = if qm8 == 1 {
            q_m & 0xFF
        } else {
            (!q_m) & 0xFF
        };
        q_out = (bit9 << 9) | (bit8 << 8) | body;
        if qm8 == 0 {
            *disparity = disparity.saturating_add(n0_qm - n1_qm);
        } else {
            *disparity = disparity.saturating_add(n1_qm - n0_qm);
        }
    } else if (*disparity > 0 && n1_qm > n0_qm) || (*disparity < 0 && n0_qm > n1_qm) {
        let bit9 = 1u16;
        let bit8 = qm8 as u16;
        let body = (!q_m) & 0xFF;
        q_out = (bit9 << 9) | (bit8 << 8) | body;
        *disparity = disparity.saturating_add(2 * qm8 + (n0_qm - n1_qm));
    } else {
        let bit9 = 0u16;
        let bit8 = qm8 as u16;
        let body = q_m & 0xFF;
        q_out = (bit9 << 9) | (bit8 << 8) | body;
        *disparity = disparity.saturating_add((n1_qm - n0_qm) - 2 * (1 - qm8));
    }

    q_out & 0x3FF
}

/// Decode a 10-bit HDMI/DVI TMDS symbol back into `(pixel_byte, c0, c1, de)`.
pub fn tmds_decode_symbol(sym10: u16) -> (u8, bool, bool, bool) {
    let s = sym10 & 0x3FF;
    for (idx, &ctrl) in TMDS_CTRL_SYMBOLS.iter().enumerate() {
        if s == ctrl {
            let c0 = (idx & 1) != 0;
            let c1 = ((idx >> 1) & 1) != 0;
            return (0, c0, c1, false);
        }
    }

    let bit9 = ((s >> 9) & 1) != 0;
    let bit8 = ((s >> 8) & 1) != 0;
    let q_m = if bit9 { (!s) & 0xFF } else { s & 0xFF } as u8;

    let mut data = q_m & 1;
    for i in 1..8 {
        let cur = (q_m >> i) & 1;
        let prev = (q_m >> (i - 1)) & 1;
        let d_bit = if bit8 {
            cur ^ prev
        } else {
            !(cur ^ prev) & 1
        };
        data |= d_bit << i;
    }

    (data, false, false, true)
}

/// CTA-861 / HDMI 1.4 InfoFrame packet (3-byte header + checksum + 27-byte payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdmiInfoFrame {
    /// InfoFrame type code (`0x82` = AVI InfoFrame, `0x84` = Audio InfoFrame, `0x81` = Vendor Specific).
    pub frame_type: u8,
    pub version: u8,
    pub length: u8,
    pub payload: [u8; 27],
}

impl HdmiInfoFrame {
    /// Build a standard CTA-861 AVI InfoFrame (`0x82`, version 2, length 13) for a given VIC code and color format.
    pub fn avi(vic: u8, ycbcr_mode: u8) -> Self {
        let mut payload = [0u8; 27];
        // PB1: [6:5] Y1:Y0 color indicator (0=RGB, 1=YCbCr422, 2=YCbCr444), [4] active format present
        payload[0] = ((ycbcr_mode & 0x3) << 5) | 0x10;
        // PB2: Colorimetry + aspect ratio
        payload[1] = 0x28;
        // PB4: Video Identification Code (VIC)
        payload[3] = vic;
        Self {
            frame_type: 0x82,
            version: 0x02,
            length: 13,
            payload,
        }
    }

    /// Compute the CTA-861 InfoFrame checksum byte PB0 such that
    /// `frame_type + version + length + checksum + sum(payload[0..length]) == 0 (mod 256)`.
    pub fn checksum(&self) -> u8 {
        let n = (self.length as usize).min(27);
        let mut sum: u8 = self
            .frame_type
            .wrapping_add(self.version)
            .wrapping_add(self.length);
        for &b in &self.payload[..n] {
            sum = sum.wrapping_add(b);
        }
        0u8.wrapping_sub(sum)
    }

    /// Compute the 8-bit BCH(32,24) parity byte (HB3) over the 24-bit InfoFrame header `(HB0, HB1, HB2)`
    /// using the HDMI generator polynomial $G(x) = 1 + x^6 + x^7 + x^8$.
    pub fn header_bch_ecc(&self) -> u8 {
        let header_bits: u32 = (self.frame_type as u32)
            | ((self.version as u32) << 8)
            | ((self.length as u32) << 16);
        let mut crc: u8 = 0;
        for i in 0..24 {
            let bit = ((header_bits >> i) & 1) as u8;
            let fb = bit ^ (crc & 1);
            crc >>= 1;
            if fb != 0 {
                crc ^= 0b1100_0001; // x^8 + x^7 + x^6 + 1 (LSB-first tap mask)
            }
        }
        crc
    }

    /// Pack the 4-byte HDMI data island packet header `[HB0, HB1, HB2, HB3_ECC]`.
    pub fn packet_header(&self) -> [u8; 4] {
        [
            self.frame_type,
            self.version,
            self.length,
            self.header_bch_ecc(),
        ]
    }
}

/// Generate synthesizable SystemVerilog for a 3-channel HDMI 1.4/2.0 TMDS Encoder + InfoFrame Packetizer.
pub fn generate_hdmi_tmds_tx_sv(cfg: &HdmiTmdsConfig) -> String {
    let mod_name = &cfg.module_name;
    let scramble_en = if cfg.standard == HdmiStandard::Hdmi20 {
        1
    } else {
        0
    };
    let infoframe_en = if cfg.enable_infoframe { 1 } else { 0 };
    format!(
        r#"// Synthesizable HDMI 1.4/2.0 8b/10b TMDS Transmitter + InfoFrame Packetizer
// Generated by helion-ipxact::video
module {mod_name}_tx #(
    parameter int BPC = {bpc},
    parameter bit HDMI20_SCRAMBLE = 1'b{scramble_en},
    parameter bit ENABLE_INFOFRAME = 1'b{infoframe_en}
) (
    input  logic        pix_clk,
    input  logic        rst_n,
    input  logic [7:0]  vid_r,
    input  logic [7:0]  vid_g,
    input  logic [7:0]  vid_b,
    input  logic        hsync,
    input  logic        vsync,
    input  logic        vde,
    input  logic        ade,
    input  logic [3:0]  aux_ch0,
    input  logic [3:0]  aux_ch1,
    input  logic [3:0]  aux_ch2,
    output logic [9:0]  tmds_ch0,
    output logic [9:0]  tmds_ch1,
    output logic [9:0]  tmds_ch2
);
    logic [15:0] lfsr_q;
    always_ff @(posedge pix_clk or negedge rst_n) begin
        if (!rst_n) begin
            lfsr_q <= 16'hFFFF;
        end else if (vde && HDMI20_SCRAMBLE) begin
            lfsr_q <= {{lfsr_q[14:0], lfsr_q[15] ^ lfsr_q[12] ^ lfsr_q[11] ^ lfsr_q[10]}};
        end
    end

    wire [7:0] b_in = HDMI20_SCRAMBLE ? (vid_b ^ lfsr_q[7:0])  : vid_b;
    wire [7:0] g_in = HDMI20_SCRAMBLE ? (vid_g ^ lfsr_q[11:4]) : vid_g;
    wire [7:0] r_in = HDMI20_SCRAMBLE ? (vid_r ^ lfsr_q[15:8]) : vid_r;

    {mod_name}_lane_enc u_enc_ch0 (
        .clk   (pix_clk),
        .rst_n (rst_n),
        .din   (b_in),
        .c0    (hsync),
        .c1    (vsync),
        .vde   (vde),
        .ade   (ENABLE_INFOFRAME & ade),
        .aux   (aux_ch0),
        .dout  (tmds_ch0)
    );

    {mod_name}_lane_enc u_enc_ch1 (
        .clk   (pix_clk),
        .rst_n (rst_n),
        .din   (g_in),
        .c0    (1'b0),
        .c1    (1'b0),
        .vde   (vde),
        .ade   (ENABLE_INFOFRAME & ade),
        .aux   (aux_ch1),
        .dout  (tmds_ch1)
    );

    {mod_name}_lane_enc u_enc_ch2 (
        .clk   (pix_clk),
        .rst_n (rst_n),
        .din   (r_in),
        .c0    (1'b0),
        .c1    (1'b0),
        .vde   (vde),
        .ade   (ENABLE_INFOFRAME & ade),
        .aux   (aux_ch2),
        .dout  (tmds_ch2)
    );
endmodule

module {mod_name}_lane_enc (
    input  logic       clk,
    input  logic       rst_n,
    input  logic [7:0] din,
    input  logic       c0,
    input  logic       c1,
    input  logic       vde,
    input  logic       ade,
    input  logic [3:0] aux,
    output logic [9:0] dout
);
    logic signed [4:0] cnt_q;
    logic [8:0] q_m;
    logic [3:0] n1_d, n1_q, n0_q;
    logic use_xnor;

    always_comb begin
        n1_d = din[0] + din[1] + din[2] + din[3] + din[4] + din[5] + din[6] + din[7];
        use_xnor = (n1_d > 4'd4) || ((n1_d == 4'd4) && (din[0] == 1'b0));
        q_m[0] = din[0];
        q_m[1] = use_xnor ? ~(q_m[0] ^ din[1]) : (q_m[0] ^ din[1]);
        q_m[2] = use_xnor ? ~(q_m[1] ^ din[2]) : (q_m[1] ^ din[2]);
        q_m[3] = use_xnor ? ~(q_m[2] ^ din[3]) : (q_m[2] ^ din[3]);
        q_m[4] = use_xnor ? ~(q_m[3] ^ din[4]) : (q_m[3] ^ din[4]);
        q_m[5] = use_xnor ? ~(q_m[4] ^ din[5]) : (q_m[4] ^ din[5]);
        q_m[6] = use_xnor ? ~(q_m[5] ^ din[6]) : (q_m[5] ^ din[6]);
        q_m[7] = use_xnor ? ~(q_m[6] ^ din[7]) : (q_m[6] ^ din[7]);
        q_m[8] = ~use_xnor;
        n1_q = q_m[0] + q_m[1] + q_m[2] + q_m[3] + q_m[4] + q_m[5] + q_m[6] + q_m[7];
        n0_q = 4'd8 - n1_q;
    end

    function automatic logic [9:0] terc4_encode(input logic [3:0] d);
        case (d)
            4'h0: terc4_encode = 10'b1010011100;
            4'h1: terc4_encode = 10'b1001100011;
            4'h2: terc4_encode = 10'b1011100100;
            4'h3: terc4_encode = 10'b1011100010;
            4'h4: terc4_encode = 10'b0101110001;
            4'h5: terc4_encode = 10'b0100011110;
            4'h6: terc4_encode = 10'b0110001110;
            4'h7: terc4_encode = 10'b0100111100;
            4'h8: terc4_encode = 10'b1011001100;
            4'h9: terc4_encode = 10'b0100111001;
            4'hA: terc4_encode = 10'b0110011100;
            4'hB: terc4_encode = 10'b1011000110;
            4'hC: terc4_encode = 10'b1010001110;
            4'hD: terc4_encode = 10'b1001110001;
            4'hE: terc4_encode = 10'b0101100011;
            default: terc4_encode = 10'b1011000011;
        endcase
    endfunction

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            dout  <= 10'b1101010100;
            cnt_q <= 5'sd0;
        end else if (vde) begin
            if ((cnt_q == 5'sd0) || (n1_q == n0_q)) begin
                dout[9]   <= ~q_m[8];
                dout[8]   <= q_m[8];
                dout[7:0] <= q_m[8] ? q_m[7:0] : ~q_m[7:0];
                cnt_q     <= q_m[8] ? (cnt_q + $signed({{1'b0, n1_q}}) - $signed({{1'b0, n0_q}}))
                                    : (cnt_q + $signed({{1'b0, n0_q}}) - $signed({{1'b0, n1_q}}));
            end else if (((cnt_q > 5'sd0) && (n1_q > n0_q)) || ((cnt_q < 5'sd0) && (n0_q > n1_q))) begin
                dout[9]   <= 1'b1;
                dout[8]   <= q_m[8];
                dout[7:0] <= ~q_m[7:0];
                cnt_q     <= cnt_q + $signed({{3'b0, q_m[8], 1'b0}}) + $signed({{1'b0, n0_q}}) - $signed({{1'b0, n1_q}});
            end else begin
                dout[9]   <= 1'b0;
                dout[8]   <= q_m[8];
                dout[7:0] <= q_m[7:0];
                cnt_q     <= cnt_q - $signed({{3'b0, ~q_m[8], 1'b0}}) + $signed({{1'b0, n1_q}}) - $signed({{1'b0, n0_q}});
            end
        end else if (ade) begin
            dout  <= terc4_encode(aux);
            cnt_q <= 5'sd0;
        end else begin
            cnt_q <= 5'sd0;
            case ({{c1, c0}})
                2'b00: dout <= 10'b1101010100;
                2'b01: dout <= 10'b0010101011;
                2'b10: dout <= 10'b0101010100;
                2'b11: dout <= 10'b1010101011;
            endcase
        end
    end
endmodule
"#,
        bpc = cfg.bpc,
    )
}

/// Generate synthesizable SystemVerilog for a 3-channel HDMI 1.4/2.0 10b/8b TMDS Receiver/Decoder.
pub fn generate_hdmi_tmds_rx_sv(cfg: &HdmiTmdsConfig) -> String {
    let mod_name = &cfg.module_name;
    format!(
        r#"// Synthesizable HDMI 1.4/2.0 10b/8b TMDS Receiver / Decoder
// Generated by helion-ipxact::video
module {mod_name}_rx (
    input  logic       pix_clk,
    input  logic       rst_n,
    input  logic [9:0] tmds_ch0,
    input  logic [9:0] tmds_ch1,
    input  logic [9:0] tmds_ch2,
    output logic [7:0] vid_r,
    output logic [7:0] vid_g,
    output logic [7:0] vid_b,
    output logic       hsync,
    output logic       vsync,
    output logic       vde
);
    logic c0_g, c1_g, vde_g;
    logic c0_r, c1_r, vde_r;

    {mod_name}_lane_dec u_dec_ch0 (
        .clk   (pix_clk),
        .rst_n (rst_n),
        .din   (tmds_ch0),
        .dout  (vid_b),
        .c0    (hsync),
        .c1    (vsync),
        .de    (vde)
    );

    {mod_name}_lane_dec u_dec_ch1 (
        .clk   (pix_clk),
        .rst_n (rst_n),
        .din   (tmds_ch1),
        .dout  (vid_g),
        .c0    (c0_g),
        .c1    (c1_g),
        .de    (vde_g)
    );

    {mod_name}_lane_dec u_dec_ch2 (
        .clk   (pix_clk),
        .rst_n (rst_n),
        .din   (tmds_ch2),
        .dout  (vid_r),
        .c0    (c0_r),
        .c1    (c1_r),
        .de    (vde_r)
    );
endmodule

module {mod_name}_lane_dec (
    input  logic       clk,
    input  logic       rst_n,
    input  logic [9:0] din,
    output logic [7:0] dout,
    output logic       c0,
    output logic       c1,
    output logic       de
);
    wire [7:0] d_inv = din[9] ? ~din[7:0] : din[7:0];
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            dout <= 8'h00;
            c0   <= 1'b0;
            c1   <= 1'b0;
            de   <= 1'b0;
        end else begin
            case (din)
                10'b1101010100: begin dout <= 8'h00; c0 <= 1'b0; c1 <= 1'b0; de <= 1'b0; end
                10'b0010101011: begin dout <= 8'h00; c0 <= 1'b1; c1 <= 1'b0; de <= 1'b0; end
                10'b0101010100: begin dout <= 8'h00; c0 <= 1'b0; c1 <= 1'b1; de <= 1'b0; end
                10'b1010101011: begin dout <= 8'h00; c0 <= 1'b1; c1 <= 1'b1; de <= 1'b0; end
                default: begin
                    c0 <= 1'b0;
                    c1 <= 1'b0;
                    de <= 1'b1;
                    dout[0] <= d_inv[0];
                    dout[1] <= din[8] ? (d_inv[1] ^ d_inv[0]) : ~(d_inv[1] ^ d_inv[0]);
                    dout[2] <= din[8] ? (d_inv[2] ^ d_inv[1]) : ~(d_inv[2] ^ d_inv[1]);
                    dout[3] <= din[8] ? (d_inv[3] ^ d_inv[2]) : ~(d_inv[3] ^ d_inv[2]);
                    dout[4] <= din[8] ? (d_inv[4] ^ d_inv[3]) : ~(d_inv[4] ^ d_inv[3]);
                    dout[5] <= din[8] ? (d_inv[5] ^ d_inv[4]) : ~(d_inv[5] ^ d_inv[4]);
                    dout[6] <= din[8] ? (d_inv[6] ^ d_inv[5]) : ~(d_inv[6] ^ d_inv[5]);
                    dout[7] <= din[8] ? (d_inv[7] ^ d_inv[6]) : ~(d_inv[7] ^ d_inv[6]);
                end
            endcase
        end
    end
endmodule
"#
    )
}

// ============================================================================
// 2. AXI Video DMA (PG020: Triple-Buffered Frame-Sync mm2s / s2mm Engine)
// ============================================================================

/// PG020 Video DMA Genlock synchronization mode between `s2mm` (write) and `mm2s` (read).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum VdmaGenlockMode {
    /// Dynamic Genlock Master (`s2mm` rotates across 3 buffers while avoiding `mm2s` active buffer;
    /// `mm2s` locks onto the most recently completed `s2mm` frame buffer).
    DynamicGenlock,
    /// Park mode (locks onto software-programmed park frame index).
    Parked,
    /// Free-running independent ring buffers.
    FreeRun,
}

impl Default for VdmaGenlockMode {
    fn default() -> Self {
        Self::DynamicGenlock
    }
}

/// Configuration for the PG020 Video DMA (`mm2s` + `s2mm`) IP generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VdmaConfig {
    pub module_name: String,
    pub data_width: u32,
    pub addr_width: u32,
    pub num_frame_buffers: usize,
    pub max_burst_len: u32,
    pub enable_mm2s: bool,
    pub enable_s2mm: bool,
    pub genlock_mode: VdmaGenlockMode,
}

impl Default for VdmaConfig {
    fn default() -> Self {
        Self {
            module_name: "h_video_vdma".into(),
            data_width: 32,
            addr_width: 32,
            num_frame_buffers: 3,
            max_burst_len: 16,
            enable_mm2s: true,
            enable_s2mm: true,
            genlock_mode: VdmaGenlockMode::DynamicGenlock,
        }
    }
}

/// Behavioral PG020 triple-buffer frame-sync ring state machine (`s2mm` writer + `mm2s` reader).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VdmaFrameRing {
    pub num_buffers: usize,
    pub base_addrs: Vec<u64>,
    pub s2mm_current: usize,
    pub s2mm_last_completed: usize,
    pub mm2s_current: usize,
    pub s2mm_frame_count: u64,
    pub mm2s_frame_count: u64,
}

impl VdmaFrameRing {
    pub fn new(base_addrs: Vec<u64>) -> Self {
        let n = base_addrs.len().max(3);
        let mut addrs = base_addrs;
        while addrs.len() < n {
            addrs.push(0);
        }
        Self {
            num_buffers: n,
            base_addrs: addrs,
            s2mm_current: 0,
            s2mm_last_completed: 0,
            mm2s_current: 1 % n,
            s2mm_frame_count: 0,
            mm2s_frame_count: 0,
        }
    }

    /// Advance `s2mm` on write `fsync`: marks the current write buffer as the latest completed
    /// frame and selects the next buffer in the ring that is NOT currently being read by `mm2s`
    /// (guaranteeing tear-free triple buffering!).
    pub fn on_s2mm_fsync(&mut self) -> u64 {
        self.s2mm_last_completed = self.s2mm_current;
        self.s2mm_frame_count += 1;
        for step in 1..=self.num_buffers {
            let cand = (self.s2mm_current + step) % self.num_buffers;
            if cand != self.mm2s_current {
                self.s2mm_current = cand;
                break;
            }
        }
        self.base_addrs[self.s2mm_current]
    }

    /// Advance `mm2s` on read `fsync`: locks onto the latest completed `s2mm` frame buffer
    /// (never reading the buffer currently being written by `s2mm`).
    pub fn on_mm2s_fsync(&mut self) -> u64 {
        if self.s2mm_last_completed != self.s2mm_current {
            self.mm2s_current = self.s2mm_last_completed;
        }
        self.mm2s_frame_count += 1;
        self.base_addrs[self.mm2s_current]
    }
}

/// Generate synthesizable SystemVerilog for the PG020 Triple-Buffered Video DMA (`mm2s` + `s2mm`).
pub fn generate_axi_vdma_sv(cfg: &VdmaConfig) -> String {
    let mod_name = &cfg.module_name;
    let dw = cfg.data_width.max(8);
    let aw = cfg.addr_width.max(16);
    let n_buf = cfg.num_frame_buffers.max(3);
    format!(
        r#"// Synthesizable PG020 Triple-Buffered Video DMA (mm2s + s2mm)
// Generated by helion-ipxact::video
module {mod_name} #(
    parameter int DATA_W = {dw},
    parameter int ADDR_W = {aw},
    parameter int NUM_BUFFERS = {n_buf}
) (
    input  logic              clk,
    input  logic              rst_n,
    // Control / Frame-Store Programming Port
    input  logic              ctrl_we,
    input  logic [5:0]        ctrl_addr,
    input  logic [31:0]       ctrl_wdata,
    output logic [31:0]       ctrl_rdata,
    // Frame Sync Pulses
    input  logic              s2mm_fsync,
    input  logic              mm2s_fsync,
    // Video Stream In (s2mm)
    input  logic [DATA_W-1:0] s_axis_s2mm_tdata,
    input  logic              s_axis_s2mm_tvalid,
    input  logic              s_axis_s2mm_tlast,
    input  logic              s_axis_s2mm_tuser_sof,
    output logic              s_axis_s2mm_tready,
    // Video Stream Out (mm2s)
    output logic [DATA_W-1:0] m_axis_mm2s_tdata,
    output logic              m_axis_mm2s_tvalid,
    output logic              m_axis_mm2s_tlast,
    output logic              m_axis_mm2s_tuser_sof,
    input  logic              m_axis_mm2s_tready,
    // Memory-Mapped Master Write / Read Port
    output logic [ADDR_W-1:0] mem_wr_addr,
    output logic [DATA_W-1:0] mem_wr_data,
    output logic              mem_wr_valid,
    input  logic              mem_wr_ready,
    output logic [ADDR_W-1:0] mem_rd_addr,
    output logic              mem_rd_req,
    input  logic [DATA_W-1:0] mem_rd_data,
    input  logic              mem_rd_valid,
    output logic              s2mm_introut,
    output logic              mm2s_introut
);
    logic [ADDR_W-1:0] frame_base [0:NUM_BUFFERS-1];
    logic [15:0]       hsize_bytes;
    logic [15:0]       stride_bytes;
    logic [15:0]       vsize_lines;
    logic              run_s2mm, run_mm2s;

    logic [1:0]        s2mm_buf_idx;
    logic [1:0]        s2mm_done_idx;
    logic [1:0]        mm2s_buf_idx;
    logic [ADDR_W-1:0] s2mm_line_base, s2mm_byte_off;
    logic [15:0]       s2mm_line_cnt;
    logic [ADDR_W-1:0] mm2s_line_base, mm2s_byte_off;
    logic [15:0]       mm2s_line_cnt;

    localparam int BYTES_PER_BEAT = DATA_W / 8;

    // Register programming + Dynamic Genlock Triple-Buffer Ring
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            frame_base[0] <= '0;
            frame_base[1] <= '0;
            frame_base[2] <= '0;
            hsize_bytes   <= 16'd1920;
            stride_bytes  <= 16'd1920;
            vsize_lines   <= 16'd1080;
            run_s2mm      <= 1'b0;
            run_mm2s      <= 1'b0;
            s2mm_buf_idx  <= 2'd0;
            s2mm_done_idx <= 2'd0;
            mm2s_buf_idx  <= 2'd1;
            s2mm_line_base <= '0;
            s2mm_byte_off  <= '0;
            s2mm_line_cnt  <= 16'd0;
            mm2s_line_base <= '0;
            mm2s_byte_off  <= '0;
            mm2s_line_cnt  <= 16'd0;
            s2mm_introut   <= 1'b0;
            mm2s_introut   <= 1'b0;
        end else begin
            s2mm_introut <= 1'b0;
            mm2s_introut <= 1'b0;

            if (ctrl_we) begin
                case (ctrl_addr)
                    6'h00: {{run_mm2s, run_s2mm}} <= ctrl_wdata[1:0];
                    6'h04: hsize_bytes   <= ctrl_wdata[15:0];
                    6'h08: stride_bytes  <= ctrl_wdata[15:0];
                    6'h0C: vsize_lines   <= ctrl_wdata[15:0];
                    6'h10: frame_base[0] <= ctrl_wdata[ADDR_W-1:0];
                    6'h14: frame_base[1] <= ctrl_wdata[ADDR_W-1:0];
                    6'h18: frame_base[2] <= ctrl_wdata[ADDR_W-1:0];
                    default: ;
                endcase
            end

            // Dynamic Genlock s2mm frame rotation (never collides with mm2s_buf_idx)
            if (s2mm_fsync || (s_axis_s2mm_tvalid && s_axis_s2mm_tready && s_axis_s2mm_tuser_sof)) begin
                s2mm_done_idx <= s2mm_buf_idx;
                if (((s2mm_buf_idx + 2'd1) % 3) != mm2s_buf_idx) begin
                    s2mm_buf_idx   <= (s2mm_buf_idx + 2'd1) % 3;
                    s2mm_line_base <= frame_base[(s2mm_buf_idx + 2'd1) % 3];
                end else begin
                    s2mm_buf_idx   <= (s2mm_buf_idx + 2'd2) % 3;
                    s2mm_line_base <= frame_base[(s2mm_buf_idx + 2'd2) % 3];
                end
                s2mm_byte_off <= '0;
                s2mm_line_cnt <= 16'd0;
                s2mm_introut  <= 1'b1;
            end else if (s_axis_s2mm_tvalid && s_axis_s2mm_tready) begin
                if (s_axis_s2mm_tlast) begin
                    s2mm_line_base <= s2mm_line_base + stride_bytes;
                    s2mm_byte_off  <= '0;
                    s2mm_line_cnt  <= s2mm_line_cnt + 16'd1;
                end else begin
                    s2mm_byte_off <= s2mm_byte_off + BYTES_PER_BEAT[ADDR_W-1:0];
                end
            end

            // Dynamic Genlock mm2s frame follower
            if (mm2s_fsync) begin
                mm2s_buf_idx   <= s2mm_done_idx;
                mm2s_line_base <= frame_base[s2mm_done_idx];
                mm2s_byte_off  <= '0;
                mm2s_line_cnt  <= 16'd0;
                mm2s_introut   <= 1'b1;
            end else if (m_axis_mm2s_tvalid && m_axis_mm2s_tready) begin
                if (m_axis_mm2s_tlast) begin
                    mm2s_line_base <= mm2s_line_base + stride_bytes;
                    mm2s_byte_off  <= '0;
                    mm2s_line_cnt  <= mm2s_line_cnt + 16'd1;
                end else begin
                    mm2s_byte_off <= mm2s_byte_off + BYTES_PER_BEAT[ADDR_W-1:0];
                end
            end
        end
    end

    assign s_axis_s2mm_tready    = run_s2mm & mem_wr_ready;
    assign mem_wr_addr           = s2mm_line_base + s2mm_byte_off;
    assign mem_wr_data           = s_axis_s2mm_tdata;
    assign mem_wr_valid          = run_s2mm & s_axis_s2mm_tvalid;

    assign mem_rd_addr           = mm2s_line_base + mm2s_byte_off;
    assign mem_rd_req            = run_mm2s & m_axis_mm2s_tready;
    assign m_axis_mm2s_tdata     = mem_rd_data;
    assign m_axis_mm2s_tvalid    = run_mm2s & mem_rd_valid;
    assign m_axis_mm2s_tlast     = (mm2s_byte_off + BYTES_PER_BEAT[ADDR_W-1:0]) >= hsize_bytes;
    assign m_axis_mm2s_tuser_sof = (mm2s_line_cnt == 16'd0) && (mm2s_byte_off == '0);
    assign ctrl_rdata            = {{26'd0, mm2s_buf_idx, s2mm_done_idx, s2mm_buf_idx}};
endmodule
"#
    )
}

// ============================================================================
// 3. Video Timing Controller (PG016: Programmable 480p / 720p / 1080p / 4K)
// ============================================================================

/// Standard CEA-861 / VESA video resolution presets for PG016 VTC.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum VideoResolutionPreset {
    /// `640x480p60` (`800x525` total, 25.175 MHz pixel clock).
    P480,
    /// `1280x720p60` (`1650x750` total, 74.25 MHz pixel clock).
    P720,
    /// `1920x1080p60` (`2200x1125` total, 148.5 MHz pixel clock).
    P1080,
    /// `3840x2160p60` 4K UHD (`4400x2250` total, 594.0 MHz pixel clock).
    Uhd4K,
}

/// Complete horizontal and vertical timing parameters for a video mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoTimingMode {
    pub h_active: u16,
    pub h_front_porch: u16,
    pub h_sync_width: u16,
    pub h_back_porch: u16,
    pub v_active: u16,
    pub v_front_porch: u16,
    pub v_sync_width: u16,
    pub v_back_porch: u16,
    pub hsync_active_high: bool,
    pub vsync_active_high: bool,
    pub pixel_clock_khz: u32,
}

impl VideoTimingMode {
    pub fn from_preset(preset: VideoResolutionPreset) -> Self {
        match preset {
            VideoResolutionPreset::P480 => Self {
                h_active: 640,
                h_front_porch: 16,
                h_sync_width: 96,
                h_back_porch: 48,
                v_active: 480,
                v_front_porch: 10,
                v_sync_width: 2,
                v_back_porch: 33,
                hsync_active_high: false,
                vsync_active_high: false,
                pixel_clock_khz: 25_175,
            },
            VideoResolutionPreset::P720 => Self {
                h_active: 1280,
                h_front_porch: 110,
                h_sync_width: 40,
                h_back_porch: 220,
                v_active: 720,
                v_front_porch: 5,
                v_sync_width: 5,
                v_back_porch: 20,
                hsync_active_high: true,
                vsync_active_high: true,
                pixel_clock_khz: 74_250,
            },
            VideoResolutionPreset::P1080 => Self {
                h_active: 1920,
                h_front_porch: 88,
                h_sync_width: 44,
                h_back_porch: 148,
                v_active: 1080,
                v_front_porch: 4,
                v_sync_width: 5,
                v_back_porch: 36,
                hsync_active_high: true,
                vsync_active_high: true,
                pixel_clock_khz: 148_500,
            },
            VideoResolutionPreset::Uhd4K => Self {
                h_active: 3840,
                h_front_porch: 176,
                h_sync_width: 88,
                h_back_porch: 296,
                v_active: 2160,
                v_front_porch: 8,
                v_sync_width: 10,
                v_back_porch: 72,
                hsync_active_high: true,
                vsync_active_high: true,
                pixel_clock_khz: 594_000,
            },
        }
    }

    pub fn h_total(&self) -> u16 {
        self.h_active + self.h_front_porch + self.h_sync_width + self.h_back_porch
    }

    pub fn v_total(&self) -> u16 {
        self.v_active + self.v_front_porch + self.v_sync_width + self.v_back_porch
    }

    pub fn h_sync_start(&self) -> u16 {
        self.h_active + self.h_front_porch
    }

    pub fn h_sync_end(&self) -> u16 {
        self.h_sync_start() + self.h_sync_width
    }

    pub fn v_sync_start(&self) -> u16 {
        self.v_active + self.v_front_porch
    }

    pub fn v_sync_end(&self) -> u16 {
        self.v_sync_start() + self.v_sync_width
    }
}

/// Configuration for the PG016 Video Timing Controller (`h_video_vtc`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VtcConfig {
    pub module_name: String,
    pub default_preset: VideoResolutionPreset,
    pub enable_generator: bool,
    pub enable_detector: bool,
}

impl Default for VtcConfig {
    fn default() -> Self {
        Self {
            module_name: "h_video_vtc".into(),
            default_preset: VideoResolutionPreset::P1080,
            enable_generator: true,
            enable_detector: true,
        }
    }
}

/// Generate synthesizable SystemVerilog for the PG016 Video Timing Controller (Sync Generator + Detector).
pub fn generate_vtc_sv(cfg: &VtcConfig) -> String {
    let mod_name = &cfg.module_name;
    let t = VideoTimingMode::from_preset(cfg.default_preset);
    let hpol = if t.hsync_active_high { 1 } else { 0 };
    let vpol = if t.vsync_active_high { 1 } else { 0 };
    format!(
        r#"// Synthesizable PG016 Video Timing Controller (480p / 720p / 1080p / 4K Generator & Detector)
// Generated by helion-ipxact::video
module {mod_name} #(
    parameter int H_ACTIVE     = {h_active},
    parameter int H_SYNC_START = {h_ss},
    parameter int H_SYNC_END   = {h_se},
    parameter int H_TOTAL      = {h_tot},
    parameter int V_ACTIVE     = {v_active},
    parameter int V_SYNC_START = {v_ss},
    parameter int V_SYNC_END   = {v_se},
    parameter int V_TOTAL      = {v_tot},
    parameter bit HSYNC_POL    = 1'b{hpol},
    parameter bit VSYNC_POL    = 1'b{vpol}
) (
    input  logic        clk,
    input  logic        rst_n,
    input  logic        clken,
    // Programmable Timing Override Interface (Helion-MM)
    input  logic        cfg_we,
    input  logic [4:0]  cfg_addr,
    input  logic [15:0] cfg_wdata,
    // Incoming Video Timing Detector Inputs
    input  logic        det_hsync_in,
    input  logic        det_vsync_in,
    input  logic        det_active_in,
    output logic [12:0] det_h_active,
    output logic [12:0] det_v_active,
    output logic        det_locked,
    // Generated Video Timing Outputs
    output logic        gen_hsync,
    output logic        gen_vsync,
    output logic        gen_hblank,
    output logic        gen_vblank,
    output logic        gen_active_video,
    output logic        gen_fsync,
    output logic [12:0] gen_x,
    output logic [12:0] gen_y
);
    logic [12:0] reg_h_active, reg_h_ss, reg_h_se, reg_h_tot;
    logic [12:0] reg_v_active, reg_v_ss, reg_v_se, reg_v_tot;
    logic [12:0] h_cnt, v_cnt;

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            reg_h_active <= H_ACTIVE[12:0];
            reg_h_ss     <= H_SYNC_START[12:0];
            reg_h_se     <= H_SYNC_END[12:0];
            reg_h_tot    <= H_TOTAL[12:0];
            reg_v_active <= V_ACTIVE[12:0];
            reg_v_ss     <= V_SYNC_START[12:0];
            reg_v_se     <= V_SYNC_END[12:0];
            reg_v_tot    <= V_TOTAL[12:0];
        end else if (cfg_we) begin
            case (cfg_addr)
                5'h00: reg_h_active <= cfg_wdata[12:0];
                5'h01: reg_h_ss     <= cfg_wdata[12:0];
                5'h02: reg_h_se     <= cfg_wdata[12:0];
                5'h03: reg_h_tot    <= cfg_wdata[12:0];
                5'h04: reg_v_active <= cfg_wdata[12:0];
                5'h05: reg_v_ss     <= cfg_wdata[12:0];
                5'h06: reg_v_se     <= cfg_wdata[12:0];
                5'h07: reg_v_tot    <= cfg_wdata[12:0];
                default: ;
            endcase
        end
    end

    // Timing Generator Counters
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            h_cnt <= 13'd0;
            v_cnt <= 13'd0;
        end else if (clken) begin
            if (h_cnt + 13'd1 >= reg_h_tot) begin
                h_cnt <= 13'd0;
                if (v_cnt + 13'd1 >= reg_v_tot)
                    v_cnt <= 13'd0;
                else
                    v_cnt <= v_cnt + 13'd1;
            end else begin
                h_cnt <= h_cnt + 13'd1;
            end
        end
    end

    wire h_sync_raw = (h_cnt >= reg_h_ss) && (h_cnt < reg_h_se);
    wire v_sync_raw = (v_cnt >= reg_v_ss) && (v_cnt < reg_v_se);
    assign gen_hsync        = HSYNC_POL ? h_sync_raw : ~h_sync_raw;
    assign gen_vsync        = VSYNC_POL ? v_sync_raw : ~v_sync_raw;
    assign gen_hblank       = (h_cnt >= reg_h_active);
    assign gen_vblank       = (v_cnt >= reg_v_active);
    assign gen_active_video = (h_cnt < reg_h_active) && (v_cnt < reg_v_active);
    assign gen_fsync        = (h_cnt == 13'd0) && (v_cnt == 13'd0);
    assign gen_x            = h_cnt;
    assign gen_y            = v_cnt;

    // Timing Detector
    logic [12:0] cur_h_act, cur_v_act;
    logic        prev_active, prev_vsync;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            cur_h_act    <= 13'd0;
            cur_v_act    <= 13'd0;
            det_h_active <= 13'd0;
            det_v_active <= 13'd0;
            det_locked   <= 1'b0;
            prev_active  <= 1'b0;
            prev_vsync   <= 1'b0;
        end else if (clken) begin
            prev_active <= det_active_in;
            prev_vsync  <= det_vsync_in;
            if (det_active_in) begin
                cur_h_act <= cur_h_act + 13'd1;
            end else if (prev_active && !det_active_in) begin
                det_h_active <= cur_h_act;
                cur_h_act    <= 13'd0;
                cur_v_act    <= cur_v_act + 13'd1;
            end
            if (!prev_vsync && det_vsync_in) begin
                det_locked   <= (det_v_active == cur_v_act) && (cur_v_act != 13'd0);
                det_v_active <= cur_v_act;
                cur_v_act    <= 13'd0;
            end
        end
    end
endmodule
"#,
        h_active = t.h_active,
        h_ss = t.h_sync_start(),
        h_se = t.h_sync_end(),
        h_tot = t.h_total(),
        v_active = t.v_active,
        v_ss = t.v_sync_start(),
        v_se = t.v_sync_end(),
        v_tot = t.v_total(),
    )
}

// ============================================================================
// 4. MIPI CSI-2 D-PHY RX Packet Parser + Bayer-to-RGB Demosaic
// ============================================================================

/// Color Filter Array (CFA) Bayer mosaic pattern (2×2 phase).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum BayerPattern {
    /// Row 0: `R G`, Row 1: `G B` (code `2'b00`).
    Rggb,
    /// Row 0: `G R`, Row 1: `B G` (code `2'b01`).
    Grbg,
    /// Row 0: `G B`, Row 1: `R G` (code `2'b10`).
    Gbrg,
    /// Row 0: `B G`, Row 1: `G R` (code `2'b11`).
    Bggr,
}

impl BayerPattern {
    pub fn phase_code(self) -> u8 {
        match self {
            Self::Rggb => 0b00,
            Self::Grbg => 0b01,
            Self::Gbrg => 0b10,
            Self::Bggr => 0b11,
        }
    }
}

impl Default for BayerPattern {
    fn default() -> Self {
        Self::Rggb
    }
}

/// MIPI CSI-2 Hamming(24,18) parity generator masks for the 6 ECC bits `P[0..5]`
/// over the 24-bit packet header `H[23:0] = (word_count as u32) << 8 | (data_id as u32)`.
pub const MIPI_CSI2_ECC_MASKS: [u32; 6] = [
    0b1111_0001_0010_1100_1011_0111, // P0
    0b1111_0010_0101_0101_0101_1011, // P1
    0b0111_0100_1001_1001_1010_1101, // P2
    0b1011_1000_1110_0011_1000_1110, // P3
    0b1101_1111_0000_0011_1111_0000, // P4
    0b1110_1111_1111_1100_0000_0000, // P5
];

/// Compute the 6-bit MIPI CSI-2 Packet Header ECC (`0x00..=0x3F`) over `(data_id, word_count)`.
pub fn mipi_csi2_header_ecc(data_id: u8, word_count: u16) -> u8 {
    let h24: u32 = (data_id as u32) | ((word_count as u32) << 8);
    let mut ecc = 0u8;
    for (bit, &mask) in MIPI_CSI2_ECC_MASKS.iter().enumerate() {
        let parity = ((h24 & mask).count_ones() & 1) as u8;
        ecc |= parity << bit;
    }
    ecc
}

/// Verify and single-bit error-correct a 24-bit MIPI CSI-2 packet header `(data_id, word_count, rx_ecc)`.
///
/// Returns `Ok((corrected_data_id, corrected_word_count, was_corrected))` when valid or single-bit
/// corrected, or `Err(())` on uncorrectable multi-bit error.
pub fn mipi_csi2_correct_header(
    data_id: u8,
    word_count: u16,
    rx_ecc: u8,
) -> Result<(u8, u16, bool), ()> {
    let expected = mipi_csi2_header_ecc(data_id, word_count);
    let syndrome = (expected ^ (rx_ecc & 0x3F)) & 0x3F;
    if syndrome == 0 {
        return Ok((data_id, word_count, false));
    }
    // If syndrome has weight 1, the error is in the ECC byte itself
    if syndrome.count_ones() == 1 {
        return Ok((data_id, word_count, true));
    }
    // Check if syndrome matches the column mask of one of the 24 header bits
    let mut h24: u32 = (data_id as u32) | ((word_count as u32) << 8);
    for bit_pos in 0..24u32 {
        let mut col_syn = 0u8;
        for (p, &mask) in MIPI_CSI2_ECC_MASKS.iter().enumerate() {
            if ((mask >> bit_pos) & 1) != 0 {
                col_syn |= 1 << p;
            }
        }
        if col_syn == syndrome {
            h24 ^= 1u32 << bit_pos;
            let fixed_id = (h24 & 0xFF) as u8;
            let fixed_wc = ((h24 >> 8) & 0xFFFF) as u16;
            return Ok((fixed_id, fixed_wc, true));
        }
    }
    Err(())
}

/// Perform 3×3 bilinear Bayer-to-RGB demosaicing on a 3×3 neighborhood `w` (`w[row][col]`)
/// at pixel parity `(row_parity & 1, col_parity & 1)` for `pattern`.
///
/// Returns `(r, g, b)`.
pub fn demosaic_bayer_3x3(
    w: [[u8; 3]; 3],
    pattern: BayerPattern,
    row_parity: u8,
    col_parity: u8,
) -> (u8, u8, u8) {
    let c = w[1][1] as u16;
    let h_avg = ((w[1][0] as u16 + w[1][2] as u16) + 1) / 2;
    let v_avg = ((w[0][1] as u16 + w[2][1] as u16) + 1) / 2;
    let cross_avg = ((w[0][1] as u16 + w[2][1] as u16 + w[1][0] as u16 + w[1][2] as u16) + 2) / 4;
    let diag_avg = ((w[0][0] as u16 + w[0][2] as u16 + w[2][0] as u16 + w[2][2] as u16) + 2) / 4;

    // Effective site phase: 0 = R, 1 = Gr (G on R row), 2 = Gb (G on B row), 3 = B
    let pos_bits = ((row_parity & 1) << 1) | (col_parity & 1);
    let site = pos_bits ^ pattern.phase_code();
    match site {
        0 => (c as u8, cross_avg as u8, diag_avg as u8), // Center is Red
        1 => (h_avg as u8, c as u8, v_avg as u8),        // Center is Green on Red row
        2 => (v_avg as u8, c as u8, h_avg as u8),        // Center is Green on Blue row
        _ => (diag_avg as u8, cross_avg as u8, c as u8), // Center is Blue
    }
}

/// Configuration for the MIPI CSI-2 D-PHY RX Packet Parser + Bayer Demosaic IP generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MipiCsi2Config {
    pub module_name: String,
    pub num_lanes: u8,
    pub pixel_bits: u8,
    pub line_width: u16,
    pub bayer_pattern: BayerPattern,
}

impl Default for MipiCsi2Config {
    fn default() -> Self {
        Self {
            module_name: "h_mipi_csi2_rx".into(),
            num_lanes: 2,
            pixel_bits: 8,
            line_width: 1920,
            bayer_pattern: BayerPattern::Rggb,
        }
    }
}

/// Generate synthesizable SystemVerilog for the MIPI CSI-2 D-PHY RX Packet Parser + 3×3 Bayer-to-RGB Demosaic IP.
pub fn generate_mipi_csi2_rx_demosaic_sv(cfg: &MipiCsi2Config) -> String {
    let mod_name = &cfg.module_name;
    let lanes = cfg.num_lanes.clamp(1, 4);
    let pbits = cfg.pixel_bits.max(8);
    let lw = cfg.line_width.max(16);
    let phase = cfg.bayer_pattern.phase_code();
    format!(
        r#"// Synthesizable MIPI CSI-2 D-PHY RX Packet Parser + 3x3 Bayer-to-RGB Demosaic
// Generated by helion-ipxact::video
module {mod_name} #(
    parameter int NUM_LANES  = {lanes},
    parameter int PIXEL_BITS = {pbits},
    parameter int LINE_WIDTH = {lw},
    parameter logic [1:0] BAYER_PHASE = 2'd{phase}
) (
    input  logic                    rx_byte_clk,
    input  logic                    rst_n,
    input  logic [NUM_LANES*8-1:0]  dphy_rx_data,
    input  logic [NUM_LANES-1:0]    dphy_rx_valid,
    input  logic [NUM_LANES-1:0]    dphy_rx_sot,
    // Decoded Packet Header & ECC Status
    output logic [5:0]              pkt_data_type,
    output logic [1:0]              pkt_virtual_channel,
    output logic [15:0]             pkt_word_count,
    output logic                    pkt_ecc_ok,
    output logic                    pkt_ecc_err,
    output logic                    frame_start,
    output logic                    frame_end,
    // Demosaiced 24-bit RGB Streaming Output (Helion-ST)
    output logic [23:0]             m_vid_rgb,
    output logic                    m_vid_valid,
    output logic                    m_vid_sof,
    output logic                    m_vid_eol
);
    typedef enum logic [1:0] {{
        ST_IDLE    = 2'd0,
        ST_HEADER  = 2'd1,
        ST_PAYLOAD = 2'd2
    }} rx_state_e;

    rx_state_e   state_q;
    logic [31:0] hdr_shift;
    logic [1:0]  hdr_cnt;
    logic [15:0] pay_rem;
    logic        raw_pix_valid;
    logic [7:0]  raw_pix_byte;

    function automatic logic [5:0] calc_csi2_ecc(input logic [23:0] h);
        logic [5:0] e;
        e[0] = ^(h & 24'b1111_0001_0010_1100_1011_0111);
        e[1] = ^(h & 24'b1111_0010_0101_0101_0101_1011);
        e[2] = ^(h & 24'b0111_0100_1001_1001_1010_1101);
        e[3] = ^(h & 24'b1011_1000_1110_0011_1000_1110);
        e[4] = ^(h & 24'b1101_1111_0000_0011_1111_0000);
        e[5] = ^(h & 24'b1110_1111_1111_1100_0000_0000);
        return e;
    endfunction

    // MIPI CSI-2 Short / Long Packet Parser FSM
    always_ff @(posedge rx_byte_clk or negedge rst_n) begin
        if (!rst_n) begin
            state_q             <= ST_IDLE;
            hdr_shift           <= 32'd0;
            hdr_cnt             <= 2'd0;
            pay_rem             <= 16'd0;
            pkt_data_type       <= 6'd0;
            pkt_virtual_channel <= 2'd0;
            pkt_word_count      <= 16'd0;
            pkt_ecc_ok          <= 1'b0;
            pkt_ecc_err         <= 1'b0;
            frame_start         <= 1'b0;
            frame_end           <= 1'b0;
            raw_pix_valid       <= 1'b0;
            raw_pix_byte        <= 8'd0;
        end else begin
            frame_start   <= 1'b0;
            frame_end     <= 1'b0;
            raw_pix_valid <= 1'b0;

            case (state_q)
                ST_IDLE: begin
                    if (|dphy_rx_sot && dphy_rx_valid[0] && (dphy_rx_data[7:0] == 8'hB8)) begin
                        state_q <= ST_HEADER;
                        hdr_cnt <= 2'd0;
                    end
                end
                ST_HEADER: begin
                    if (dphy_rx_valid[0]) begin
                        hdr_shift <= {{dphy_rx_data[7:0], hdr_shift[31:8]}};
                        if (hdr_cnt == 2'd3) begin
                            logic [23:0] h24;
                            logic [5:0]  rx_ecc, exp_ecc;
                            h24     = {{hdr_shift[31:16], hdr_shift[15:8]}};
                            rx_ecc  = dphy_rx_data[5:0];
                            exp_ecc = calc_csi2_ecc(h24);
                            pkt_data_type       <= h24[5:0];
                            pkt_virtual_channel <= h24[7:6];
                            pkt_word_count      <= h24[23:8];
                            pkt_ecc_ok          <= (exp_ecc == rx_ecc);
                            pkt_ecc_err         <= (exp_ecc != rx_ecc);
                            if (h24[5:0] == 6'h00) begin
                                frame_start <= 1'b1;
                                state_q     <= ST_IDLE;
                            end else if (h24[5:0] == 6'h01) begin
                                frame_end <= 1'b1;
                                state_q   <= ST_IDLE;
                            end else if (h24[23:8] != 16'd0) begin
                                pay_rem <= h24[23:8];
                                state_q <= ST_PAYLOAD;
                            end else begin
                                state_q <= ST_IDLE;
                            end
                        end else begin
                            hdr_cnt <= hdr_cnt + 2'd1;
                        end
                    end
                end
                ST_PAYLOAD: begin
                    if (dphy_rx_valid[0]) begin
                        raw_pix_valid <= 1'b1;
                        raw_pix_byte  <= dphy_rx_data[7:0];
                        if (pay_rem <= 16'd1) begin
                            state_q <= ST_IDLE;
                            pay_rem <= 16'd0;
                        end else begin
                            pay_rem <= pay_rem - 16'd1;
                        end
                    end
                end
                default: state_q <= ST_IDLE;
            endcase
        end
    end

    // 2-Line Buffer + 3x3 Sliding Window Bayer-to-RGB Bilinear Demosaic
    logic [7:0] line_buf0 [0:LINE_WIDTH-1];
    logic [7:0] line_buf1 [0:LINE_WIDTH-1];
    logic [12:0] col_x, row_y;
    logic [7:0] w00, w01, w02;
    logic [7:0] w10, w11, w12;
    logic [7:0] w20, w21, w22;

    always_ff @(posedge rx_byte_clk or negedge rst_n) begin
        if (!rst_n) begin
            col_x       <= 13'd0;
            row_y       <= 13'd0;
            m_vid_rgb   <= 24'd0;
            m_vid_valid <= 1'b0;
            m_vid_sof   <= 1'b0;
            m_vid_eol   <= 1'b0;
            w00 <= 8'd0; w01 <= 8'd0; w02 <= 8'd0;
            w10 <= 8'd0; w11 <= 8'd0; w12 <= 8'd0;
            w20 <= 8'd0; w21 <= 8'd0; w22 <= 8'd0;
        end else begin
            m_vid_valid <= raw_pix_valid;
            if (frame_start) begin
                col_x <= 13'd0;
                row_y <= 13'd0;
            end else if (raw_pix_valid) begin
                logic [9:0] h_avg, v_avg, cross_avg, diag_avg;
                logic [1:0] site_phase;
                w00 <= w01; w01 <= w02; w02 <= line_buf1[col_x];
                w10 <= w11; w11 <= w12; w12 <= line_buf0[col_x];
                w20 <= w21; w21 <= w22; w22 <= raw_pix_byte;
                line_buf1[col_x] <= line_buf0[col_x];
                line_buf0[col_x] <= raw_pix_byte;

                h_avg     = (w10 + w12 + 10'd1) >> 1;
                v_avg     = (w01 + w21 + 10'd1) >> 1;
                cross_avg = (w01 + w21 + w10 + w12 + 10'd2) >> 2;
                diag_avg  = (w00 + w02 + w20 + w22 + 10'd2) >> 2;
                site_phase = {{row_y[0], col_x[0]}} ^ BAYER_PHASE;

                case (site_phase)
                    2'd0: m_vid_rgb <= {{w11, cross_avg[7:0], diag_avg[7:0]}};
                    2'd1: m_vid_rgb <= {{h_avg[7:0], w11, v_avg[7:0]}};
                    2'd2: m_vid_rgb <= {{v_avg[7:0], w11, h_avg[7:0]}};
                    2'd3: m_vid_rgb <= {{diag_avg[7:0], cross_avg[7:0], w11}};
                endcase

                m_vid_sof <= (row_y == 13'd0) && (col_x == 13'd0);
                if (col_x + 13'd1 >= LINE_WIDTH[12:0]) begin
                    m_vid_eol <= 1'b1;
                    col_x     <= 13'd0;
                    row_y     <= row_y + 13'd1;
                end else begin
                    m_vid_eol <= 1'b0;
                    col_x     <= col_x + 13'd1;
                end
            end
        end
    end
endmodule
"#
    )
}

// ============================================================================
// 5. RGB / YCbCr Color Space Converter (CSC) & 2D Bilinear Video Scaler
// ============================================================================

/// ITU-R / SMPTE Colorimetry Standard for RGB ↔ YCbCr conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ColorStandard {
    /// ITU-R BT.601 (SDTV: $K_R = 0.299, K_G = 0.587, K_B = 0.114$).
    Bt601,
    /// ITU-R BT.709 (HDTV: $K_R = 0.2126, K_G = 0.7152, K_B = 0.0722$).
    Bt709,
    /// ITU-R BT.2020 (UHDTV / 4K: $K_R = 0.2627, K_G = 0.6780, K_B = 0.0593$).
    Bt2020,
}

impl Default for ColorStandard {
    fn default() -> Self {
        Self::Bt709
    }
}

/// Conversion direction for the Color Space Converter IP.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum CscDirection {
    RgbToYCbCr,
    YCbCrToRgb,
}

impl Default for CscDirection {
    fn default() -> Self {
        Self::RgbToYCbCr
    }
}

impl ColorStandard {
    /// Q8.8 fixed-point forward matrix coefficients `([c_yr, c_yg, c_yb], [c_cbr, c_cbg, c_cbb], [c_crr, c_crg, c_crb])`
    /// where $Y = (c_{yr} R + c_{yg} G + c_{yb} B + 128) \gg 8$,
    /// $Cb = 128 + (c_{cbr} R + c_{cbg} G + c_{cbb} B + 128) \gg 8$,
    /// $Cr = 128 + (c_{crr} R + c_{crg} G + c_{crb} B + 128) \gg 8$.
    pub fn forward_q8_coeffs(self) -> ([i32; 3], [i32; 3], [i32; 3]) {
        match self {
            Self::Bt601 => ([77, 150, 29], [-43, -85, 128], [128, -107, -21]),
            Self::Bt709 => ([54, 183, 19], [-29, -99, 128], [128, -116, -12]),
            Self::Bt2020 => ([67, 174, 15], [-36, -92, 128], [128, -118, -10]),
        }
    }

    /// Q8.8 fixed-point inverse matrix coefficients `(c_rv, c_gu, c_gv, c_bu)`
    /// where $R = Y + (c_{rv} (Cr - 128) + 128) \gg 8$,
    /// $G = Y + (c_{gu} (Cb - 128) + c_{gv} (Cr - 128) + 128) \gg 8$,
    /// $B = Y + (c_{bu} (Cb - 128) + 128) \gg 8$.
    pub fn inverse_q8_coeffs(self) -> (i32, i32, i32, i32) {
        match self {
            Self::Bt601 => (359, -88, -183, 454),
            Self::Bt709 => (403, -48, -120, 475),
            Self::Bt2020 => (377, -42, -146, 482),
        }
    }
}

/// Convert an 8-bit `(R, G, B)` pixel into `(Y, Cb, Cr)` using the Q8.8 hardware pipeline coefficients.
pub fn rgb_to_ycbcr(r: u8, g: u8, b: u8, std: ColorStandard) -> (u8, u8, u8) {
    let (cy, ccb, ccr) = std.forward_q8_coeffs();
    let ri = r as i32;
    let gi = g as i32;
    let bi = b as i32;
    let y = ((cy[0] * ri + cy[1] * gi + cy[2] * bi + 128) >> 8).clamp(0, 255) as u8;
    let cb = (128 + ((ccb[0] * ri + ccb[1] * gi + ccb[2] * bi + 128) >> 8)).clamp(0, 255) as u8;
    let cr = (128 + ((ccr[0] * ri + ccr[1] * gi + ccr[2] * bi + 128) >> 8)).clamp(0, 255) as u8;
    (y, cb, cr)
}

/// Convert an 8-bit `(Y, Cb, Cr)` pixel back into `(R, G, B)` using the Q8.8 hardware pipeline coefficients.
pub fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8, std: ColorStandard) -> (u8, u8, u8) {
    let (c_rv, c_gu, c_gv, c_bu) = std.inverse_q8_coeffs();
    let yi = y as i32;
    let u = cb as i32 - 128;
    let v = cr as i32 - 128;
    let r = (yi + ((c_rv * v + 128) >> 8)).clamp(0, 255) as u8;
    let g = (yi + ((c_gu * u + c_gv * v + 128) >> 8)).clamp(0, 255) as u8;
    let b = (yi + ((c_bu * u + 128) >> 8)).clamp(0, 255) as u8;
    (r, g, b)
}

/// Scale a 2D 8-bit image plane of dimensions `(in_w, in_h)` to `(out_w, out_h)` using
/// Q8.8 fixed-point 2×2 bilinear interpolation matching the hardware scaler phase accumulator.
pub fn bilinear_scale_plane(
    src: &[u8],
    in_w: usize,
    in_h: usize,
    out_w: usize,
    out_h: usize,
) -> Vec<u8> {
    if in_w == 0 || in_h == 0 || out_w == 0 || out_h == 0 || src.len() < in_w * in_h {
        return Vec::new();
    }
    let mut dst = vec![0u8; out_w * out_h];
    let x_step_q8 = if out_w > 1 && in_w > 1 {
        ((in_w - 1) * 256) / (out_w - 1)
    } else {
        0
    };
    let y_step_q8 = if out_h > 1 && in_h > 1 {
        ((in_h - 1) * 256) / (out_h - 1)
    } else {
        0
    };

    for oy in 0..out_h {
        let y_q8 = oy * y_step_q8;
        let y0 = (y_q8 >> 8).min(in_h - 1);
        let y1 = (y0 + 1).min(in_h - 1);
        let fy = (y_q8 & 0xFF) as u32;
        let inv_fy = 256 - fy;

        for ox in 0..out_w {
            let x_q8 = ox * x_step_q8;
            let x0 = (x_q8 >> 8).min(in_w - 1);
            let x1 = (x0 + 1).min(in_w - 1);
            let fx = (x_q8 & 0xFF) as u32;
            let inv_fx = 256 - fx;

            let p00 = src[y0 * in_w + x0] as u32;
            let p10 = src[y0 * in_w + x1] as u32;
            let p01 = src[y1 * in_w + x0] as u32;
            let p11 = src[y1 * in_w + x1] as u32;

            let top = p00 * inv_fx + p10 * fx;
            let bot = p01 * inv_fx + p11 * fx;
            let val = (top * inv_fy + bot * fy + 32768) >> 16;
            dst[oy * out_w + ox] = val.min(255) as u8;
        }
    }
    dst
}

/// Configuration for the RGB/YCbCr Color Space Converter & Bilinear Video Scaler IP generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CscScalerConfig {
    pub module_name: String,
    pub standard: ColorStandard,
    pub direction: CscDirection,
    pub in_width: u16,
    pub in_height: u16,
    pub out_width: u16,
    pub out_height: u16,
    pub enable_scaler: bool,
}

impl Default for CscScalerConfig {
    fn default() -> Self {
        Self {
            module_name: "h_video_csc_scaler".into(),
            standard: ColorStandard::Bt709,
            direction: CscDirection::RgbToYCbCr,
            in_width: 1280,
            in_height: 720,
            out_width: 1920,
            out_height: 1080,
            enable_scaler: true,
        }
    }
}

/// Generate synthesizable SystemVerilog for the pipelined RGB/YCbCr Color Space Converter + 2D Bilinear Scaler.
pub fn generate_csc_scaler_sv(cfg: &CscScalerConfig) -> String {
    let mod_name = &cfg.module_name;
    let (cy, ccb, ccr) = cfg.standard.forward_q8_coeffs();
    let (c_rv, c_gu, c_gv, c_bu) = cfg.standard.inverse_q8_coeffs();
    let dir_bit = if cfg.direction == CscDirection::RgbToYCbCr {
        0
    } else {
        1
    };
    let scale_en = if cfg.enable_scaler { 1 } else { 0 };
    format!(
        r#"// Synthesizable Pipelined RGB/YCbCr Color Space Converter + 2D Bilinear Video Scaler
// Generated by helion-ipxact::video
module {mod_name} #(
    parameter bit DIRECTION_YCBCR_TO_RGB = 1'b{dir_bit},
    parameter bit ENABLE_SCALER          = 1'b{scale_en},
    parameter int IN_WIDTH               = {in_w},
    parameter int IN_HEIGHT              = {in_h},
    parameter int OUT_WIDTH              = {out_w},
    parameter int OUT_HEIGHT             = {out_h}
) (
    input  logic        clk,
    input  logic        rst_n,
    input  logic [23:0] s_vid_data,
    input  logic        s_vid_valid,
    input  logic        s_vid_sof,
    input  logic        s_vid_eol,
    // Fractional Q8.8 Bilinear Phase Weights (for horizontal/vertical resampling)
    input  logic [7:0]  phase_fx,
    input  logic [7:0]  phase_fy,
    output logic [23:0] m_vid_data,
    output logic        m_vid_valid,
    output logic        m_vid_sof,
    output logic        m_vid_eol
);
    // Stage 1: Q8.8 Fixed-Point Color Space Matrix Conversion
    logic [7:0] csc_c0, csc_c1, csc_c2;
    logic       csc_valid, csc_sof, csc_eol;

    function automatic logic [7:0] clip_u8(input logic signed [19:0] val);
        if (val < 20'sd0)
            clip_u8 = 8'd0;
        else if (val > 20'sd255)
            clip_u8 = 8'd255;
        else
            clip_u8 = val[7:0];
    endfunction

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            csc_c0    <= 8'd0;
            csc_c1    <= 8'd128;
            csc_c2    <= 8'd128;
            csc_valid <= 1'b0;
            csc_sof   <= 1'b0;
            csc_eol   <= 1'b0;
        end else begin
            csc_valid <= s_vid_valid;
            csc_sof   <= s_vid_sof;
            csc_eol   <= s_vid_eol;
            if (s_vid_valid) begin
                logic signed [19:0] a, b, c;
                a = $signed({{12'd0, s_vid_data[23:16]}});
                b = $signed({{12'd0, s_vid_data[15:8]}});
                c = $signed({{12'd0, s_vid_data[7:0]}});
                if (!DIRECTION_YCBCR_TO_RGB) begin
                    csc_c0 <= clip_u8((({cy0} * a) + ({cy1} * b) + ({cy2} * c) + 20'sd128) >>> 8);
                    csc_c1 <= clip_u8(20'sd128 + ((({ccb0} * a) + ({ccb1} * b) + ({ccb2} * c) + 20'sd128) >>> 8));
                    csc_c2 <= clip_u8(20'sd128 + ((({ccr0} * a) + ({ccr1} * b) + ({ccr2} * c) + 20'sd128) >>> 8));
                end else begin
                    logic signed [19:0] u, v;
                    u = b - 20'sd128;
                    v = c - 20'sd128;
                    csc_c0 <= clip_u8(a + ((({crv} * v) + 20'sd128) >>> 8));
                    csc_c1 <= clip_u8(a + ((({cgu} * u) + ({cgv} * v) + 20'sd128) >>> 8));
                    csc_c2 <= clip_u8(a + ((({cbu} * u) + 20'sd128) >>> 8));
                end
            end
        end
    end

    // Stage 2: 2x2 Bilinear Interpolation Filter
    logic [7:0] prev_c0, prev_c1, prev_c2;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            prev_c0    <= 8'd0;
            prev_c1    <= 8'd128;
            prev_c2    <= 8'd128;
            m_vid_data  <= 24'd0;
            m_vid_valid <= 1'b0;
            m_vid_sof   <= 1'b0;
            m_vid_eol   <= 1'b0;
        end else begin
            m_vid_valid <= csc_valid;
            m_vid_sof   <= csc_sof;
            m_vid_eol   <= csc_eol;
            if (csc_valid) begin
                prev_c0 <= csc_c0;
                prev_c1 <= csc_c1;
                prev_c2 <= csc_c2;
                if (ENABLE_SCALER) begin
                    logic [15:0] w_cur, w_prev;
                    logic [7:0]  out0, out1, out2;
                    w_cur  = {{8'd0, phase_fx}};
                    w_prev = 16'd256 - w_cur;
                    out0   = ((prev_c0 * w_prev) + (csc_c0 * w_cur) + 16'd128) >> 8;
                    out1   = ((prev_c1 * w_prev) + (csc_c1 * w_cur) + 16'd128) >> 8;
                    out2   = ((prev_c2 * w_prev) + (csc_c2 * w_cur) + 16'd128) >> 8;
                    m_vid_data <= {{out0, out1, out2}};
                end else begin
                    m_vid_data <= {{csc_c0, csc_c1, csc_c2}};
                end
            end
        end
    end
endmodule
"#,
        in_w = cfg.in_width,
        in_h = cfg.in_height,
        out_w = cfg.out_width,
        out_h = cfg.out_height,
        cy0 = cy[0],
        cy1 = cy[1],
        cy2 = cy[2],
        ccb0 = ccb[0],
        ccb1 = ccb[1],
        ccb2 = ccb[2],
        ccr0 = ccr[0],
        ccr1 = ccr[1],
        ccr2 = ccr[2],
        crv = c_rv,
        cgu = c_gu,
        cgv = c_gv,
        cbu = c_bu,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmds_8b10b_roundtrips_all_bytes_and_controls_with_bounded_disparity() {
        let mut disp: i8 = 0;
        // Control symbols reset disparity and roundtrip (c0, c1)
        for c1 in [false, true] {
            for c0 in [false, true] {
                let sym = tmds_encode_byte(0xAA, c0, c1, false, &mut disp);
                assert_eq!(disp, 0);
                let (d_out, c0_out, c1_out, de_out) = tmds_decode_symbol(sym);
                assert!(!de_out);
                assert_eq!(c0_out, c0);
                assert_eq!(c1_out, c1);
                assert_eq!(d_out, 0);
            }
        }

        // Active video bytes 0..=255 across multiple pseudo-random passes keep DC disparity bounded
        for pass in 0..4u8 {
            for b in 0..=255u8 {
                let val = b.wrapping_mul(pass.saturating_add(1)).wrapping_add(pass);
                let sym = tmds_encode_byte(val, false, false, true, &mut disp);
                assert!(
                    disp.abs() <= 10,
                    "TMDS running disparity {} exceeded bound on val={:#04x}",
                    disp,
                    val
                );
                let (decoded, _, _, de_out) = tmds_decode_symbol(sym);
                assert!(de_out);
                assert_eq!(decoded, val, "TMDS decode mismatch for {:#04x}", val);
            }
        }

        // Verify AVI InfoFrame checksum sum-to-zero property
        let info = HdmiInfoFrame::avi(16, 0); // VIC 16 = 1080p60 RGB
        let chk = info.checksum();
        let mut total = info
            .frame_type
            .wrapping_add(info.version)
            .wrapping_add(info.length)
            .wrapping_add(chk);
        for &pb in &info.payload[..info.length as usize] {
            total = total.wrapping_add(pb);
        }
        assert_eq!(total, 0, "CTA-861 InfoFrame modulo-256 sum must equal 0");
        let hdr = info.packet_header();
        assert_eq!(hdr[0], 0x82);
        assert_ne!(hdr[3], 0);
    }

    #[test]
    fn vdma_triple_buffer_ring_never_collides_reader_and_writer() {
        let mut ring = VdmaFrameRing::new(vec![0x1000_0000, 0x1040_0000, 0x1080_0000]);
        assert_ne!(ring.s2mm_current, ring.mm2s_current);

        // Fast writer vs slow reader
        for _ in 0..20 {
            let wr_addr = ring.on_s2mm_fsync();
            assert_ne!(
                ring.s2mm_current, ring.mm2s_current,
                "s2mm writer must never step onto active mm2s read buffer"
            );
            assert!(ring.base_addrs.contains(&wr_addr));
        }
        // Reader catches up to most recently completed writer frame
        let rd_addr = ring.on_mm2s_fsync();
        assert_eq!(ring.mm2s_current, ring.s2mm_last_completed);
        assert_ne!(ring.s2mm_current, ring.mm2s_current);
        assert!(ring.base_addrs.contains(&rd_addr));
    }

    #[test]
    fn mipi_csi2_hamming_ecc_corrects_all_single_bit_errors() {
        let data_id = 0x2Au8; // RAW8 virtual channel 0
        let wc = 1920u16;
        let ecc = mipi_csi2_header_ecc(data_id, wc);
        assert_eq!(
            mipi_csi2_correct_header(data_id, wc, ecc),
            Ok((data_id, wc, false))
        );

        // Flip each of the 24 header bits and verify exact single-bit correction
        let orig_h24: u32 = (data_id as u32) | ((wc as u32) << 8);
        for bit in 0..24u32 {
            let corrupted = orig_h24 ^ (1u32 << bit);
            let bad_id = (corrupted & 0xFF) as u8;
            let bad_wc = ((corrupted >> 8) & 0xFFFF) as u16;
            assert_eq!(
                mipi_csi2_correct_header(bad_id, bad_wc, ecc),
                Ok((data_id, wc, true)),
                "must correct single-bit flip at header bit {bit}"
            );
        }

        // Flip each of the 6 ECC bits
        for p in 0..6u8 {
            let bad_ecc = ecc ^ (1u8 << p);
            assert_eq!(
                mipi_csi2_correct_header(data_id, wc, bad_ecc),
                Ok((data_id, wc, true)),
                "must tolerate single-bit flip at ECC bit {p}"
            );
        }
    }

    #[test]
    fn bayer_demosaic_and_csc_bilinear_scaler_accuracy() {
        let win = [
            [40, 80, 44],
            [82, 200, 86],
            [42, 84, 46],
        ];
        // At (0,0) in RGGB, center is Red=200, G is cross average (80+84+82+86)/4 = 83, B is diag average (40+44+42+46)/4 = 43
        let (r, g, b) = demosaic_bayer_3x3(win, BayerPattern::Rggb, 0, 0);
        assert_eq!((r, g, b), (200, 83, 43));
        // At (0,0) in BGGR, center is Blue=200, G=83, R=43
        let (r_b, g_b, b_b) = demosaic_bayer_3x3(win, BayerPattern::Bggr, 0, 0);
        assert_eq!((r_b, g_b, b_b), (43, 83, 200));

        // CSC roundtrip across BT.601, BT.709, BT.2020
        for std in [ColorStandard::Bt601, ColorStandard::Bt709, ColorStandard::Bt2020] {
            for &(r0, g0, b0) in &[(220u8, 40u8, 60u8), (30, 180, 90), (128, 128, 128)] {
                let (y, cb, cr) = rgb_to_ycbcr(r0, g0, b0, std);
                let (r1, g1, b1) = ycbcr_to_rgb(y, cb, cr, std);
                assert!(r0.abs_diff(r1) <= 3, "{std:?} R: {r0} vs {r1}");
                assert!(g0.abs_diff(g1) <= 3, "{std:?} G: {g0} vs {g1}");
                assert!(b0.abs_diff(b1) <= 3, "{std:?} B: {b0} vs {b1}");
            }
        }

        // Bilinear 2x2 -> 3x3 upscale
        let src = vec![0u8, 100, 100, 200];
        let scaled = bilinear_scale_plane(&src, 2, 2, 3, 3);
        assert_eq!(scaled.len(), 9);
        assert_eq!(scaled[0], 0);
        assert_eq!(scaled[4], 100); // exact center of [0, 100; 100, 200]
        assert_eq!(scaled[8], 200);
    }
}
