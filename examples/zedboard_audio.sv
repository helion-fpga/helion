// ZedBoard audio out (ADAU1761) tone demo for the Helion 7-series backend.
// Top: zedboard_audio. Pins are in examples/zedboard_audio.xdc.
//
// Plays a 999 Hz square wave at -18 dBFS on both channels of the
// headphone (HPH OUT) and line-out jacks. Everything runs from the 100 MHz
// GCLK; the PL is the I2S bus master and makes every audio clock:
//   MCLK   = 100 MHz / 8    = 12.5 MHz
//   BCLK   = 100 MHz / 34   = 2.941 MHz (64 BCLK per frame)
//   LRCLK  = 100 MHz / 2176 = 45955.9 Hz (the sample rate, fs)
// The codec's PLL turns MCLK into its core clock, which must be 1024 x fs
// (Linux adau17x1_hw_params: core / rate = 1024 selects CONVSR = fs):
//   1024 x fs = 12.5 MHz x 64 / 17 = 12.5 MHz x (R + N / M), R = 3, N = 13,
//   M = 17, input divider X = 1 (fractional mode).
// The ratio is exact, so the codec's converters run locked to the BCLK and
// LRCLK this design sends. 45.956 kHz lies between the 44.1 kHz and
// 48 kHz rates the codec is normally run at (core 45.158 and 49.152 MHz);
// 12.5 MHz cannot make exactly 48 kHz with an integer BCLK divider.
//
// Register setup over I2C (write-only, 50 kHz; every value cited):
//  - I2C address 0x3B (write byte 0x76): Analog Devices' ZedBoard device
//    tree (analogdevicesinc/linux zynq-zed-adv7511.dtsi, audio-codec@3b)
//    and the TU Kaiserslautern / Mike Field ZedBoard design
//    (ems-kl/zedboard_audio hdl/adau1761_configuraiton_data.vhd, byte 0x76),
//    which also drives AC-ADR0 and AC-ADR1 high (adau1761_izedboard.vhd);
//    this design does the same. The register address is two bytes, MSB
//    first (0x40 then the low byte), in both sources.
//  - Register addresses and bit fields: Linux sound/soc/codecs/adau17x1.h,
//    adau17x1.c and adau1761.c (v6.6, Analog Devices).
//  - Order and the board-proven values: ems-kl/zedboard_audio. That design
//    makes the codec the I2S master and feeds a 24 MHz MCLK; here the PL is
//    master and MCLK is 12.5 MHz, so R15 and the PLL bytes differ.
//    0x4000 = 0x0E  clock control: core clock off, source PLL (bit 3),
//                   INFREQ (bits 2:1) = 11, as in ems-kl
//    0x4002 = 00 11 00 0D 19 01  PLL: M = 0x0011, N = 0x000D, byte 4 =
//                   R << 3 | X-code << 1 | fractional = 0x19, byte 5 =
//                   PLL enable. Encoding from Linux adau_calc_pll_cfg
//                   (adau-utils.c), which gives ems-kl's bytes
//                   00 7D 00 0C 23 01 for its 24 MHz MCLK. The 6 bytes go
//                   in one write (Linux: "can only be written at once").
//    wait 20 ms for the PLL to lock (no read-back: see the I2C note)
//    0x4000 = 0x0F  core clock on (bit 0)
//    0x4015 = 0x00  serial port 0: slave (bit 0 = 0), BCLK and LRCLK
//                   polarity normal (Linux I2S format)
//    0x4016 = 0x00  serial port 1: 64 BCLK per frame, MSB delay 1 (I2S)
//    0x4017 = 0x00  converter 0: CONVSR = fs (core / 1024)
//    0x40F8 = 0x00  serial port sampling rate = fs
//    0x401C = 0x21  left playback mixer: enable (bit 0), left DAC (bit 5)
//    0x401E = 0x41  right playback mixer: enable (bit 0), right DAC (bit 6)
//    0x4023/0x4024 = 0xE7  headphone L/R: volume 0x39 (0 dB, Linux TLV
//                   -57 dB + 1 dB/step), unmute (bit 1), HP enable (bit 0)
//    0x4025/0x4026 = 0xE7  line out L/R, same value as ems-kl
//    0x4029 = 0x03  playback power: left and right enable
//    0x402A = 0x03  DAC control 0: left and right DAC enable
//    0x40F2 = 0x01  serial input route: serial port to DACs (DSP bypass)
//    0x40F9 = 0x7F  clock enable 0: all seven blocks
//    0x40FA = 0x03  clock enable 1: digital clock 0 and 1
//    0x4036 = 0x00, then 0x4036 = 0x03  dejitter: Linux adau1761.c
//                   adau1761_dejitter_fixup re-initialises it this way
//                   after power changes when the codec is the I2S slave
//                   (0x03 is also the driver's reset default); ems-kl skips
//                   it because there the codec is master. Effect not
//                   checked against the datasheet (R67): TODO-UNVERIFIED.
// The ADAU1761 datasheet itself could not be downloaded here (the
// analog.com host refused the connection); INFREQ's meaning with the PLL,
// the PLL lock time, and the PLL input/output ranges are therefore
// TODO-UNVERIFIED against it: see docs/zedboard/rung5-av.md.
//
// I2S framing (Linux I2S mode: LRCLK low = left, MSB one BCLK after the
// LRCLK edge, data and LRCLK change on BCLK falling edges and the codec
// samples on rising edges): each 32-BCLK half frame carries the delay bit,
// a 24-bit two's-complement sample MSB first, then 7 zero bits.
//
// I2C: SCL and SDA are driven push-pull, because Helion's v2 front end
// does not accept an inout port on the top module and has no tristate
// output primitive. The master never reads the bus: it drives SDA low
// itself for each ACK slot, starting 50 ns after SCL falls on the 8th bit,
// the level an acknowledging slave drives. The FPGA can still drive a 1
// against the slave for up to those 50 ns, if the slave pulls SDA low that
// soon after SCL falls. A NACK cannot be seen and there is no ACK-error LED.
// shortcut: no ACK check, no PLL-lock poll and no clock-stretch support,
// upgrade to an open-drain IOBUF once Helion supports inout pins.
//
// led[0] lights when the register list has been sent; led[1] toggles every
// 16384 frames (0.36 s) as a heartbeat that proves the fabric runs.
module zedboard_audio (
    input  logic       clk,
    output logic       ac_mclk,
    output logic       ac_bclk,
    output logic       ac_lrclk,
    output logic       ac_sdata,
    output logic       ac_adr0,
    output logic       ac_adr1,
    // Idle-high I2C lines from configuration on.
    output logic       ac_scl = 1'b1,
    output logic       ac_sda = 1'b1,
    output logic [1:0] led
);
    assign ac_adr0 = 1'b1;
    assign ac_adr1 = 1'b1;

    // ------------------------------------------------------------------
    // I2C writer. ROM entry = {wait after this transaction, last byte of a
    // transaction, byte}.
    // ------------------------------------------------------------------
    localparam int unsigned Q        = 500;       // quarter SCL period: 50 kHz
    localparam int unsigned N_ENT    = 85;        // 20 writes, 85 bytes
    localparam int unsigned T_INIT   = 10_000;    // 100 us after configuration
    localparam int unsigned T_PLL    = 2_000_000; // 20 ms PLL lock wait

    typedef enum logic [2:0] {
        I_GAP,
        I_START,
        I_LOAD,
        I_BIT,
        I_STOP,
        I_DONE
    } istate_t;

    istate_t     ist;
    logic [7:0]  idx;
    logic [9:0]  ent;
    logic [8:0]  qcnt;
    logic [1:0]  q;
    logic [3:0]  bitn;
    logic [8:0]  sh;
    logic [20:0] wcnt = T_INIT[20:0];
    logic        qtick;
    logic        init_done;
    logic        beat;

    assign led = {beat, init_done};

    always_comb begin
        case (idx)
            8'd0: ent = 10'h076;
            8'd1: ent = 10'h040;
            8'd2: ent = 10'h000;
            8'd3: ent = 10'h10e;
            8'd4: ent = 10'h076;
            8'd5: ent = 10'h040;
            8'd6: ent = 10'h002;
            8'd7: ent = 10'h000;
            8'd8: ent = 10'h011;
            8'd9: ent = 10'h000;
            8'd10: ent = 10'h00d;
            8'd11: ent = 10'h019;
            8'd12: ent = 10'h301;
            8'd13: ent = 10'h076;
            8'd14: ent = 10'h040;
            8'd15: ent = 10'h000;
            8'd16: ent = 10'h10f;
            8'd17: ent = 10'h076;
            8'd18: ent = 10'h040;
            8'd19: ent = 10'h015;
            8'd20: ent = 10'h100;
            8'd21: ent = 10'h076;
            8'd22: ent = 10'h040;
            8'd23: ent = 10'h016;
            8'd24: ent = 10'h100;
            8'd25: ent = 10'h076;
            8'd26: ent = 10'h040;
            8'd27: ent = 10'h017;
            8'd28: ent = 10'h100;
            8'd29: ent = 10'h076;
            8'd30: ent = 10'h040;
            8'd31: ent = 10'h0f8;
            8'd32: ent = 10'h100;
            8'd33: ent = 10'h076;
            8'd34: ent = 10'h040;
            8'd35: ent = 10'h01c;
            8'd36: ent = 10'h121;
            8'd37: ent = 10'h076;
            8'd38: ent = 10'h040;
            8'd39: ent = 10'h01e;
            8'd40: ent = 10'h141;
            8'd41: ent = 10'h076;
            8'd42: ent = 10'h040;
            8'd43: ent = 10'h023;
            8'd44: ent = 10'h1e7;
            8'd45: ent = 10'h076;
            8'd46: ent = 10'h040;
            8'd47: ent = 10'h024;
            8'd48: ent = 10'h1e7;
            8'd49: ent = 10'h076;
            8'd50: ent = 10'h040;
            8'd51: ent = 10'h025;
            8'd52: ent = 10'h1e7;
            8'd53: ent = 10'h076;
            8'd54: ent = 10'h040;
            8'd55: ent = 10'h026;
            8'd56: ent = 10'h1e7;
            8'd57: ent = 10'h076;
            8'd58: ent = 10'h040;
            8'd59: ent = 10'h029;
            8'd60: ent = 10'h103;
            8'd61: ent = 10'h076;
            8'd62: ent = 10'h040;
            8'd63: ent = 10'h02a;
            8'd64: ent = 10'h103;
            8'd65: ent = 10'h076;
            8'd66: ent = 10'h040;
            8'd67: ent = 10'h0f2;
            8'd68: ent = 10'h101;
            8'd69: ent = 10'h076;
            8'd70: ent = 10'h040;
            8'd71: ent = 10'h0f9;
            8'd72: ent = 10'h17f;
            8'd73: ent = 10'h076;
            8'd74: ent = 10'h040;
            8'd75: ent = 10'h0fa;
            8'd76: ent = 10'h103;
            8'd77: ent = 10'h076;
            8'd78: ent = 10'h040;
            8'd79: ent = 10'h036;
            8'd80: ent = 10'h100;
            8'd81: ent = 10'h076;
            8'd82: ent = 10'h040;
            8'd83: ent = 10'h036;
            8'd84: ent = 10'h103;
            default: ent = 10'h000;
        endcase
    end

    assign qtick = (qcnt == Q[8:0] - 9'd1);

    always_ff @(posedge clk) begin
        if (ist == I_GAP || ist == I_LOAD || ist == I_DONE || qtick)
            qcnt <= 9'd0;
        else
            qcnt <= qcnt + 9'd1;

        case (ist)
            I_GAP: begin
                // Bus idle (both lines high) for the current wait.
                q <= 2'd0;
                if (wcnt == 21'd0)
                    ist <= (idx == N_ENT[7:0]) ? I_DONE : I_START;
                else
                    wcnt <= wcnt - 21'd1;
            end

            I_START: if (qtick) begin
                // q0: both high, q1: SDA falls (START), q2: SCL falls.
                q <= q + 2'd1;
                if (q == 2'd0) ac_sda <= 1'b0;
                if (q == 2'd1) ac_scl <= 1'b0;
                if (q == 2'd2) ist <= I_LOAD;
            end

            I_LOAD: begin
                // Eight data bits then the ACK slot, driven low.
                sh   <= {ent[7:0], 1'b0};
                bitn <= 4'd0;
                q    <= 2'd0;
                ist  <= I_BIT;
            end

            I_BIT: if (!qtick) begin
                // In q0 SCL is low. NXP UM10204 Rev. 7.0, Table 11: SDA must
                // be valid within 3.45 us of the SCL fall (tVD;DAT, tVD;ACK
                // max) and may not move before SCL is below 0.3 VDD (note
                // [2]). A slave may start its ACK 0 ns after the 8th fall
                // (tHD;DAT min), so the ACK level goes out 50 ns after it.
                // A data bit goes out 2.5 us after the fall: under 3.45 us,
                // late enough for the slave to release the ACK it may hold
                // after the previous byte, and 7.5 us before SCL rises.
                if (q == 2'd0 && bitn == 4'd8 && qcnt == 9'd4)
                    ac_sda <= 1'b0;
                if (q == 2'd0 && bitn != 4'd8 && qcnt == 9'd249) begin
                    ac_sda <= sh[8];
                    sh <= {sh[7:0], 1'b0};
                end
            end else begin
                // q0: SCL low (SDA set above); q1, q2: SCL high (the slave
                // samples); q3: SCL low again.
                q <= q + 2'd1;
                case (q)
                    2'd0: begin end
                    2'd1: ac_scl <= 1'b1;
                    2'd2: ac_scl <= 1'b1;
                    default: begin
                        ac_scl <= 1'b0;
                        if (bitn == 4'd8) begin
                            if (ent[8]) begin
                                ist <= I_STOP;
                            end else begin
                                idx <= idx + 8'd1;
                                ist <= I_LOAD;
                            end
                        end else begin
                            bitn <= bitn + 4'd1;
                        end
                    end
                endcase
            end

            I_STOP: if (qtick) begin
                // q0: SDA low, q1: SCL high, q2: SDA rises (STOP), q3: idle.
                q <= q + 2'd1;
                case (q)
                    2'd0: ac_sda <= 1'b0;
                    2'd1: ac_scl <= 1'b1;
                    2'd2: ac_sda <= 1'b1;
                    default: begin
                        idx  <= idx + 8'd1;
                        wcnt <= ent[9] ? T_PLL[20:0] : 21'd0;
                        ist  <= I_GAP;
                    end
                endcase
            end

            I_DONE: init_done <= 1'b1;

            default: ist <= I_GAP;
        endcase
    end

    // ------------------------------------------------------------------
    // Audio clocks and I2S data.
    // ------------------------------------------------------------------
    localparam int unsigned BCLK_HALF = 17;  // system clocks per BCLK half
    localparam logic [23:0] AMP_POS   = 24'h100000; // +1/8 full scale
    localparam logic [23:0] AMP_NEG   = 24'hf00000; // -1/8 full scale

    logic [1:0]  mcnt;
    logic [4:0]  bcnt;
    logic [5:0]  slot;     // BCLK slot in the frame, 0..63
    logic [5:0]  nslot;
    logic [4:0]  tone_cnt; // frames in the current half period, 0..22
    logic        tone_neg;
    logic [13:0] frames;
    logic [31:0] word;

    assign nslot = slot + 6'd1;
    assign word  = {1'b0, tone_neg ? AMP_NEG : AMP_POS, 7'b0};

    always_ff @(posedge clk) begin
        // MCLK: toggles every 4 clocks.
        mcnt <= mcnt + 2'd1;
        if (mcnt == 2'd3)
            ac_mclk <= ~ac_mclk;

        if (bcnt == BCLK_HALF[4:0] - 5'd1) begin
            bcnt    <= 5'd0;
            ac_bclk <= ~ac_bclk;
            if (ac_bclk) begin
                // BCLK falls now: move to the next slot and put its bit
                // and LRCLK level on the bus.
                slot     <= nslot;
                ac_lrclk <= nslot[5];
                ac_sdata <= word[5'd31 - nslot[4:0]];
                if (nslot == 6'd0) begin
                    // New frame: the slot-0 bit is the I2S delay bit (0
                    // for every sample), so the next sample may change now.
                    frames <= frames + 14'd1;
                    if (frames == 14'h3fff)
                        beat <= ~beat;
                    if (tone_cnt == 5'd22) begin
                        tone_cnt <= 5'd0;
                        tone_neg <= ~tone_neg;
                    end else begin
                        tone_cnt <= tone_cnt + 5'd1;
                    end
                end
            end
        end else begin
            bcnt <= bcnt + 5'd1;
        end
    end
endmodule
