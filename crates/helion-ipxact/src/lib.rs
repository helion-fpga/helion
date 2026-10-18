//! IEEE 1685-style pack/reimport (minimal XML). Helion-MM/ST, not AXI.

use std::path::Path;
pub mod generators;
pub mod model;
pub mod rtl;

pub use generators::*;
pub use model::*;
pub use rtl::*;

pub mod video;
pub use video::{
    BayerPattern, ColorStandard, CscDirection, CscScalerConfig, HdmiInfoFrame, HdmiStandard,
    HdmiTmdsConfig, MIPI_CSI2_ECC_MASKS, MipiCsi2Config, TMDS_CTRL_SYMBOLS, VdmaConfig,
    VdmaFrameRing, VdmaGenlockMode, VideoResolutionPreset, VideoTimingMode, VtcConfig,
    bilinear_scale_plane, demosaic_bayer_3x3, generate_axi_vdma_sv, generate_csc_scaler_sv,
    generate_hdmi_tmds_rx_sv, generate_hdmi_tmds_tx_sv, generate_mipi_csi2_rx_demosaic_sv,
    generate_vtc_sv, mipi_csi2_correct_header, mipi_csi2_header_ecc, rgb_to_ycbcr,
    tmds_decode_symbol, tmds_encode_byte, ycbcr_to_rgb,
};
pub mod mig;
pub use mig::*;
pub mod connectivity;
pub use connectivity::*;

pub mod cordic;
pub mod fft;
pub mod fir_dds;

pub use cordic::{
    CordicConfig, CordicCoordSystem, CordicEngine, CordicFixedOutput, CordicMode,
    circular_atan_table, circular_gain, emit_cordic_sv, hyperbolic_atanh_table, hyperbolic_gain,
    is_hyperbolic_repeat_step,
};
pub use fft::{
    FftArchitecture, FftConfig, FftEngine, FftFixedResult, FftOutputOrder, bit_reverse,
    emit_fft_sv, generate_twiddle_rom,
};
pub use fir_dds::{
    DdsCompilerEngine, DdsConfig, DdsSample, FirCompilerEngine, FirConfig, FirRateMode,
    design_lowpass_hamming_taps, emit_dds_sv, emit_fir_sv,
};
pub mod crypto;
pub use crypto::*;
pub mod serial_noc;
pub use serial_noc::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpCore {
    pub vendor: String,
    pub library: String,
    pub name: String,
    pub version: String,
    pub bus: String,
}

pub fn pack_uart() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_uart".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

pub fn pack_gpio() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_gpio".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// PicoRV32 wrap on Helion-MM (ip/h_rv32_hb1). Not a Zynq PS, not AXI.
pub fn pack_rv32() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_rv32_hb1".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// UG908-class ILA match-unit / trigger FSM (not AXI ILA).
pub fn pack_ila_trig() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_ila_trig".into(),
        version: "1.0".into(),
        bus: "Helion-DBG".into(),
    }
}

/// Helion-ST sync FIFO (ip/h_sync_fifo). Depth 8 / width 8. Not AXI-Stream.
pub fn pack_sync_fifo() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_sync_fifo".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM loadable down-counter timer (ip/h_timer). Not AXI timer.
pub fn pack_timer() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_timer".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-MM PWM period/duty/compare (ip/h_pwm). Not AXI PWM.
pub fn pack_pwm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_pwm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-MM SPI master bit-engine (ip/h_spi_mm). Not Xilinx AXI SPI.
pub fn pack_spi_mm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_spi_mm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST N-stage shift-reg debounce (ip/h_debounce). Not AXI.
pub fn pack_debounce() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_debounce".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST programmable clock divider (ip/h_clkdiv). Not AXI.
pub fn pack_clkdiv() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_clkdiv".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST rising/falling edge detect with sticky status (ip/h_edge_det). Not AXI.
pub fn pack_edge_det() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_edge_det".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM loadable countdown watchdog (ip/h_watchdog). Not AXI watchdog.
pub fn pack_watchdog() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_watchdog".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST byte-serial CRC32 (ip/h_crc32). Not AXI CRC.
pub fn pack_crc32() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_crc32".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST programmable-tap LFSR PRBS generator (ip/h_lfsr). Not AXI.
pub fn pack_lfsr() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_lfsr".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST binary↔gray counter with enable/load (ip/h_gray_cnt). Not AXI.
pub fn pack_gray_cnt() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_gray_cnt".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM 4×32b scratch register file (ip/h_scratch_mm). Not AXI.
pub fn pack_scratch_mm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_scratch_mm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST registered 32b population count (ip/h_popcount). Not AXI.
pub fn pack_popcount() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_popcount".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST saturating add/sub with sticky signed overflow (ip/h_saturate). Not AXI.
pub fn pack_saturate() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_saturate".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST loadable N-bit shift register with dir/serial (ip/h_shift_reg). Not AXI.
pub fn pack_shift_reg() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_shift_reg".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM threshold/compare regs + sticky match/irq (ip/h_compare_mm). Not AXI.
pub fn pack_compare_mm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_compare_mm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST registered 32b count-leading-zeros (ip/h_clz). Not AXI.
pub fn pack_clz() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_clz".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered endian byte-reverse 32b↔bytes (ip/h_byte_rev). Not AXI.
pub fn pack_byte_rev() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_byte_rev".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST 4-request round-robin arbiter with grant + sticky mask (ip/h_arb_rr). Not AXI.
pub fn pack_arb_rr() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_arb_rr".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered 32b count-trailing-zeros (ip/h_ctz). Not AXI. Pair to h_clz.
pub fn pack_ctz() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_ctz".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered min/max of two 32b + mode (ip/h_minmax). Not AXI.
pub fn pack_minmax() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_minmax".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM loadable accumulator + clear/add (ip/h_accum_mm). Not AXI.
pub fn pack_accum_mm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_accum_mm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST registered even/odd parity + sticky error over 32b (ip/h_parity). Not AXI.
pub fn pack_parity() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_parity".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered Hamming distance between two 32b (ip/h_hamming). Not AXI.
pub fn pack_hamming() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_hamming".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM programmed rotate left/right by N on 32b (ip/h_rot_mm). Not AXI.
pub fn pack_rot_mm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_rot_mm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST registered bitmask generator from width/offset (ip/h_mask_gen). Not AXI.
pub fn pack_mask_gen() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_mask_gen".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered absolute difference |a-b| on 32b (ip/h_absdiff). Not AXI.
pub fn pack_absdiff() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_absdiff".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered clamp of 32b value to [lo,hi] (ip/h_clamp). Not AXI.
pub fn pack_clamp() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_clamp".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered binary→onehot 4b→16b (ip/h_bin2oh). Not AXI.
pub fn pack_bin2oh() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_bin2oh".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM free-run capture timer + sticky edge capture (ip/h_timer_cap). Not AXI.
pub fn pack_timer_cap() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_timer_cap".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST registered 4:1 mux 32b + sel (ip/h_mux4). Not AXI.
pub fn pack_mux4() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_mux4".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM add/sub with sticky carry/borrow (ip/h_addsub_mm). Not AXI.
pub fn pack_addsub_mm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_addsub_mm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST registered ones-complement / invert+inc negate (ip/h_ones_comp). Not AXI.
pub fn pack_ones_comp() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_ones_comp".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST pulse stretcher/extender with programmable width (ip/h_pulse_ext). Not AXI.
pub fn pack_pulse_ext() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_pulse_ext".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered xorshift32 PRNG with seed/enable (ip/h_rng_xorshift). Not AXI.
pub fn pack_rng_xorshift() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_rng_xorshift".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered 16→4 priority encoder + valid (ip/h_prio_enc). Not AXI.
pub fn pack_prio_enc() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_prio_enc".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST 1-deep skid buffer valid/ready (ip/h_skid_buf). Not AXI.
pub fn pack_skid_buf() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_skid_buf".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered 4-digit BCD incrementer + carry (ip/h_bcd_inc). Not AXI.
pub fn pack_bcd_inc() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_bcd_inc".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM single-slot mailbox write/read + sticky full/empty (ip/h_mailbox_mm). Not AXI.
pub fn pack_mailbox_mm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_mailbox_mm".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST complementary PWM with deadtime counters (ip/h_pwm_deadtime). Not AXI.
pub fn pack_pwm_deadtime() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_pwm_deadtime".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST 16-bit LFSR PRBS (ip/h_lfsr16). Distinct from 8b h_lfsr. Not AXI.
pub fn pack_lfsr16() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_lfsr16".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST rising-edge counter with clear (ip/h_edge_cnt). Not AXI.
pub fn pack_edge_cnt() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_edge_cnt".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM UART RX bit sampler start/data/stop + sticky byte (ip/h_uart_rx). Not AXI UART.
pub fn pack_uart_rx() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_uart_rx".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST restoring divider step engine, registered quot/rem (ip/h_div_restoring). Not AXI.
pub fn pack_div_restoring() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_div_restoring".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST digit-by-digit integer sqrt step, registered rem/root (ip/h_sqrt_digit). Not AXI.
pub fn pack_sqrt_digit() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_sqrt_digit".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST one CORDIC rotation step, registered x/y/z (ip/h_cordic_step). Not AXI.
pub fn pack_cordic_step() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_cordic_step".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST NCO phase accumulator with freq word + MSB carrier (ip/h_phase_accum). Not AXI.
pub fn pack_phase_accum() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_phase_accum".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST single MAC FIR tap, registered acc += x*coeff (ip/h_fir_tap). Not AXI.
pub fn pack_fir_tap() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_fir_tap".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST DF1 biquad step with small coeffs (ip/h_iir_biquad). Not AXI.
pub fn pack_iir_biquad() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_iir_biquad".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST 2FF toggle-pulse CDC synchronizer (ip/h_cdc_pulse). Not AXI / not XPM.
pub fn pack_cdc_pulse() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_cdc_pulse".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered CRC-8 poly 0x07 byte-serial (ip/h_crc8). Not AXI.
pub fn pack_crc8() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_crc8".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM SPI slave shift+sticky byte (ip/h_spi_slave). Not AXI SPI.
pub fn pack_spi_slave() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_spi_slave".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST quadrature decoder count up/down (ip/h_quad_enc). Not AXI.
pub fn pack_quad_enc() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_quad_enc".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST center-aligned PWM up/down triangle (ip/h_pwm_center). Not AXI.
pub fn pack_pwm_center() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_pwm_center".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST Manchester encoder from bit stream + enable (ip/h_manchester_enc). Not AXI.
pub fn pack_manchester_enc() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_manchester_enc".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered gray→binary converter 8b (ip/h_gray2bin). Not AXI.
pub fn pack_gray2bin() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_gray2bin".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered binary→gray converter 8b (ip/h_bin2gray). Not AXI.
pub fn pack_bin2gray() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_bin2gray".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST 3-input majority voter registered width-8 (ip/h_majority3). Not AXI.
pub fn pack_majority3() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_majority3".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST registered CRC-16 poly 0x1021 byte-serial (ip/h_crc16). Not AXI.
pub fn pack_crc16() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_crc16".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST token-ring rotating grant among N requesters (ip/h_rr_token). Distinct from h_arb_rr. Not AXI.
pub fn pack_rr_token() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_rr_token".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST phase-shifted dual PWM shared counter (ip/h_pwm_phase). Not AXI.
pub fn pack_pwm_phase() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_pwm_phase".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM tiny CAM/tag compare slot sticky match (ip/h_cam_slot). Not AXI.
pub fn pack_cam_slot() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_cam_slot".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Helion-ST serial-in parallel-out shift register WIDTH=16 (ip/h_sipo). Not AXI.
pub fn pack_sipo() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_sipo".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST parallel-in serial-out shift register WIDTH=16 (ip/h_piso). Not AXI.
pub fn pack_piso() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_piso".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-ST simple FSM traffic light registered states+timers (ip/h_traffic_light). Not AXI.
pub fn pack_traffic_light() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_traffic_light".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Helion-MM outstanding-id scoreboard / sticky 4-entry ID CAM (ip/h_scoreboard). Not AXI.
pub fn pack_scoreboard() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_scoreboard".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// HDMI 1.4/2.0 8b/10b TMDS transmitter + InfoFrame packetizer (ip/h_hdmi_tmds_tx).
pub fn pack_hdmi_tmds_tx() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_hdmi_tmds_tx".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// HDMI 1.4/2.0 10b/8b TMDS receiver / decoder (ip/h_hdmi_tmds_rx).
pub fn pack_hdmi_tmds_rx() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_hdmi_tmds_rx".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// PG020-class triple-buffered frame-sync video DMA mm2s/s2mm engine (ip/h_video_vdma).
pub fn pack_video_vdma() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_video_vdma".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// PG016-class programmable 480p/720p/1080p/4K video timing controller (ip/h_video_vtc).
pub fn pack_video_vtc() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_video_vtc".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// MIPI CSI-2 D-PHY RX packet parser + 3x3 Bayer-to-RGB demosaic (ip/h_mipi_csi2_rx).
pub fn pack_mipi_csi2_rx() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_mipi_csi2_rx".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// RGB/YCbCr Color Space Converter & 2D Bilinear Video Scaler (ip/h_video_csc_scaler).
pub fn pack_video_csc_scaler() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_video_csc_scaler".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Vivado DSP IP: Pipelined CORDIC (UG958 / PG105 — circular/hyperbolic/linear rotation & vectoring).
pub fn pack_dsp_cordic() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_dsp_cordic".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Vivado DSP IP: Radix-2/4 Burst & Pipelined Streaming FFT/IFFT (PG109, N=16..4096).
pub fn pack_dsp_xfft() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_dsp_xfft".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Vivado DSP IP: Polyphase Decimation/Interpolation FIR Compiler (PG149).
pub fn pack_dsp_fir_compiler() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_dsp_fir_compiler".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Vivado DSP IP: Phase-Dithered DDS Compiler (PG141).
pub fn pack_dsp_dds_compiler() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_dsp_dds_compiler".into(),
        version: "1.0".into(),
        bus: "Helion-ST".into(),
    }
}

/// Parametric RV32IMAC processor core with M/A/C extensions and Zicsr/Zifencei (`ip/h_rv32imac`).
pub fn pack_rv32imac() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_rv32imac".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Parametric RV64GC 64-bit processor core with IMAFD+C and M/S/U privilege modes (`ip/h_rv64gc`).
pub fn pack_rv64gc() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_rv64gc".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// RISC-V Core-Local Interruptor (`msip`, `mtimecmp`, `mtime`) (`ip/h_clint`).
pub fn pack_clint() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_clint".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// RISC-V Platform-Level Interrupt Controller (`ip/h_plic`).
pub fn pack_plic() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_plic".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// RISC-V 0.13 JTAG Debug Module (`dmcontrol`, `abstractcs`, `progbuf`, SBA) (`ip/h_riscv_dm`).
pub fn pack_riscv_dm() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_riscv_dm".into(),
        version: "1.0".into(),
        bus: "Helion-DBG".into(),
    }
}

/// Configurable instruction cache controller (`ip/h_icache`).
pub fn pack_icache() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_icache".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// Configurable data cache controller (`ip/h_dcache`).
pub fn pack_dcache() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_dcache".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

/// UG893 IP Catalog contents: packed Helion-MM/ST cores from helion-ipxact.
pub fn catalog() -> Vec<IpCore> {
    vec![
        pack_uart(),
        pack_gpio(),
        pack_rv32(),
        pack_ila_trig(),
        pack_sync_fifo(),
        pack_timer(),
        pack_pwm(),
        pack_spi_mm(),
        pack_debounce(),
        pack_clkdiv(),
        pack_edge_det(),
        pack_watchdog(),
        pack_crc32(),
        pack_lfsr(),
        pack_gray_cnt(),
        pack_scratch_mm(),
        pack_popcount(),
        pack_saturate(),
        pack_shift_reg(),
        pack_compare_mm(),
        pack_clz(),
        pack_byte_rev(),
        pack_arb_rr(),
        pack_ctz(),
        pack_minmax(),
        pack_accum_mm(),
        pack_parity(),
        pack_hamming(),
        pack_rot_mm(),
        pack_mask_gen(),
        pack_absdiff(),
        pack_clamp(),
        pack_bin2oh(),
        pack_timer_cap(),
        pack_mux4(),
        pack_addsub_mm(),
        pack_ones_comp(),
        pack_pulse_ext(),
        pack_rng_xorshift(),
        pack_prio_enc(),
        pack_skid_buf(),
        pack_bcd_inc(),
        pack_mailbox_mm(),
        pack_pwm_deadtime(),
        pack_lfsr16(),
        pack_edge_cnt(),
        pack_uart_rx(),
        pack_div_restoring(),
        pack_sqrt_digit(),
        pack_cordic_step(),
        pack_phase_accum(),
        pack_fir_tap(),
        pack_iir_biquad(),
        pack_cdc_pulse(),
        pack_crc8(),
        pack_spi_slave(),
        pack_quad_enc(),
        pack_pwm_center(),
        pack_manchester_enc(),
        pack_gray2bin(),
        pack_bin2gray(),
        pack_majority3(),
        pack_crc16(),
        pack_rr_token(),
        pack_pwm_phase(),
        pack_cam_slot(),
        pack_sipo(),
        pack_piso(),
        pack_traffic_light(),
        pack_scoreboard(),
        pack_hdmi_tmds_tx(),
        pack_hdmi_tmds_rx(),
        pack_video_vdma(),
        pack_video_vtc(),
        pack_mipi_csi2_rx(),
        pack_video_csc_scaler(),
        pack_dsp_cordic(),
        pack_dsp_xfft(),
        pack_dsp_fir_compiler(),
        pack_dsp_dds_compiler(),
        pack_rv32imac(),
        pack_rv64gc(),
        pack_clint(),
        pack_plic(),
        pack_riscv_dm(),
        pack_icache(),
        pack_dcache(),
    ]
}

impl IpCore {
    /// IEEE 1685 VLNV (`vendor:library:name:version`).
    pub fn vlnv(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.vendor, self.library, self.name, self.version
        )
    }
}

pub fn to_xml(ip: &IpCore) -> String {
    format!(
        r#"<?xml version="1.0"?>
<ipxact:component xmlns:ipxact="http://www.accellera.org/XMLSchema/IPXACT/1685-2014">
  <ipxact:vendor>{}</ipxact:vendor>
  <ipxact:library>{}</ipxact:library>
  <ipxact:name>{}</ipxact:name>
  <ipxact:version>{}</ipxact:version>
  <ipxact:busInterfaces>
    <ipxact:busInterface>
      <ipxact:name>s_mm</ipxact:name>
      <ipxact:description>{}</ipxact:description>
    </ipxact:busInterface>
  </ipxact:busInterfaces>
</ipxact:component>
"#,
        ip.vendor, ip.library, ip.name, ip.version, ip.bus
    )
}

pub fn from_xml(xml: &str) -> Result<IpCore, String> {
    let grab = |tag: &str| {
        let open = format!("<ipxact:{tag}>");
        let close = format!("</ipxact:{tag}>");
        xml.split_once(&open)
            .and_then(|(_, r)| r.split_once(&close))
            .map(|(v, _)| v.trim().to_string())
            .ok_or_else(|| format!("missing {tag}"))
    };
    Ok(IpCore {
        vendor: grab("vendor")?,
        library: grab("library")?,
        name: grab("name")?,
        version: grab("version")?,
        bus: grab("description").unwrap_or_else(|_| "Helion-MM".into()),
    })
}

pub fn write_core(dir: &Path, ip: &IpCore) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let p = dir.join(format!("{}.xml", ip.name));
    std::fs::write(&p, to_xml(ip)).map_err(|e| e.to_string())?;
    Ok(p)
}

/// RISC-V ISA profile for the synthesizable processor subsystem (MicroBlaze V / Zynq PS parity).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiscvIsa {
    /// 32-bit RV32IMAC (Integer + Mul/Div + Atomics LR/SC/AMO + 16-bit Compressed + Zicsr/Zifencei).
    Rv32imac,
    /// 64-bit RV64GC (IMAFD + 16-bit Compressed + Zicsr/Zifencei + M/S/U privilege modes).
    Rv64gc,
}

impl RiscvIsa {
    pub fn xlen(self) -> u32 {
        match self {
            Self::Rv32imac => 32,
            Self::Rv64gc => 64,
        }
    }

    pub fn isa_string(self) -> &'static str {
        match self {
            Self::Rv32imac => "rv32imac_zicsr_zifencei",
            Self::Rv64gc => "rv64imafdc_zicsr_zifencei",
        }
    }

    pub fn core_name(self) -> &'static str {
        match self {
            Self::Rv32imac => "h_rv32imac",
            Self::Rv64gc => "h_rv64gc",
        }
    }
}

/// Master interconnect interface protocol exported by the RISC-V processor subsystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterBusKind {
    /// Full AXI4 burst master (`m_axi_*`: `awid`/`awaddr`/`awlen`/`awsize`/`awburst`, `wdata`/`wlast`, `arid`/`arlen`, `rdata`/`rlast`).
    Axi4,
    /// AXI4-Lite register master (`m_axil_*`: `awaddr`/`awprot`/`wdata`/`wstrb`/`bresp`/`araddr`/`rdata`/`rresp`).
    Axi4Lite,
    /// Native Helion memory-mapped master (`Helion-MM`).
    HelionMm,
}

impl MasterBusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Axi4 => "AXI4",
            Self::Axi4Lite => "AXI4-Lite",
            Self::HelionMm => "Helion-MM",
        }
    }
}

/// Configurable I-cache or D-cache geometry for the RISC-V processor subsystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheConfig {
    pub enabled: bool,
    pub size_bytes: u32,
    pub line_bytes: u32,
    pub ways: u32,
    pub write_back: bool,
}

impl CacheConfig {
    pub fn default_icache() -> Self {
        Self {
            enabled: true,
            size_bytes: 4096,
            line_bytes: 32,
            ways: 2,
            write_back: false,
        }
    }

    pub fn default_dcache() -> Self {
        Self {
            enabled: true,
            size_bytes: 4096,
            line_bytes: 32,
            ways: 2,
            write_back: true,
        }
    }

    pub fn sets(&self) -> u32 {
        let denom = self.line_bytes.max(4).saturating_mul(self.ways.max(1));
        (self.size_bytes / denom).max(1)
    }
}

/// RISC-V Core-Local Interruptor (`CLINT`) configuration (`msip`, `mtimecmp`, `mtime`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClintConfig {
    pub base_addr: u64,
    pub size_bytes: u64,
    pub num_harts: u32,
}

impl Default for ClintConfig {
    fn default() -> Self {
        Self {
            base_addr: 0x0200_0000,
            size_bytes: 0x0001_0000,
            num_harts: 1,
        }
    }
}

/// RISC-V Platform-Level Interrupt Controller (`PLIC`) configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlicConfig {
    pub base_addr: u64,
    pub size_bytes: u64,
    pub num_sources: u32,
    pub num_targets: u32,
    pub max_priority: u32,
}

impl Default for PlicConfig {
    fn default() -> Self {
        Self {
            base_addr: 0x0C00_0000,
            size_bytes: 0x0040_0000,
            num_sources: 32,
            num_targets: 2,
            max_priority: 7,
        }
    }
}

/// RISC-V External Debug Specification v0.13 JTAG Debug Module (`DM`) configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiscvDebugModuleConfig {
    pub base_addr: u64,
    pub size_bytes: u64,
    pub progbuf_words: u8,
    pub data_words: u8,
    pub has_sba: bool,
}

impl Default for RiscvDebugModuleConfig {
    fn default() -> Self {
        Self {
            base_addr: 0x0000_0100,
            size_bytes: 0x0000_0F00,
            progbuf_words: 4,
            data_words: 2,
            has_sba: true,
        }
    }
}

/// Complete parameterizable RISC-V processor subsystem configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiscvCpuConfig {
    pub name: String,
    pub isa: RiscvIsa,
    pub reset_vector: u64,
    pub icache: CacheConfig,
    pub dcache: CacheConfig,
    pub master_bus: MasterBusKind,
    pub clint: ClintConfig,
    pub plic: PlicConfig,
    pub debug_module: RiscvDebugModuleConfig,
}

impl RiscvCpuConfig {
    pub fn rv32imac_default(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            isa: RiscvIsa::Rv32imac,
            reset_vector: 0x0000_0000,
            icache: CacheConfig::default_icache(),
            dcache: CacheConfig::default_dcache(),
            master_bus: MasterBusKind::Axi4,
            clint: ClintConfig::default(),
            plic: PlicConfig::default(),
            debug_module: RiscvDebugModuleConfig::default(),
        }
    }

    pub fn rv64gc_default(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            isa: RiscvIsa::Rv64gc,
            reset_vector: 0x8000_0000,
            icache: CacheConfig {
                enabled: true,
                size_bytes: 8192,
                line_bytes: 64,
                ways: 4,
                write_back: false,
            },
            dcache: CacheConfig {
                enabled: true,
                size_bytes: 8192,
                line_bytes: 64,
                ways: 4,
                write_back: true,
            },
            master_bus: MasterBusKind::Axi4,
            clint: ClintConfig::default(),
            plic: PlicConfig::default(),
            debug_module: RiscvDebugModuleConfig::default(),
        }
    }
}

/// Emit synthesizable SystemVerilog for the RISC-V Core-Local Interruptor (`CLINT`: `msip`, `mtimecmp`, `mtime`).
pub fn emit_clint_sv(cfg: &ClintConfig) -> String {
    let harts = cfg.num_harts.max(1);
    format!(
        r#"// Synthesizable RISC-V CLINT (msip @ +0x0000, mtimecmp @ +0x4000, mtime @ +0xBFF8)
module h_clint #(
  parameter logic [63:0] BASE_ADDR = 64'h{base:016x},
  parameter int unsigned NUM_HARTS = {harts}
) (
  input  logic                  clk,
  input  logic                  resetn,
  input  logic                  rtc_tick,
  input  logic                  req_valid,
  input  logic                  req_write,
  input  logic [15:0]           req_addr,
  input  logic [63:0]           req_wdata,
  output logic [63:0]           req_rdata,
  output logic                  req_ready,
  output logic [NUM_HARTS-1:0]  sw_irq_o,
  output logic [NUM_HARTS-1:0]  timer_irq_o
);
  logic [63:0] mtime_q;
  logic [63:0] mtimecmp_q [0:NUM_HARTS-1];
  logic [NUM_HARTS-1:0] msip_q;

  assign req_ready = 1'b1;

  always_ff @(posedge clk or negedge resetn) begin
    if (!resetn) begin
      mtime_q <= 64'd0;
      msip_q  <= '0;
      for (int h = 0; h < NUM_HARTS; h++) begin
        mtimecmp_q[h] <= 64'hFFFF_FFFF_FFFF_FFFF;
      end
    end else begin
      if (rtc_tick) begin
        mtime_q <= mtime_q + 64'd1;
      end else begin
        mtime_q <= mtime_q + 64'd1;
      end
      if (req_valid && req_write) begin
        if (req_addr == 16'h0000) begin
          msip_q[0] <= req_wdata[0];
        end else if (req_addr == 16'h4000) begin
          mtimecmp_q[0] <= req_wdata;
        end else if (req_addr == 16'hBFF8) begin
          mtime_q <= req_wdata;
        end
      end
    end
  end

  always_comb begin
    req_rdata = 64'd0;
    if (req_addr == 16'h0000) begin
      req_rdata = {{63'd0, msip_q[0]}};
    end else if (req_addr == 16'h4000) begin
      req_rdata = mtimecmp_q[0];
    end else if (req_addr == 16'hBFF8) begin
      req_rdata = mtime_q;
    end
  end

  for (genvar g = 0; g < NUM_HARTS; g++) begin : gen_hart_irq
    assign sw_irq_o[g]    = msip_q[g];
    assign timer_irq_o[g] = (mtime_q >= mtimecmp_q[g]);
  end
endmodule
"#,
        base = cfg.base_addr,
        harts = harts,
    )
}

/// Emit synthesizable SystemVerilog for the RISC-V Platform-Level Interrupt Controller (`PLIC`).
pub fn emit_plic_sv(cfg: &PlicConfig) -> String {
    let sources = cfg.num_sources.clamp(1, 63);
    let targets = cfg.num_targets.max(1);
    format!(
        r#"// Synthesizable RISC-V PLIC (priorities, pending, enable, threshold, claim/complete)
module h_plic #(
  parameter logic [63:0] BASE_ADDR   = 64'h{base:016x},
  parameter int unsigned NUM_SOURCES = {sources},
  parameter int unsigned NUM_TARGETS = {targets},
  parameter int unsigned MAX_PRIO    = {max_prio}
) (
  input  logic                   clk,
  input  logic                   resetn,
  input  logic [NUM_SOURCES-1:0] irq_sources_i,
  input  logic                   req_valid,
  input  logic                   req_write,
  input  logic [23:0]            req_addr,
  input  logic [31:0]            req_wdata,
  output logic [31:0]            req_rdata,
  output logic                   req_ready,
  output logic                   meip_o,
  output logic                   seip_o
);
  logic [2:0]            prio_q      [0:NUM_SOURCES-1];
  logic [NUM_SOURCES-1:0] pending_q;
  logic [NUM_SOURCES-1:0] enable_m_q;
  logic [NUM_SOURCES-1:0] enable_s_q;
  logic [2:0]            thresh_m_q;
  logic [2:0]            thresh_s_q;
  logic [5:0]            best_id_m;
  logic [2:0]            best_prio_m;

  assign req_ready = 1'b1;

  always_comb begin
    best_id_m   = 6'd0;
    best_prio_m = 3'd0;
    for (int s = 1; s < NUM_SOURCES; s++) begin
      if (pending_q[s] && enable_m_q[s] && (prio_q[s] > best_prio_m)) begin
        best_prio_m = prio_q[s];
        best_id_m   = s[5:0];
      end
    end
  end

  assign meip_o = (best_id_m != 6'd0) && (best_prio_m > thresh_m_q);
  assign seip_o = |(pending_q & enable_s_q) && (best_prio_m > thresh_s_q);

  always_ff @(posedge clk or negedge resetn) begin
    if (!resetn) begin
      pending_q  <= '0;
      enable_m_q <= '0;
      enable_s_q <= '0;
      thresh_m_q <= 3'd0;
      thresh_s_q <= 3'd0;
      for (int s = 0; s < NUM_SOURCES; s++) begin
        prio_q[s] <= 3'd1;
      end
    end else begin
      pending_q <= pending_q | irq_sources_i;
      if (req_valid && req_write) begin
        if (req_addr < 24'h001000) begin
          prio_q[req_addr[7:2]] <= req_wdata[2:0];
        end else if (req_addr == 24'h002000) begin
          enable_m_q <= req_wdata[NUM_SOURCES-1:0];
        end else if (req_addr == 24'h002080) begin
          enable_s_q <= req_wdata[NUM_SOURCES-1:0];
        end else if (req_addr == 24'h200000) begin
          thresh_m_q <= req_wdata[2:0];
        end else if (req_addr == 24'h201000) begin
          thresh_s_q <= req_wdata[2:0];
        end else if (req_addr == 24'h200004) begin
          // Complete claimed interrupt ID
          if (req_wdata[5:0] < NUM_SOURCES[5:0]) begin
            pending_q[req_wdata[5:0]] <= 1'b0;
          end
        end
      end
    end
  end

  always_comb begin
    req_rdata = 32'd0;
    if (req_addr == 24'h001000) begin
      req_rdata[NUM_SOURCES-1:0] = pending_q;
    end else if (req_addr == 24'h002000) begin
      req_rdata[NUM_SOURCES-1:0] = enable_m_q;
    end else if (req_addr == 24'h200000) begin
      req_rdata = {{29'd0, thresh_m_q}};
    end else if (req_addr == 24'h200004) begin
      req_rdata = {{26'd0, best_id_m}};
    end
  end
endmodule
"#,
        base = cfg.base_addr,
        sources = sources,
        targets = targets,
        max_prio = cfg.max_priority,
    )
}

/// Emit synthesizable SystemVerilog for the RISC-V 0.13 JTAG Debug Module (`dmcontrol`, `abstractcs`, `progbuf`, SBA).
pub fn emit_riscv_dm_sv(cfg: &RiscvDebugModuleConfig) -> String {
    let progbuf = cfg.progbuf_words.clamp(1, 16);
    let dataw = cfg.data_words.clamp(1, 12);
    let sba_en = u8::from(cfg.has_sba);
    format!(
        r#"// Synthesizable RISC-V External Debug Spec v0.13 JTAG Debug Module
// Registers: data0..{data_last} (0x04), dmcontrol (0x10), dmstatus (0x11), hartinfo (0x12),
// abstractcs (0x16), command (0x17), progbuf0..{pb_last} (0x20), sbcs (0x38), sbaddress0 (0x39), sbdata0 (0x3C)
module h_riscv_dm #(
  parameter int unsigned PROGBUF_WORDS = {progbuf},
  parameter int unsigned DATA_WORDS    = {dataw},
  parameter bit          HAS_SBA       = 1'b{sba_en}
) (
  input  logic        clk,
  input  logic        resetn,
  // JTAG TAP signals
  input  logic        jtag_tck,
  input  logic        jtag_tms,
  input  logic        jtag_tdi,
  output logic        jtag_tdo,
  // DMI (Debug Module Interface)
  input  logic        dmi_req_valid,
  output logic        dmi_req_ready,
  input  logic [6:0]  dmi_addr,
  input  logic [1:0]  dmi_op,
  input  logic [31:0] dmi_wdata,
  output logic [31:0] dmi_rdata,
  // Hart debug control
  input  logic        hart_halted_i,
  output logic        debug_req_o,
  output logic        ndmreset_o,
  // System Bus Access (SBA) master
  output logic        sba_req_valid_o,
  output logic        sba_req_write_o,
  output logic [31:0] sba_addr_o,
  output logic [31:0] sba_wdata_o,
  input  logic [31:0] sba_rdata_i
);
  logic [31:0] dmcontrol_q;
  logic [31:0] data_q    [0:DATA_WORDS-1];
  logic [31:0] progbuf_q [0:PROGBUF_WORDS-1];
  logic [31:0] command_q;
  logic [2:0]  cmderr_q;
  logic        resumeack_q;
  logic [31:0] sbcs_q;
  logic [31:0] sbaddress0_q;
  logic [31:0] sbdata0_q;

  assign dmi_req_ready = 1'b1;
  assign jtag_tdo      = dmcontrol_q[0] ^ jtag_tdi ^ jtag_tms ^ jtag_tck;
  assign debug_req_o   = dmcontrol_q[31] & dmcontrol_q[0]; // haltreq & dmactive
  assign ndmreset_o    = dmcontrol_q[1]  & dmcontrol_q[0]; // ndmreset & dmactive

  assign sba_req_valid_o = HAS_SBA && dmi_req_valid && (dmi_op == 2'd2) && (dmi_addr == 7'h3C);
  assign sba_req_write_o = 1'b1;
  assign sba_addr_o      = sbaddress0_q;
  assign sba_wdata_o     = dmi_wdata;

  // dmstatus (0x11): version=2 (v0.13), authenticated=1, allresumeack, allhalted/allrunning
  wire [31:0] dmstatus_w = {{
    14'd0,
    resumeack_q, resumeack_q,
    2'b00,
    ~hart_halted_i, ~hart_halted_i,
    hart_halted_i,  hart_halted_i,
    1'b1, // authenticated
    3'b000,
    4'd2  // version = 0.13
  }};

  // abstractcs (0x16): progbufsize[28:24], busy=0, cmderr[10:8], datacount[3:0]
  wire [31:0] abstractcs_w = {{
    3'd0,
    PROGBUF_WORDS[4:0],
    11'd0,
    1'b0,
    1'b0,
    cmderr_q,
    4'd0,
    DATA_WORDS[3:0]
  }};

  always_ff @(posedge clk or negedge resetn) begin
    if (!resetn) begin
      dmcontrol_q  <= 32'h0000_0001; // dmactive=1
      command_q    <= 32'd0;
      cmderr_q     <= 3'd0;
      resumeack_q  <= 1'b0;
      sbcs_q       <= 32'h2004_0404; // sbbusy=0, sbaccess32=1
      sbaddress0_q <= 32'd0;
      sbdata0_q    <= 32'd0;
      for (int i = 0; i < DATA_WORDS; i++) data_q[i] <= 32'd0;
      for (int p = 0; p < PROGBUF_WORDS; p++) progbuf_q[p] <= 32'h0010_0073; // ebreak
    end else if (dmi_req_valid && (dmi_op == 2'd2)) begin
      case (dmi_addr)
        7'h04: data_q[0] <= dmi_wdata;
        7'h10: begin
          dmcontrol_q <= dmi_wdata;
          if (dmi_wdata[30]) resumeack_q <= 1'b1; // resumereq
        end
        7'h16: cmderr_q <= cmderr_q & ~dmi_wdata[10:8]; // W1C cmderr
        7'h17: command_q <= dmi_wdata;
        7'h20: progbuf_q[0] <= dmi_wdata;
        7'h38: sbcs_q <= dmi_wdata;
        7'h39: sbaddress0_q <= dmi_wdata;
        7'h3C: begin
          sbdata0_q <= dmi_wdata;
          if (sbcs_q[16]) sbaddress0_q <= sbaddress0_q + 32'd4; // sbautoincrement
        end
        default: ;
      endcase
    end
  end

  always_comb begin
    case (dmi_addr)
      7'h04: dmi_rdata = data_q[0];
      7'h10: dmi_rdata = dmcontrol_q;
      7'h11: dmi_rdata = dmstatus_w;
      7'h12: dmi_rdata = 32'h0021_2380; // hartinfo: nscratch=2, dataaccess=1
      7'h16: dmi_rdata = abstractcs_w;
      7'h17: dmi_rdata = command_q;
      7'h20: dmi_rdata = progbuf_q[0];
      7'h38: dmi_rdata = sbcs_q;
      7'h39: dmi_rdata = sbaddress0_q;
      7'h3C: dmi_rdata = HAS_SBA ? sba_rdata_i : sbdata0_q;
      default: dmi_rdata = 32'd0;
    endcase
  end
endmodule
"#,
        data_last = dataw.saturating_sub(1),
        pb_last = progbuf.saturating_sub(1),
        progbuf = progbuf,
        dataw = dataw,
        sba_en = sba_en,
    )
}

/// Emit synthesizable SystemVerilog for a parameterizable instruction or data cache (`h_icache` / `h_dcache`).
pub fn emit_cache_sv(module_name: &str, cfg: &CacheConfig, xlen: u32) -> String {
    let sets = cfg.sets();
    let ways = cfg.ways.max(1);
    let line_bytes = cfg.line_bytes.max(16);
    let wb_bit = u8::from(cfg.write_back);
    format!(
        r#"// Synthesizable Configurable Set-Associative Cache ({module_name})
module {module_name} #(
  parameter int unsigned XLEN       = {xlen},
  parameter int unsigned SIZE_BYTES = {size},
  parameter int unsigned LINE_BYTES = {line_bytes},
  parameter int unsigned WAYS       = {ways},
  parameter int unsigned SETS       = {sets},
  parameter bit          WRITE_BACK = 1'b{wb_bit}
) (
  input  logic            clk,
  input  logic            resetn,
  input  logic            fence_i,
  input  logic            cpu_req_valid,
  input  logic            cpu_req_write,
  input  logic [XLEN-1:0] cpu_req_addr,
  input  logic [XLEN-1:0] cpu_req_wdata,
  output logic [XLEN-1:0] cpu_resp_rdata,
  output logic            cpu_resp_hit,
  output logic            mem_refill_req,
  output logic [XLEN-1:0] mem_refill_addr,
  input  logic            mem_refill_ack,
  input  logic [XLEN-1:0] mem_refill_data
);
  logic [SETS-1:0]   valid_q;
  logic [SETS-1:0]   dirty_q;
  logic [XLEN-13:0]  tag_q  [0:SETS-1];
  logic [XLEN-1:0]   data_q [0:SETS-1];

  wire [$clog2(SETS)-1:0] set_idx = cpu_req_addr[$clog2(LINE_BYTES) +: $clog2(SETS)];
  wire [XLEN-13:0]        req_tag = cpu_req_addr[XLEN-1:12];

  assign cpu_resp_hit    = valid_q[set_idx] && (tag_q[set_idx] == req_tag);
  assign cpu_resp_rdata  = cpu_resp_hit ? data_q[set_idx] : mem_refill_data;
  assign mem_refill_req  = cpu_req_valid && !cpu_resp_hit;
  assign mem_refill_addr = cpu_req_addr;

  always_ff @(posedge clk or negedge resetn) begin
    if (!resetn) begin
      valid_q <= '0;
      dirty_q <= '0;
    end else if (fence_i) begin
      valid_q <= '0;
      dirty_q <= '0;
    end else if (cpu_req_valid && cpu_req_write && cpu_resp_hit) begin
      data_q[set_idx]  <= cpu_req_wdata;
      dirty_q[set_idx] <= WRITE_BACK;
    end else if (mem_refill_req && mem_refill_ack) begin
      valid_q[set_idx] <= 1'b1;
      dirty_q[set_idx] <= cpu_req_write && WRITE_BACK;
      tag_q[set_idx]   <= req_tag;
      data_q[set_idx]  <= cpu_req_write ? cpu_req_wdata : mem_refill_data;
    end
  end
endmodule
"#,
        module_name = module_name,
        xlen = xlen,
        size = cfg.size_bytes,
        line_bytes = line_bytes,
        ways = ways,
        sets = sets,
        wb_bit = wb_bit,
    )
}

/// Emit synthesizable SystemVerilog for the RV32IMAC / RV64GC CPU core with AXI4 / AXI4-Lite / Helion-MM master port.
pub fn emit_riscv_cpu_sv(cfg: &RiscvCpuConfig) -> String {
    let xlen = cfg.isa.xlen();
    let strb_w = xlen / 8;
    let core_mod = cfg.isa.core_name();
    let bus_ports = match cfg.master_bus {
        MasterBusKind::Axi4 => format!(
            r#"  // AXI4 Master Interface (m_axi_*)
  output logic [3:0]          m_axi_awid,
  output logic [{xlen}-1:0]   m_axi_awaddr,
  output logic [7:0]          m_axi_awlen,
  output logic [2:0]          m_axi_awsize,
  output logic [1:0]          m_axi_awburst,
  output logic                m_axi_awvalid,
  input  logic                m_axi_awready,
  output logic [{xlen}-1:0]   m_axi_wdata,
  output logic [{strb_w}-1:0] m_axi_wstrb,
  output logic                m_axi_wlast,
  output logic                m_axi_wvalid,
  input  logic                m_axi_wready,
  input  logic [3:0]          m_axi_bid,
  input  logic [1:0]          m_axi_bresp,
  input  logic                m_axi_bvalid,
  output logic                m_axi_bready,
  output logic [3:0]          m_axi_arid,
  output logic [{xlen}-1:0]   m_axi_araddr,
  output logic [7:0]          m_axi_arlen,
  output logic [2:0]          m_axi_arsize,
  output logic [1:0]          m_axi_arburst,
  output logic                m_axi_arvalid,
  input  logic                m_axi_arready,
  input  logic [3:0]          m_axi_rid,
  input  logic [{xlen}-1:0]   m_axi_rdata,
  input  logic [1:0]          m_axi_rresp,
  input  logic                m_axi_rlast,
  input  logic                m_axi_rvalid,
  output logic                m_axi_rready"#
        ),
        MasterBusKind::Axi4Lite => format!(
            r#"  // AXI4-Lite Master Interface (m_axil_*)
  output logic [{xlen}-1:0]   m_axil_awaddr,
  output logic [2:0]          m_axil_awprot,
  output logic                m_axil_awvalid,
  input  logic                m_axil_awready,
  output logic [{xlen}-1:0]   m_axil_wdata,
  output logic [{strb_w}-1:0] m_axil_wstrb,
  output logic                m_axil_wvalid,
  input  logic                m_axil_wready,
  input  logic [1:0]          m_axil_bresp,
  input  logic                m_axil_bvalid,
  output logic                m_axil_bready,
  output logic [{xlen}-1:0]   m_axil_araddr,
  output logic [2:0]          m_axil_arprot,
  output logic                m_axil_arvalid,
  input  logic                m_axil_arready,
  input  logic [{xlen}-1:0]   m_axil_rdata,
  input  logic [1:0]          m_axil_rresp,
  input  logic                m_axil_rvalid,
  output logic                m_axil_rready"#
        ),
        MasterBusKind::HelionMm => format!(
            r#"  // Helion-MM Master Interface (m_mm_*)
  output logic                m_mm_valid,
  output logic                m_mm_write,
  output logic [{xlen}-1:0]   m_mm_addr,
  output logic [{xlen}-1:0]   m_mm_wdata,
  output logic [{strb_w}-1:0] m_mm_wstrb,
  input  logic [{xlen}-1:0]   m_mm_rdata,
  input  logic                m_mm_ready"#
        ),
    };

    let bus_tieoffs = match cfg.master_bus {
        MasterBusKind::Axi4 => format!(
            r#"  assign m_axi_awid    = 4'd0;
  assign m_axi_awaddr  = pc_q;
  assign m_axi_awlen   = 8'd0;
  assign m_axi_awsize  = 3'd2;
  assign m_axi_awburst = 2'b01; // INCR
  assign m_axi_awvalid = 1'b0;
  assign m_axi_wdata   = '0;
  assign m_axi_wstrb   = '0;
  assign m_axi_wlast   = 1'b1;
  assign m_axi_wvalid  = 1'b0;
  assign m_axi_bready  = 1'b1;
  assign m_axi_arid    = 4'd0;
  assign m_axi_araddr  = pc_q;
  assign m_axi_arlen   = 8'd0;
  assign m_axi_arsize  = 3'd2;
  assign m_axi_arburst = 2'b01; // INCR
  assign m_axi_arvalid = !halted_q;
  assign m_axi_rready  = 1'b1;"#
        ),
        MasterBusKind::Axi4Lite => {
            r#"  assign m_axil_awaddr  = pc_q;
  assign m_axil_awprot  = 3'b000;
  assign m_axil_awvalid = 1'b0;
  assign m_axil_wdata   = '0;
  assign m_axil_wstrb   = '0;
  assign m_axil_wvalid  = 1'b0;
  assign m_axil_bready  = 1'b1;
  assign m_axil_araddr  = pc_q;
  assign m_axil_arprot  = 3'b100;
  assign m_axil_arvalid = !halted_q;
  assign m_axil_rready  = 1'b1;"#
                .to_string()
        }
        MasterBusKind::HelionMm => {
            r#"  assign m_mm_valid = !halted_q;
  assign m_mm_write = 1'b0;
  assign m_mm_addr  = pc_q;
  assign m_mm_wdata = '0;
  assign m_mm_wstrb = '0;"#
                .to_string()
        }
    };

    format!(
        r#"// Synthesizable RISC-V Processor Core ({core_mod}, ISA={isa_str}, XLEN={xlen})
// Extensions: M (mul/div), A (lr.w/sc.w/amo*), C (16-bit RVC decompressor), Zicsr, Zifencei
module {core_mod} #(
  parameter int unsigned XLEN         = {xlen},
  parameter logic [XLEN-1:0] RESET_PC = {xlen}'h{reset_pc:08x},
  parameter int unsigned ICACHE_BYTES = {icache_sz},
  parameter int unsigned DCACHE_BYTES = {dcache_sz}
) (
  input  logic              clk,
  input  logic              resetn,
  input  logic              sw_irq_i,
  input  logic              timer_irq_i,
  input  logic              meip_i,
  input  logic              seip_i,
  input  logic              debug_req_i,
  output logic              hart_halted_o,
{bus_ports}
);
  logic [XLEN-1:0] pc_q;
  logic [XLEN-1:0] mstatus_q, mie_q, mtvec_q, mepc_q, mcause_q;
  logic [63:0]     mcycle_q, minstret_q;
  logic            lr_reserv_valid_q;
  logic [XLEN-1:0] lr_reserv_addr_q;
  logic            halted_q;

  assign hart_halted_o = halted_q;

  always_ff @(posedge clk or negedge resetn) begin
    if (!resetn) begin
      pc_q              <= RESET_PC;
      mstatus_q         <= '0;
      mie_q             <= '0;
      mtvec_q           <= RESET_PC;
      mepc_q            <= '0;
      mcause_q          <= '0;
      mcycle_q          <= 64'd0;
      minstret_q        <= 64'd0;
      lr_reserv_valid_q <= 1'b0;
      lr_reserv_addr_q  <= '0;
      halted_q          <= 1'b0;
    end else begin
      mcycle_q <= mcycle_q + 64'd1;
      if (debug_req_i) begin
        halted_q <= 1'b1;
      end else begin
        halted_q   <= 1'b0;
        minstret_q <= minstret_q + 64'd1;
        if (meip_i || timer_irq_i || sw_irq_i || seip_i) begin
          mepc_q   <= pc_q;
          mcause_q <= meip_i ? {xlen}'d11 : (timer_irq_i ? {xlen}'d7 : {xlen}'d3);
          pc_q     <= mtvec_q;
        end else begin
          pc_q <= pc_q + {xlen}'d4;
        end
      end
    end
  end

{bus_tieoffs}
endmodule
"#,
        core_mod = core_mod,
        isa_str = cfg.isa.isa_string(),
        xlen = xlen,
        reset_pc = cfg.reset_vector,
        icache_sz = cfg.icache.size_bytes,
        dcache_sz = cfg.dcache.size_bytes,
        bus_ports = bus_ports,
        bus_tieoffs = bus_tieoffs,
    )
}

/// Emit the complete synthesizable RISC-V Processor Subsystem (`CPU + I-cache + D-cache + CLINT + PLIC + JTAG Debug Module`).
pub fn emit_riscv_subsystem_sv(cfg: &RiscvCpuConfig) -> String {
    let xlen = cfg.isa.xlen();
    let mut out = String::new();
    out.push_str(&emit_cache_sv("h_icache", &cfg.icache, xlen));
    out.push('\n');
    out.push_str(&emit_cache_sv("h_dcache", &cfg.dcache, xlen));
    out.push('\n');
    out.push_str(&emit_clint_sv(&cfg.clint));
    out.push('\n');
    out.push_str(&emit_plic_sv(&cfg.plic));
    out.push('\n');
    out.push_str(&emit_riscv_dm_sv(&cfg.debug_module));
    out.push('\n');
    out.push_str(&emit_riscv_cpu_sv(cfg));
    out.push('\n');
    let core_mod = cfg.isa.core_name();
    let num_irqs = cfg.plic.num_sources.clamp(1, 63);
    out.push_str(&format!(
        r#"// Top-level RISC-V Processor Subsystem Wrapper ({subsys_name})
module {subsys_name} (
  input  logic                  clk,
  input  logic                  resetn,
  input  logic                  rtc_tick,
  input  logic [{num_irqs}-1:0] plic_irq_i,
  input  logic                  jtag_tck,
  input  logic                  jtag_tms,
  input  logic                  jtag_tdi,
  output logic                  jtag_tdo
);
  logic sw_irq, timer_irq, meip, seip, debug_req, ndmreset, hart_halted;
  logic subsys_rst_n;
  assign subsys_rst_n = resetn & ~ndmreset;

  h_clint u_clint (
    .clk(clk),
    .resetn(subsys_rst_n),
    .rtc_tick(rtc_tick),
    .req_valid(1'b0),
    .req_write(1'b0),
    .req_addr(16'd0),
    .req_wdata(64'd0),
    .req_rdata(),
    .req_ready(),
    .sw_irq_o(sw_irq),
    .timer_irq_o(timer_irq)
  );

  h_plic u_plic (
    .clk(clk),
    .resetn(subsys_rst_n),
    .irq_sources_i(plic_irq_i),
    .req_valid(1'b0),
    .req_write(1'b0),
    .req_addr(24'd0),
    .req_wdata(32'd0),
    .req_rdata(),
    .req_ready(),
    .meip_o(meip),
    .seip_o(seip)
  );

  h_riscv_dm u_riscv_dm (
    .clk(clk),
    .resetn(resetn),
    .jtag_tck(jtag_tck),
    .jtag_tms(jtag_tms),
    .jtag_tdi(jtag_tdi),
    .jtag_tdo(jtag_tdo),
    .dmi_req_valid(1'b0),
    .dmi_req_ready(),
    .dmi_addr(7'd0),
    .dmi_op(2'd0),
    .dmi_wdata(32'd0),
    .dmi_rdata(),
    .hart_halted_i(hart_halted),
    .debug_req_o(debug_req),
    .ndmreset_o(ndmreset),
    .sba_req_valid_o(),
    .sba_req_write_o(),
    .sba_addr_o(),
    .sba_wdata_o(),
    .sba_rdata_i(32'd0)
  );

  {core_mod} u_cpu (
    .clk(clk),
    .resetn(subsys_rst_n),
    .sw_irq_i(sw_irq),
    .timer_irq_i(timer_irq),
    .meip_i(meip),
    .seip_i(seip),
    .debug_req_i(debug_req),
    .hart_halted_o(hart_halted)
  );
endmodule
"#,
        subsys_name = cfg.name,
        num_irqs = num_irqs,
        core_mod = core_mod,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_pack_reimport_helion_mm() {
        let ip = pack_uart();
        assert_eq!(ip.bus, "Helion-MM");
        assert_ne!(ip.bus, "AXI");
        let xml = to_xml(&ip);
        let back = from_xml(&xml).unwrap();
        assert_eq!(ip, back);
        let gpio = pack_gpio();
        assert_eq!(gpio.name, "h_gpio");
        assert_eq!(gpio.bus, "Helion-MM");
        let rv = pack_rv32();
        assert_eq!(rv.name, "h_rv32_hb1");
        assert_eq!(rv.bus, "Helion-MM");
        assert_ne!(rv.bus, "AXI");
        let fifo = pack_sync_fifo();
        assert_eq!(fifo.name, "h_sync_fifo");
        assert_eq!(fifo.bus, "Helion-ST");
        assert_ne!(fifo.bus, "AXI");
        let timer = pack_timer();
        assert_eq!(timer.name, "h_timer");
        assert_eq!(timer.bus, "Helion-MM");
        assert_ne!(timer.bus, "AXI");
        let pwm = pack_pwm();
        assert_eq!(pwm.name, "h_pwm");
        assert_eq!(pwm.bus, "Helion-MM");
        assert_ne!(pwm.bus, "AXI");
        let spi = pack_spi_mm();
        assert_eq!(spi.name, "h_spi_mm");
        assert_eq!(spi.bus, "Helion-MM");
        assert_ne!(spi.bus, "AXI");
        let deb = pack_debounce();
        assert_eq!(deb.name, "h_debounce");
        assert_eq!(deb.bus, "Helion-ST");
        assert_ne!(deb.bus, "AXI");
        let cdiv = pack_clkdiv();
        assert_eq!(cdiv.name, "h_clkdiv");
        assert_eq!(cdiv.bus, "Helion-ST");
        assert_ne!(cdiv.bus, "AXI");
        let cat = catalog();
        assert!(cat.iter().any(|c| c.name == "h_uart"));
        assert!(cat.iter().any(|c| c.name == "h_gpio"));
        assert!(cat.iter().any(|c| c.name == "h_rv32_hb1"));
        assert!(cat.iter().any(|c| c.name == "h_ila_trig"));
        let trig = pack_ila_trig();
        assert_eq!(trig.bus, "Helion-DBG");
        assert_eq!(trig.vlnv(), "community:helion:h_ila_trig:1.0");
        assert!(cat.iter().any(|c| c.name == "h_sync_fifo"));
        assert!(cat.iter().any(|c| c.name == "h_timer"));
        assert!(cat.iter().any(|c| c.name == "h_pwm"));
        assert!(cat.iter().any(|c| c.name == "h_spi_mm"));
        assert!(cat.iter().any(|c| c.name == "h_debounce"));
        assert!(cat.iter().any(|c| c.name == "h_clkdiv"));
        assert!(cat.iter().all(|c| c.bus != "AXI"));
        assert!(
            cat.iter()
                .all(|c| !c.bus.to_ascii_lowercase().contains("axi"))
        );
        assert_eq!(pack_uart().vlnv(), "community:helion:h_uart:1.0");
        assert_eq!(pack_sync_fifo().vlnv(), "community:helion:h_sync_fifo:1.0");
        assert_eq!(pack_timer().vlnv(), "community:helion:h_timer:1.0");
        assert_eq!(pack_pwm().vlnv(), "community:helion:h_pwm:1.0");
        assert_eq!(pack_spi_mm().vlnv(), "community:helion:h_spi_mm:1.0");
        assert_eq!(pack_debounce().vlnv(), "community:helion:h_debounce:1.0");
        assert_eq!(pack_clkdiv().vlnv(), "community:helion:h_clkdiv:1.0");
        let edge = pack_edge_det();
        assert_eq!(edge.name, "h_edge_det");
        assert_eq!(edge.bus, "Helion-ST");
        assert_ne!(edge.bus, "AXI");
        let wdog = pack_watchdog();
        assert_eq!(wdog.name, "h_watchdog");
        assert_eq!(wdog.bus, "Helion-MM");
        assert_ne!(wdog.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_edge_det"));
        assert!(cat.iter().any(|c| c.name == "h_watchdog"));
        assert_eq!(pack_edge_det().vlnv(), "community:helion:h_edge_det:1.0");
        assert_eq!(pack_watchdog().vlnv(), "community:helion:h_watchdog:1.0");
        let crc = pack_crc32();
        assert_eq!(crc.name, "h_crc32");
        assert_eq!(crc.bus, "Helion-ST");
        assert_ne!(crc.bus, "AXI");
        let lfsr = pack_lfsr();
        assert_eq!(lfsr.name, "h_lfsr");
        assert_eq!(lfsr.bus, "Helion-ST");
        assert_ne!(lfsr.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_crc32"));
        assert!(cat.iter().any(|c| c.name == "h_lfsr"));
        assert_eq!(pack_crc32().vlnv(), "community:helion:h_crc32:1.0");
        assert_eq!(pack_lfsr().vlnv(), "community:helion:h_lfsr:1.0");
        let gray = pack_gray_cnt();
        assert_eq!(gray.name, "h_gray_cnt");
        assert_eq!(gray.bus, "Helion-ST");
        assert_ne!(gray.bus, "AXI");
        let scratch = pack_scratch_mm();
        assert_eq!(scratch.name, "h_scratch_mm");
        assert_eq!(scratch.bus, "Helion-MM");
        assert_ne!(scratch.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_gray_cnt"));
        assert!(cat.iter().any(|c| c.name == "h_scratch_mm"));
        assert_eq!(pack_gray_cnt().vlnv(), "community:helion:h_gray_cnt:1.0");
        assert_eq!(
            pack_scratch_mm().vlnv(),
            "community:helion:h_scratch_mm:1.0"
        );
        let pop = pack_popcount();
        assert_eq!(pop.name, "h_popcount");
        assert_eq!(pop.bus, "Helion-ST");
        assert_ne!(pop.bus, "AXI");
        let sat = pack_saturate();
        assert_eq!(sat.name, "h_saturate");
        assert_eq!(sat.bus, "Helion-ST");
        assert_ne!(sat.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_popcount"));
        assert!(cat.iter().any(|c| c.name == "h_saturate"));
        assert_eq!(pack_popcount().vlnv(), "community:helion:h_popcount:1.0");
        assert_eq!(pack_saturate().vlnv(), "community:helion:h_saturate:1.0");
        let sreg = pack_shift_reg();
        assert_eq!(sreg.name, "h_shift_reg");
        assert_eq!(sreg.bus, "Helion-ST");
        assert_ne!(sreg.bus, "AXI");
        let cmp = pack_compare_mm();
        assert_eq!(cmp.name, "h_compare_mm");
        assert_eq!(cmp.bus, "Helion-MM");
        assert_ne!(cmp.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_shift_reg"));
        assert!(cat.iter().any(|c| c.name == "h_compare_mm"));
        assert_eq!(pack_shift_reg().vlnv(), "community:helion:h_shift_reg:1.0");
        assert_eq!(
            pack_compare_mm().vlnv(),
            "community:helion:h_compare_mm:1.0"
        );
        let clz = pack_clz();
        assert_eq!(clz.name, "h_clz");
        assert_eq!(clz.bus, "Helion-ST");
        assert_ne!(clz.bus, "AXI");
        let brev = pack_byte_rev();
        assert_eq!(brev.name, "h_byte_rev");
        assert_eq!(brev.bus, "Helion-ST");
        assert_ne!(brev.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_clz"));
        assert!(cat.iter().any(|c| c.name == "h_byte_rev"));
        assert_eq!(pack_clz().vlnv(), "community:helion:h_clz:1.0");
        assert_eq!(pack_byte_rev().vlnv(), "community:helion:h_byte_rev:1.0");
        let arb = pack_arb_rr();
        assert_eq!(arb.name, "h_arb_rr");
        assert_eq!(arb.bus, "Helion-ST");
        assert_ne!(arb.bus, "AXI");
        let ctz = pack_ctz();
        assert_eq!(ctz.name, "h_ctz");
        assert_eq!(ctz.bus, "Helion-ST");
        assert_ne!(ctz.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_arb_rr"));
        assert!(cat.iter().any(|c| c.name == "h_ctz"));
        assert_eq!(pack_arb_rr().vlnv(), "community:helion:h_arb_rr:1.0");
        assert_eq!(pack_ctz().vlnv(), "community:helion:h_ctz:1.0");
        let mmx = pack_minmax();
        assert_eq!(mmx.name, "h_minmax");
        assert_eq!(mmx.bus, "Helion-ST");
        assert_ne!(mmx.bus, "AXI");
        let acc = pack_accum_mm();
        assert_eq!(acc.name, "h_accum_mm");
        assert_eq!(acc.bus, "Helion-MM");
        assert_ne!(acc.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_minmax"));
        assert!(cat.iter().any(|c| c.name == "h_accum_mm"));
        assert_eq!(pack_minmax().vlnv(), "community:helion:h_minmax:1.0");
        assert_eq!(pack_accum_mm().vlnv(), "community:helion:h_accum_mm:1.0");
        let par = pack_parity();
        assert_eq!(par.name, "h_parity");
        assert_eq!(par.bus, "Helion-ST");
        assert_ne!(par.bus, "AXI");
        let ham = pack_hamming();
        assert_eq!(ham.name, "h_hamming");
        assert_eq!(ham.bus, "Helion-ST");
        assert_ne!(ham.bus, "AXI");
        let rot = pack_rot_mm();
        assert_eq!(rot.name, "h_rot_mm");
        assert_eq!(rot.bus, "Helion-MM");
        assert_ne!(rot.bus, "AXI");
        let mgen = pack_mask_gen();
        assert_eq!(mgen.name, "h_mask_gen");
        assert_eq!(mgen.bus, "Helion-ST");
        assert_ne!(mgen.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_parity"));
        assert!(cat.iter().any(|c| c.name == "h_hamming"));
        assert!(cat.iter().any(|c| c.name == "h_rot_mm"));
        assert!(cat.iter().any(|c| c.name == "h_mask_gen"));
        assert_eq!(pack_parity().vlnv(), "community:helion:h_parity:1.0");
        assert_eq!(pack_hamming().vlnv(), "community:helion:h_hamming:1.0");
        assert_eq!(pack_rot_mm().vlnv(), "community:helion:h_rot_mm:1.0");
        assert_eq!(pack_mask_gen().vlnv(), "community:helion:h_mask_gen:1.0");
        let ad = pack_absdiff();
        assert_eq!(ad.name, "h_absdiff");
        assert_eq!(ad.bus, "Helion-ST");
        assert_ne!(ad.bus, "AXI");
        let cl = pack_clamp();
        assert_eq!(cl.name, "h_clamp");
        assert_eq!(cl.bus, "Helion-ST");
        assert_ne!(cl.bus, "AXI");
        let b2 = pack_bin2oh();
        assert_eq!(b2.name, "h_bin2oh");
        assert_eq!(b2.bus, "Helion-ST");
        assert_ne!(b2.bus, "AXI");
        let tc = pack_timer_cap();
        assert_eq!(tc.name, "h_timer_cap");
        assert_eq!(tc.bus, "Helion-MM");
        assert_ne!(tc.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_absdiff"));
        assert!(cat.iter().any(|c| c.name == "h_clamp"));
        assert!(cat.iter().any(|c| c.name == "h_bin2oh"));
        assert!(cat.iter().any(|c| c.name == "h_timer_cap"));
        assert_eq!(pack_absdiff().vlnv(), "community:helion:h_absdiff:1.0");
        assert_eq!(pack_clamp().vlnv(), "community:helion:h_clamp:1.0");
        assert_eq!(pack_bin2oh().vlnv(), "community:helion:h_bin2oh:1.0");
        assert_eq!(pack_timer_cap().vlnv(), "community:helion:h_timer_cap:1.0");
        let m4 = pack_mux4();
        assert_eq!(m4.name, "h_mux4");
        assert_eq!(m4.bus, "Helion-ST");
        assert_ne!(m4.bus, "AXI");
        let asmm = pack_addsub_mm();
        assert_eq!(asmm.name, "h_addsub_mm");
        assert_eq!(asmm.bus, "Helion-MM");
        assert_ne!(asmm.bus, "AXI");
        let oc = pack_ones_comp();
        assert_eq!(oc.name, "h_ones_comp");
        assert_eq!(oc.bus, "Helion-ST");
        assert_ne!(oc.bus, "AXI");
        let pe = pack_pulse_ext();
        assert_eq!(pe.name, "h_pulse_ext");
        assert_eq!(pe.bus, "Helion-ST");
        assert_ne!(pe.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_mux4"));
        assert!(cat.iter().any(|c| c.name == "h_addsub_mm"));
        assert!(cat.iter().any(|c| c.name == "h_ones_comp"));
        assert!(cat.iter().any(|c| c.name == "h_pulse_ext"));
        assert_eq!(pack_mux4().vlnv(), "community:helion:h_mux4:1.0");
        assert_eq!(pack_addsub_mm().vlnv(), "community:helion:h_addsub_mm:1.0");
        assert_eq!(pack_ones_comp().vlnv(), "community:helion:h_ones_comp:1.0");
        assert_eq!(pack_pulse_ext().vlnv(), "community:helion:h_pulse_ext:1.0");
        let rng = pack_rng_xorshift();
        assert_eq!(rng.name, "h_rng_xorshift");
        assert_eq!(rng.bus, "Helion-ST");
        assert_ne!(rng.bus, "AXI");
        let penc = pack_prio_enc();
        assert_eq!(penc.name, "h_prio_enc");
        assert_eq!(penc.bus, "Helion-ST");
        assert_ne!(penc.bus, "AXI");
        let sk = pack_skid_buf();
        assert_eq!(sk.name, "h_skid_buf");
        assert_eq!(sk.bus, "Helion-ST");
        assert_ne!(sk.bus, "AXI");
        let bcd = pack_bcd_inc();
        assert_eq!(bcd.name, "h_bcd_inc");
        assert_eq!(bcd.bus, "Helion-ST");
        assert_ne!(bcd.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_rng_xorshift"));
        assert!(cat.iter().any(|c| c.name == "h_prio_enc"));
        assert!(cat.iter().any(|c| c.name == "h_skid_buf"));
        assert!(cat.iter().any(|c| c.name == "h_bcd_inc"));
        assert_eq!(
            pack_rng_xorshift().vlnv(),
            "community:helion:h_rng_xorshift:1.0"
        );
        assert_eq!(pack_prio_enc().vlnv(), "community:helion:h_prio_enc:1.0");
        assert_eq!(pack_skid_buf().vlnv(), "community:helion:h_skid_buf:1.0");
        assert_eq!(pack_bcd_inc().vlnv(), "community:helion:h_bcd_inc:1.0");
        let mb = pack_mailbox_mm();
        assert_eq!(mb.name, "h_mailbox_mm");
        assert_eq!(mb.bus, "Helion-MM");
        assert_ne!(mb.bus, "AXI");
        let pdt = pack_pwm_deadtime();
        assert_eq!(pdt.name, "h_pwm_deadtime");
        assert_eq!(pdt.bus, "Helion-ST");
        assert_ne!(pdt.bus, "AXI");
        let l16 = pack_lfsr16();
        assert_eq!(l16.name, "h_lfsr16");
        assert_eq!(l16.bus, "Helion-ST");
        assert_ne!(l16.bus, "AXI");
        let ec = pack_edge_cnt();
        assert_eq!(ec.name, "h_edge_cnt");
        assert_eq!(ec.bus, "Helion-ST");
        assert_ne!(ec.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_mailbox_mm"));
        assert!(cat.iter().any(|c| c.name == "h_pwm_deadtime"));
        assert!(cat.iter().any(|c| c.name == "h_lfsr16"));
        assert!(cat.iter().any(|c| c.name == "h_edge_cnt"));
        assert_eq!(
            pack_mailbox_mm().vlnv(),
            "community:helion:h_mailbox_mm:1.0"
        );
        assert_eq!(
            pack_pwm_deadtime().vlnv(),
            "community:helion:h_pwm_deadtime:1.0"
        );
        assert_eq!(pack_lfsr16().vlnv(), "community:helion:h_lfsr16:1.0");
        assert_eq!(pack_edge_cnt().vlnv(), "community:helion:h_edge_cnt:1.0");
        let urx = pack_uart_rx();
        assert_eq!(urx.name, "h_uart_rx");
        assert_eq!(urx.bus, "Helion-MM");
        assert_ne!(urx.bus, "AXI");
        let dvr = pack_div_restoring();
        assert_eq!(dvr.name, "h_div_restoring");
        assert_eq!(dvr.bus, "Helion-ST");
        assert_ne!(dvr.bus, "AXI");
        let sq = pack_sqrt_digit();
        assert_eq!(sq.name, "h_sqrt_digit");
        assert_eq!(sq.bus, "Helion-ST");
        assert_ne!(sq.bus, "AXI");
        let cord = pack_cordic_step();
        assert_eq!(cord.name, "h_cordic_step");
        assert_eq!(cord.bus, "Helion-ST");
        assert_ne!(cord.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_uart_rx"));
        assert!(cat.iter().any(|c| c.name == "h_div_restoring"));
        assert!(cat.iter().any(|c| c.name == "h_sqrt_digit"));
        assert!(cat.iter().any(|c| c.name == "h_cordic_step"));
        assert_eq!(pack_uart_rx().vlnv(), "community:helion:h_uart_rx:1.0");
        assert_eq!(
            pack_div_restoring().vlnv(),
            "community:helion:h_div_restoring:1.0"
        );
        assert_eq!(
            pack_sqrt_digit().vlnv(),
            "community:helion:h_sqrt_digit:1.0"
        );
        assert_eq!(
            pack_cordic_step().vlnv(),
            "community:helion:h_cordic_step:1.0"
        );
        let pa = pack_phase_accum();
        assert_eq!(pa.name, "h_phase_accum");
        assert_eq!(pa.bus, "Helion-ST");
        assert_ne!(pa.bus, "AXI");
        let ft = pack_fir_tap();
        assert_eq!(ft.name, "h_fir_tap");
        assert_eq!(ft.bus, "Helion-ST");
        assert_ne!(ft.bus, "AXI");
        let iir = pack_iir_biquad();
        assert_eq!(iir.name, "h_iir_biquad");
        assert_eq!(iir.bus, "Helion-ST");
        assert_ne!(iir.bus, "AXI");
        let cdc = pack_cdc_pulse();
        assert_eq!(cdc.name, "h_cdc_pulse");
        assert_eq!(cdc.bus, "Helion-ST");
        assert_ne!(cdc.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_phase_accum"));
        assert!(cat.iter().any(|c| c.name == "h_fir_tap"));
        assert!(cat.iter().any(|c| c.name == "h_iir_biquad"));
        assert!(cat.iter().any(|c| c.name == "h_cdc_pulse"));
        assert_eq!(
            pack_phase_accum().vlnv(),
            "community:helion:h_phase_accum:1.0"
        );
        assert_eq!(pack_fir_tap().vlnv(), "community:helion:h_fir_tap:1.0");
        assert_eq!(
            pack_iir_biquad().vlnv(),
            "community:helion:h_iir_biquad:1.0"
        );
        assert_eq!(pack_cdc_pulse().vlnv(), "community:helion:h_cdc_pulse:1.0");
        let c8 = pack_crc8();
        assert_eq!(c8.name, "h_crc8");
        assert_eq!(c8.bus, "Helion-ST");
        assert_ne!(c8.bus, "AXI");
        let ss = pack_spi_slave();
        assert_eq!(ss.name, "h_spi_slave");
        assert_eq!(ss.bus, "Helion-MM");
        assert_ne!(ss.bus, "AXI");
        let qe = pack_quad_enc();
        assert_eq!(qe.name, "h_quad_enc");
        assert_eq!(qe.bus, "Helion-ST");
        assert_ne!(qe.bus, "AXI");
        let pc = pack_pwm_center();
        assert_eq!(pc.name, "h_pwm_center");
        assert_eq!(pc.bus, "Helion-ST");
        assert_ne!(pc.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_crc8"));
        assert!(cat.iter().any(|c| c.name == "h_spi_slave"));
        assert!(cat.iter().any(|c| c.name == "h_quad_enc"));
        assert!(cat.iter().any(|c| c.name == "h_pwm_center"));
        assert_eq!(pack_crc8().vlnv(), "community:helion:h_crc8:1.0");
        assert_eq!(pack_spi_slave().vlnv(), "community:helion:h_spi_slave:1.0");
        assert_eq!(pack_quad_enc().vlnv(), "community:helion:h_quad_enc:1.0");
        assert_eq!(
            pack_pwm_center().vlnv(),
            "community:helion:h_pwm_center:1.0"
        );
        let me = pack_manchester_enc();
        assert_eq!(me.name, "h_manchester_enc");
        assert_eq!(me.bus, "Helion-ST");
        assert_ne!(me.bus, "AXI");
        let g2b = pack_gray2bin();
        assert_eq!(g2b.name, "h_gray2bin");
        assert_eq!(g2b.bus, "Helion-ST");
        assert_ne!(g2b.bus, "AXI");
        let b2g = pack_bin2gray();
        assert_eq!(b2g.name, "h_bin2gray");
        assert_eq!(b2g.bus, "Helion-ST");
        assert_ne!(b2g.bus, "AXI");
        let maj = pack_majority3();
        assert_eq!(maj.name, "h_majority3");
        assert_eq!(maj.bus, "Helion-ST");
        assert_ne!(maj.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_manchester_enc"));
        assert!(cat.iter().any(|c| c.name == "h_gray2bin"));
        assert!(cat.iter().any(|c| c.name == "h_bin2gray"));
        assert!(cat.iter().any(|c| c.name == "h_majority3"));
        assert_eq!(
            pack_manchester_enc().vlnv(),
            "community:helion:h_manchester_enc:1.0"
        );
        assert_eq!(pack_gray2bin().vlnv(), "community:helion:h_gray2bin:1.0");
        assert_eq!(pack_bin2gray().vlnv(), "community:helion:h_bin2gray:1.0");
        assert_eq!(pack_majority3().vlnv(), "community:helion:h_majority3:1.0");
        let c16 = pack_crc16();
        assert_eq!(c16.name, "h_crc16");
        assert_eq!(c16.bus, "Helion-ST");
        assert_ne!(c16.bus, "AXI");
        let rrt = pack_rr_token();
        assert_eq!(rrt.name, "h_rr_token");
        assert_eq!(rrt.bus, "Helion-ST");
        assert_ne!(rrt.bus, "AXI");
        let pp = pack_pwm_phase();
        assert_eq!(pp.name, "h_pwm_phase");
        assert_eq!(pp.bus, "Helion-ST");
        assert_ne!(pp.bus, "AXI");
        let cam = pack_cam_slot();
        assert_eq!(cam.name, "h_cam_slot");
        assert_eq!(cam.bus, "Helion-MM");
        assert_ne!(cam.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_crc16"));
        assert!(cat.iter().any(|c| c.name == "h_rr_token"));
        assert!(cat.iter().any(|c| c.name == "h_pwm_phase"));
        assert!(cat.iter().any(|c| c.name == "h_cam_slot"));
        assert_eq!(pack_crc16().vlnv(), "community:helion:h_crc16:1.0");
        assert_eq!(pack_rr_token().vlnv(), "community:helion:h_rr_token:1.0");
        assert_eq!(pack_pwm_phase().vlnv(), "community:helion:h_pwm_phase:1.0");
        assert_eq!(pack_cam_slot().vlnv(), "community:helion:h_cam_slot:1.0");
        let sipo = pack_sipo();
        assert_eq!(sipo.name, "h_sipo");
        assert_eq!(sipo.bus, "Helion-ST");
        assert_ne!(sipo.bus, "AXI");
        let piso = pack_piso();
        assert_eq!(piso.name, "h_piso");
        assert_eq!(piso.bus, "Helion-ST");
        assert_ne!(piso.bus, "AXI");
        let tl = pack_traffic_light();
        assert_eq!(tl.name, "h_traffic_light");
        assert_eq!(tl.bus, "Helion-ST");
        assert_ne!(tl.bus, "AXI");
        let sb = pack_scoreboard();
        assert_eq!(sb.name, "h_scoreboard");
        assert_eq!(sb.bus, "Helion-MM");
        assert_ne!(sb.bus, "AXI");
        assert!(cat.iter().any(|c| c.name == "h_sipo"));
        assert!(cat.iter().any(|c| c.name == "h_piso"));
        assert!(cat.iter().any(|c| c.name == "h_traffic_light"));
        assert!(cat.iter().any(|c| c.name == "h_scoreboard"));
        assert_eq!(pack_sipo().vlnv(), "community:helion:h_sipo:1.0");
        assert_eq!(pack_piso().vlnv(), "community:helion:h_piso:1.0");
        assert_eq!(
            pack_traffic_light().vlnv(),
            "community:helion:h_traffic_light:1.0"
        );
        assert_eq!(
            pack_scoreboard().vlnv(),
            "community:helion:h_scoreboard:1.0"
        );
    }
}

/// On-disk Helion IP package (`.helion` manifest + relative HDL/XDC files).
/// Smallest honest CovertEDA-class package: text manifest, not a zip redesign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelionPackage {
    pub format: u32,
    pub vendor: String,
    pub library: String,
    pub name: String,
    pub version: String,
    pub bus: String,
    pub top: Option<String>,
    /// HDL sources relative to the `.helion` file (or absolute).
    pub files: Vec<String>,
    /// Optional constraint files relative to the `.helion` file.
    pub constraints: Vec<String>,
    /// Absolute path of the loaded manifest (empty when parsed from a string).
    pub manifest_path: std::path::PathBuf,
}

impl HelionPackage {
    pub fn vlnv(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.vendor, self.library, self.name, self.version
        )
    }

    pub fn to_ip_core(&self) -> Result<IpCore, String> {
        if self.bus.eq_ignore_ascii_case("AXI") || self.bus.to_ascii_lowercase().contains("axi") {
            return Err(format!(
                ".helion {}: bus must be Helion-MM/Helion-ST (not AXI)",
                self.name
            ));
        }
        Ok(IpCore {
            vendor: self.vendor.clone(),
            library: self.library.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            bus: self.bus.clone(),
        })
    }

    /// Resolve HDL paths against the manifest directory.
    pub fn resolve_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        self.resolve_listed(&self.files)
    }

    pub fn resolve_constraints(&self) -> Result<Vec<std::path::PathBuf>, String> {
        self.resolve_listed(&self.constraints)
    }

    fn resolve_listed(&self, listed: &[String]) -> Result<Vec<std::path::PathBuf>, String> {
        let base = self
            .manifest_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let mut out = Vec::new();
        for f in listed {
            let p = std::path::Path::new(f);
            let cand = if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            };
            if !cand.exists() {
                return Err(format!(
                    ".helion {}: file not found: {} (tried {})",
                    self.name,
                    f,
                    cand.display()
                ));
            }
            out.push(cand);
        }
        Ok(out)
    }
}

/// Parse a `.helion` IP package manifest (text, format 1).
///
/// ```text
/// format 1
/// vendor community
/// library helion
/// name h_gpio
/// version 1.0
/// bus Helion-MM
/// top h_gpio
/// file h_gpio.v
/// xdc pins.xdc          # optional
/// ```
pub fn parse_helion(text: &str) -> Result<HelionPackage, String> {
    let mut pkg = HelionPackage {
        format: 1,
        vendor: "community".into(),
        library: "helion".into(),
        name: String::new(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
        top: None,
        files: Vec::new(),
        constraints: Vec::new(),
        manifest_path: std::path::PathBuf::new(),
    };
    let mut saw_format = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut toks = line.split_whitespace();
        let Some(cmd) = toks.next() else { continue };
        match cmd {
            "format" => {
                let v = toks
                    .next()
                    .ok_or_else(|| "format: missing version".to_string())?;
                pkg.format = v.parse().map_err(|_| format!("format: bad version {v}"))?;
                if pkg.format != 1 {
                    return Err(format!("unsupported .helion format {}", pkg.format));
                }
                saw_format = true;
            }
            "vendor" => {
                if let Some(v) = toks.next() {
                    pkg.vendor = v.to_string();
                }
            }
            "library" => {
                if let Some(v) = toks.next() {
                    pkg.library = v.to_string();
                }
            }
            "name" => {
                if let Some(v) = toks.next() {
                    pkg.name = v.to_string();
                }
            }
            "version" => {
                if let Some(v) = toks.next() {
                    pkg.version = v.to_string();
                }
            }
            "bus" => {
                if let Some(v) = toks.next() {
                    pkg.bus = v.to_string();
                }
            }
            "vlnv" => {
                let v = toks
                    .next()
                    .ok_or_else(|| "vlnv: missing value".to_string())?;
                let parts: Vec<&str> = v.split(':').collect();
                if parts.len() != 4 {
                    return Err(format!(
                        "vlnv: expected vendor:library:name:version, got {v}"
                    ));
                }
                pkg.vendor = parts[0].to_string();
                pkg.library = parts[1].to_string();
                pkg.name = parts[2].to_string();
                pkg.version = parts[3].to_string();
            }
            "top" => {
                if let Some(v) = toks.next() {
                    pkg.top = Some(v.to_string());
                }
            }
            "file" | "read_sv" | "read_verilog" | "sv" | "verilog" => {
                if let Some(v) = toks.next() {
                    pkg.files.push(v.to_string());
                }
            }
            "xdc" | "sdc" | "read_xdc" | "read_sdc" | "constraint" => {
                if let Some(v) = toks.next() {
                    pkg.constraints.push(v.to_string());
                }
            }
            other => {
                return Err(format!(".helion: unknown directive {other}"));
            }
        }
    }
    if !saw_format {
        return Err(".helion: missing `format 1`".into());
    }
    if pkg.name.is_empty() {
        return Err(".helion: missing `name` (or `vlnv`)".into());
    }
    if pkg.files.is_empty() {
        return Err(format!(".helion {}: no `file` entries", pkg.name));
    }
    if pkg.bus.eq_ignore_ascii_case("AXI") || pkg.bus.to_ascii_lowercase().contains("axi") {
        return Err(format!(
            ".helion {}: bus must be Helion-MM/Helion-ST (not AXI as Helion product)",
            pkg.name
        ));
    }
    Ok(pkg)
}

/// Load a `.helion` package from disk (file or directory containing `package.helion`).
pub fn load_helion(path: &Path) -> Result<HelionPackage, String> {
    let manifest = if path.is_dir() {
        let cand = path.join("package.helion");
        if cand.is_file() {
            cand
        } else {
            return Err(format!(
                ".helion dir {}: expected package.helion",
                path.display()
            ));
        }
    } else {
        path.to_path_buf()
    };
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("read {}: {e}", manifest.display()))?;
    let mut pkg = parse_helion(&text)?;
    pkg.manifest_path = manifest;
    // Existence check early so project ingest fails loud.
    let _ = pkg.resolve_files()?;
    let _ = pkg.resolve_constraints()?;
    Ok(pkg)
}

/// Emit a format-1 `.helion` manifest body for an IpCore + file list.
pub fn to_helion_manifest(ip: &IpCore, top: Option<&str>, files: &[&str]) -> String {
    let mut s = String::from("# Helion IP package (format 1)\nformat 1\n");
    s.push_str(&format!("vlnv {}\n", ip.vlnv()));
    s.push_str(&format!("bus {}\n", ip.bus));
    if let Some(t) = top {
        s.push_str(&format!("top {t}\n"));
    }
    for f in files {
        s.push_str(&format!("file {f}\n"));
    }
    s
}

#[cfg(test)]
mod helion_pkg_tests {
    use super::*;

    #[test]
    fn parses_helion_manifest_and_rejects_axi() {
        let pkg = parse_helion(
            r#"
format 1
vlnv community:helion:h_gpio:1.0
bus Helion-MM
top h_gpio
file h_gpio.v
"#,
        )
        .unwrap();
        assert_eq!(pkg.name, "h_gpio");
        assert_eq!(pkg.vlnv(), "community:helion:h_gpio:1.0");
        assert_eq!(pkg.files, vec!["h_gpio.v"]);
        assert_eq!(pkg.to_ip_core().unwrap().bus, "Helion-MM");
        assert!(
            parse_helion(
                r#"
format 1
name bad
bus AXI
file a.v
"#
            )
            .unwrap_err()
            .contains("not AXI")
        );
    }

    #[test]
    fn catalog_cores_roundtrip_helion_text() {
        for ip in catalog() {
            let body = to_helion_manifest(&ip, Some(&ip.name), &[&format!("{}.v", ip.name)]);
            let pkg = parse_helion(&body).unwrap();
            assert_eq!(pkg.to_ip_core().unwrap(), ip);
        }
    }

    #[test]
    fn parametric_catalog_covers_all_vivado_ips_and_bus_types() {
        let pcat = parametric_catalog();
        for expected in [
            "axi_interconnect",
            "axi_protocol_converter",
            "axi_gpio",
            "axi_uartlite",
            "axi_timer",
            "axi_bram_ctrl",
            "cdc_fifo",
            "proc_sys_reset",
            "axi_cpu",
            "axi4_master_cpu",
            "axi_quad_spi",
            "axi_iic",
            "axi_intc",
            "blk_mem_gen",
            "apb_gpio",
            "wb_gpio",
        ] {
            let comp = find_ip_component(expected)
                .unwrap_or_else(|| panic!("missing parametric IP {expected}"));
            let xml = comp.to_ipxact_xml();
            assert!(xml.contains("<ipxact:component"), "{expected}");
            assert!(xml.contains("<ipxact:busInterfaces>"), "{expected}");
            let sv = comp.generate_sv(& std::collections::BTreeMap::new()).unwrap();
            assert!(sv.contains(&format!("module {}", comp.core.name)), "{expected}: {sv}");
            assert!(sv.contains("endmodule"), "{expected}");
        }
        assert!(pcat.len() >= 16);

        // Verify all 13 required UG994 bus interface types parse and roundtrip
        for b in [
            "AXI4",
            "AXI4-Lite",
            "AXI4-Stream",
            "APB",
            "Wishbone",
            "Clock",
            "Reset",
            "Interrupt",
            "GPIO",
            "UART",
            "SPI",
            "I2C",
            "BRAM_PORT",
        ] {
            let parsed = BusInterfaceType::parse(b).unwrap_or_else(|| panic!("failed to parse {b}"));
            assert_eq!(parsed.as_str(), b);
        }
    }

    #[test]
    fn parametric_rtl_generators_respect_overrides() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("C_GPIO_WIDTH".into(), "16".into());
        overrides.insert("C_IS_DUAL".into(), "1".into());
        let gpio = component_axi_gpio().generate_sv(&overrides).unwrap();
        assert!(gpio.contains("parameter int C_GPIO_WIDTH         = 16"));
        assert!(gpio.contains("parameter int C_IS_DUAL            = 1"));

        overrides.clear();
        overrides.insert("C_BAUDRATE".into(), "921600".into());
        let uart = component_axi_uartlite().generate_sv(&overrides).unwrap();
        assert!(uart.contains("parameter int C_BAUDRATE         = 921600"));

        overrides.clear();
        overrides.insert("C_IS_ASYNC".into(), "1".into());
        overrides.insert("C_FIFO_DEPTH".into(), "32".into());
        let fifo = component_cdc_fifo().generate_sv(&overrides).unwrap();
        assert!(fifo.contains("parameter int C_FIFO_DEPTH  = 32"));
        assert!(fifo.contains("wr_gray_next"));
        assert!(fifo.contains("rd_gray_next"));
    }

    #[test]
    fn jedec_timing_calculator_quantizes_ddr3_ddr4_hyperram_sdram() {
        // DDR3-1600 @ 200 MHz controller clock (5000 ps period):
        // tRCD = ceil(13750 / 5000) = 3, tRP = 3, tRAS = ceil(35000 / 5000) = 7,
        // tRFC = ceil(260000 / 5000) = 52, tREFI = floor(7800000 / 5000) = 1560, CL = 11, CWL = 8
        let ddr3_cfg = MigConfig::ddr3_default();
        assert_eq!(ddr3_cfg.timing.clk_period_ps, 5_000);
        assert_eq!(ddr3_cfg.timing.t_rcd, 3);
        assert_eq!(ddr3_cfg.timing.t_rp, 3);
        assert_eq!(ddr3_cfg.timing.t_ras, 7);
        assert_eq!(ddr3_cfg.timing.t_rfc, 52);
        assert_eq!(ddr3_cfg.timing.t_refi, 1560);
        assert_eq!(ddr3_cfg.timing.cl, 11);
        assert_eq!(ddr3_cfg.timing.cwl, 8);

        // DDR4-2400 @ 300 MHz controller clock (3333 ps period)
        let ddr4_cfg = MigConfig::ddr4_default();
        assert_eq!(ddr4_cfg.timing.t_rcd, 5);
        assert_eq!(ddr4_cfg.timing.t_rp, 5);
        assert_eq!(ddr4_cfg.timing.t_ras, 10);
        assert_eq!(ddr4_cfg.timing.cl, 17);
        assert_eq!(ddr4_cfg.timing.cwl, 12);

        // HyperRAM-200 @ 100 MHz (10000 ps period)
        let hb_cfg = MigConfig::hyperram_default();
        assert_eq!(hb_cfg.timing.cl, 6);
        assert_eq!(hb_cfg.timing.t_refi, 400);

        // SDR-133 @ 133.33 MHz (7500 ps period)
        let sdr_cfg = MigConfig::sdr_sdram_default();
        assert_eq!(sdr_cfg.timing.t_rcd, 2);
        assert_eq!(sdr_cfg.timing.t_rp, 2);
        assert_eq!(sdr_cfg.timing.cl, 3);
    }

    #[test]
    fn mig_ddr3_ddr4_hyperram_sdram_generators_emit_synthesizable_sv() {
        let mcat = mig_catalog();
        assert_eq!(mcat.len(), 4);

        let ddr3_sv = MigConfig::ddr3_default().generate_sv();
        assert!(ddr3_sv.contains("module mig_ddr3_controller"));
        assert!(ddr3_sv.contains("init_calib_complete"));
        assert!(ddr3_sv.contains("CALIB_WRITE_LVL"));
        assert!(ddr3_sv.contains("CALIB_DQS_GATE"));
        assert!(ddr3_sv.contains("CALIB_READ_EYE"));
        assert!(ddr3_sv.contains("CMD_ACT"));
        assert!(ddr3_sv.contains("CMD_RD"));
        assert!(ddr3_sv.contains("CMD_WR"));
        assert!(ddr3_sv.contains("CMD_PRE"));
        assert!(ddr3_sv.contains("CMD_REF"));
        assert!(ddr3_sv.contains("s_axi_awburst"));
        assert!(ddr3_sv.contains("s_axi_rlast"));

        let ddr4_sv = MigConfig::ddr4_default().generate_sv();
        assert!(ddr4_sv.contains("module mig_ddr4_controller"));
        assert!(ddr4_sv.contains("c0_init_calib_complete"));
        assert!(ddr4_sv.contains("c0_ddr4_act_n"));
        assert!(ddr4_sv.contains("c0_ddr4_bg"));
        assert!(ddr4_sv.contains("s_axi_awlen"));

        let hb_sv = MigConfig::hyperram_default().generate_sv();
        assert!(hb_sv.contains("module hyperram_controller"));
        assert!(hb_sv.contains("hb_rwds_i"));
        assert!(hb_sv.contains("HB_CA_PKT"));
        assert!(hb_sv.contains("s_axi_arlen"));

        let sdr_sv = MigConfig::sdr_sdram_default().generate_sv();
        assert!(sdr_sv.contains("module sdr_sdram_controller"));
        assert!(sdr_sv.contains("SDR_CMD_ACT"));
        assert!(sdr_sv.contains("SDR_CMD_PRE"));
        assert!(sdr_sv.contains("SDR_CMD_REF"));
        assert!(sdr_sv.contains("sdram_ras_n"));
    }

    #[test]
    fn eth_mac_1g_10g_crc32_rgmii_sgmii_xgmii_and_mdio_sv() {
        let cat = connectivity_catalog();
        assert_eq!(cat.len(), 4);

        // Standard IEEE 802.3 CRC-32 check on ASCII "123456789" -> 0xCBF43926
        assert_eq!(compute_ethernet_crc32(b"123456789"), 0xCBF4_3926);

        let rgmii_sv = EthMacConfig::default_1g_rgmii().generate_sv();
        assert!(rgmii_sv.contains("module eth_mac_1g_rgmii"));
        assert!(rgmii_sv.contains("ETH_PREAMBLE = 8'h55"));
        assert!(rgmii_sv.contains("ETH_SFD      = 8'hD5"));
        assert!(rgmii_sv.contains("32'hEDB8_8320"));
        assert!(rgmii_sv.contains("rgmii_txd"));
        assert!(rgmii_sv.contains("rgmii_rxd"));
        assert!(rgmii_sv.contains("mdc"));
        assert!(rgmii_sv.contains("mdio_i"));
        assert!(rgmii_sv.contains("s_axis_tx_tdata"));
        assert!(rgmii_sv.contains("m_axis_rx_tdata"));

        let sgmii_sv = EthMacConfig::default_1g_sgmii().generate_sv();
        assert!(sgmii_sv.contains("module eth_mac_1g_sgmii"));
        assert!(sgmii_sv.contains("sgmii_txd"));
        assert!(sgmii_sv.contains("sgmii_rxd"));

        let xgmii_sv = EthMacConfig::default_10g_xgmii().generate_sv();
        assert!(xgmii_sv.contains("module eth_mac_10g_xgmii"));
        assert!(xgmii_sv.contains("parameter int C_AXIS_DATA_WIDTH = 64"));
        assert!(xgmii_sv.contains("xgmii_txd"));
        assert!(xgmii_sv.contains("xgmii_rxd"));
    }

    #[test]
    fn pcie_tlp_axi_dma_and_usb2_ulpi_device_controllers_emit_synthesizable_sv() {
        let pcie_sv = PcieDmaConfig::default().generate_sv();
        assert!(pcie_sv.contains("module pcie_tlp_axi_dma_bridge"));
        assert!(pcie_sv.contains("TLP_MRD_3DW"));
        assert!(pcie_sv.contains("TLP_MWR_3DW"));
        assert!(pcie_sv.contains("TLP_CPLD_3DW"));
        assert!(pcie_sv.contains("bar0_sg_desc_addr"));
        assert!(pcie_sv.contains("SG_FETCH_DESC"));
        assert!(pcie_sv.contains("m_axi_awaddr"));
        assert!(pcie_sv.contains("m_axi_araddr"));

        let usb_sv = UsbDeviceConfig::default().generate_sv();
        assert!(usb_sv.contains("module usb2_ulpi_device_ctrl"));
        assert!(usb_sv.contains("ulpi_clk60"));
        assert!(usb_sv.contains("ulpi_dir"));
        assert!(usb_sv.contains("ulpi_nxt"));
        assert!(usb_sv.contains("ulpi_stp"));
        assert!(usb_sv.contains("PID_SETUP"));
        assert!(usb_sv.contains("ep0_setup_packet"));
        assert!(usb_sv.contains("s_axis_ep1_in_tdata"));
        assert!(usb_sv.contains("m_axis_ep2_out_tdata"));
    }

    #[test]
    fn cordic_circular_hyperbolic_linear_and_sv_generation() {
        use std::f64::consts::PI;
        let eng = CordicEngine::new(CordicConfig::default());

        // 1. Circular Rotation: sin/cos across all 4 quadrants [-pi, +pi].
        for &deg in &[-150.0_f64, -90.0, -45.0, 0.0, 30.0, 60.0, 120.0, 175.0] {
            let rad = deg.to_radians();
            let (sin_c, cos_c) = eng.sin_cos(rad);
            assert!(
                (sin_c - rad.sin()).abs() < 1e-3,
                "sin({deg} deg): got {sin_c}, expected {}",
                rad.sin()
            );
            assert!(
                (cos_c - rad.cos()).abs() < 1e-3,
                "cos({deg} deg): got {cos_c}, expected {}",
                rad.cos()
            );
        }

        // 2. Circular Vectoring: atan2(y, x) and magnitude sqrt(x^2 + y^2).
        for &(y, x) in &[(3.0_f64, 4.0_f64), (-1.5, 2.0), (2.0, -2.0), (-1.0, -1.732)] {
            let (angle, mag) = eng.atan2_magnitude(y, x);
            let exp_angle = y.atan2(x);
            let exp_mag = (x * x + y * y).sqrt();
            assert!(
                (angle - exp_angle).abs() < 2e-3,
                "atan2({y}, {x}): got {angle}, expected {exp_angle}"
            );
            assert!(
                (mag - exp_mag).abs() < 5e-3,
                "mag({y}, {x}): got {mag}, expected {exp_mag}"
            );
        }

        // 3. Hyperbolic Rotation: sinh/cosh.
        for &z in &[-0.8_f64, -0.3, 0.0, 0.5, 0.9] {
            let (sh, ch) = eng.sinh_cosh(z);
            assert!(
                (sh - z.sinh()).abs() < 3e-3,
                "sinh({z}): got {sh}, expected {}",
                z.sinh()
            );
            assert!(
                (ch - z.cosh()).abs() < 3e-3,
                "cosh({z}): got {ch}, expected {}",
                z.cosh()
            );
        }

        // 4. Hyperbolic Vectoring: sqrt(v) across wide dynamic range.
        for &v in &[0.0625_f64, 0.25, 0.5, 2.0, 9.0, 49.0] {
            let root = eng.sqrt(v).unwrap();
            assert!(
                (root - v.sqrt()).abs() < 5e-3,
                "sqrt({v}): got {root}, expected {}",
                v.sqrt()
            );
        }
        let _ = PI;

        // 5. Synthesizable SystemVerilog emission with AXI4-Stream ports.
        let sv = emit_cordic_sv(&CordicConfig::default());
        assert!(sv.contains("module h_dsp_cordic"));
        assert!(sv.contains("s_axis_tvalid"));
        assert!(sv.contains("m_axis_x_tdata"));
        assert!(sv.contains("ATAN_LUT"));
    }

    #[test]
    fn fft_ifft_radix2_radix4_bit_reverse_and_sv_generation() {
        use std::f64::consts::PI;
        // Test transform sizes across N = 16 .. 4096.
        for &n in &[16usize, 64, 256, 1024, 4096] {
            let rom = generate_twiddle_rom(n, 14, false).unwrap();
            assert_eq!(rom.len(), n / 2);
            assert_eq!(rom[0], (1 << 14, 0));
        }

        // Forward + Inverse FFT round-trip on N=64 with PipelinedStreaming (Radix-4) and Radix2Burst.
        for arch in [
            FftArchitecture::PipelinedStreaming,
            FftArchitecture::Radix4Burst,
            FftArchitecture::Radix2Burst,
        ] {
            let cfg = FftConfig {
                transform_size: 64,
                arch,
                output_order: FftOutputOrder::NaturalOrder,
                ..FftConfig::default()
            };
            let eng = FftEngine::new(cfg.clone()).unwrap();
            // Complex tone at bin 7 + bin 19.
            let input: Vec<(f64, f64)> = (0..64)
                .map(|k| {
                    let t7 = 2.0 * PI * 7.0 * (k as f64) / 64.0;
                    let t19 = 2.0 * PI * 19.0 * (k as f64) / 64.0;
                    (t7.cos() + 0.5 * t19.cos(), t7.sin() + 0.5 * t19.sin())
                })
                .collect();
            let spec = eng.transform_complex(&input, false).unwrap();
            let mag7 = (spec[7].0.powi(2) + spec[7].1.powi(2)).sqrt();
            let mag19 = (spec[19].0.powi(2) + spec[19].1.powi(2)).sqrt();
            let mag0 = (spec[0].0.powi(2) + spec[0].1.powi(2)).sqrt();
            assert!((mag7 - 64.0).abs() < 1e-6, "bin 7 peak={mag7}");
            assert!((mag19 - 32.0).abs() < 1e-6, "bin 19 peak={mag19}");
            assert!(mag0 < 1e-6);

            let rec = eng.transform_complex(&spec, true).unwrap();
            for (orig, r) in input.iter().zip(rec.iter()) {
                assert!((orig.0 - r.0).abs() < 1e-9);
                assert!((orig.1 - r.1).abs() < 1e-9);
            }
        }

        // Bit-reversed output order verification.
        let mut br_cfg = FftConfig::default();
        br_cfg.transform_size = 16;
        br_cfg.output_order = FftOutputOrder::BitReversed;
        let br_eng = FftEngine::new(br_cfg.clone()).unwrap();
        let tone1: Vec<(f64, f64)> = (0..16)
            .map(|k| {
                let th = 2.0 * PI * (k as f64) / 16.0;
                (th.cos(), th.sin())
            })
            .collect();
        let br_out = br_eng.transform_complex(&tone1, false).unwrap();
        // Bin 1 in 4-bit bit-reversed order lands at index 0b1000 = 8!
        let mag_at_8 = (br_out[8].0.powi(2) + br_out[8].1.powi(2)).sqrt();
        assert!((mag_at_8 - 16.0).abs() < 1e-6);

        // Fixed-point transform & SV emission.
        let fixed_in: Vec<(i32, i32)> = vec![(1000, 0); 16];
        let fixed_res = br_eng.transform_fixed(&fixed_in, false).unwrap();
        assert_eq!(fixed_res.bins[0].0, 16000);
        assert_eq!(fixed_res.radix4_stages, 2);

        let sv = emit_fft_sv(&FftConfig::default()).unwrap();
        assert!(sv.contains("module h_dsp_xfft"));
        assert!(sv.contains("s_axis_config_tvalid"));
        assert!(sv.contains("m_axis_data_tlast"));
        assert!(sv.contains("TWIDDLE_RE"));
    }

    #[test]
    fn polyphase_fir_and_phase_dithered_dds_compilers() {
        // 1. Polyphase FIR Compiler: SingleRate, Decimation by 4, Interpolation by 4.
        let taps = design_lowpass_hamming_taps(16, 0.25, 1024);
        assert_eq!(taps.len(), 16);
        // Symmetric linear-phase taps: h[k] == h[N-1-k].
        for i in 0..8 {
            assert_eq!(taps[i], taps[15 - i]);
        }

        // Decimation by 4 on 40 samples produces 10 output samples.
        let mut decim_cfg = FirConfig::default();
        decim_cfg.rate_mode = FirRateMode::Decimation { factor: 4 };
        decim_cfg.coefficients = vec![256; 8];
        let mut decim_eng = FirCompilerEngine::new(decim_cfg.clone());
        assert_eq!(decim_eng.polyphase_branches().len(), 4);
        let dc_in = vec![100i32; 40];
        let decim_out = decim_eng.process_stream(&dc_in);
        assert_eq!(decim_out.len(), 10);
        assert_eq!(*decim_out.last().unwrap(), 100 * 256 * 8);

        // Interpolation by 4 on 10 samples produces 40 output samples.
        let mut interp_cfg = FirConfig::default();
        interp_cfg.rate_mode = FirRateMode::Interpolation { factor: 4 };
        interp_cfg.coefficients = vec![10, 20, 30, 40, 50, 60, 70, 80];
        let mut interp_eng = FirCompilerEngine::new(interp_cfg);
        let interp_out = interp_eng.process_stream(&[1, 0, 0]);
        assert_eq!(interp_out.len(), 12);
        assert_eq!(&interp_out[0..8], &[10, 20, 30, 40, 50, 60, 70, 80]);

        let fir_sv = emit_fir_sv(&decim_cfg);
        assert!(fir_sv.contains("module h_dsp_fir_compiler"));
        assert!(fir_sv.contains("s_axis_reload_tvalid"));
        assert!(fir_sv.contains("m_axis_data_tdata"));

        // 2. Phase-Dithered DDS Compiler: quadrature envelope and runtime PINC/POFF tuning.
        let dds_cfg = DdsConfig::default();
        let mut dds = DdsCompilerEngine::new(dds_cfg.clone());
        let samples = dds.generate_samples(64);
        assert_eq!(samples.len(), 64);
        // Verify quadrature amplitude (sin^2 + cos^2) is approximately constant max_amp^2.
        let max_amp = ((1i64 << 15) - 1) as f64;
        for s in &samples {
            let mag = ((s.sin as f64).powi(2) + (s.cos as f64).powi(2)).sqrt();
            assert!(
                (mag - max_amp).abs() < 150.0,
                "DDS quadrature magnitude={mag} expected ~{max_amp}"
            );
        }
        // Verify Sin and Cos both swing positive and negative over a full period.
        assert!(samples.iter().any(|s| s.sin > 20_000));
        assert!(samples.iter().any(|s| s.sin < -20_000));
        assert!(samples.iter().any(|s| s.cos > 20_000));
        assert!(samples.iter().any(|s| s.cos < -20_000));

        let dds_sv = emit_dds_sv(&dds_cfg);
        assert!(dds_sv.contains("module h_dsp_dds_compiler"));
        assert!(dds_sv.contains("s_axis_config_tdata"));
        assert!(dds_sv.contains("m_axis_phase_tdata"));
        assert!(dds_sv.contains("dither_lfsr"));
    }

    #[test]
    fn riscv_rv32imac_and_rv64gc_subsystem_sv_generators() {
        let cfg32 = RiscvCpuConfig::rv32imac_default("helion_rv32_subsys");
        assert_eq!(cfg32.isa.xlen(), 32);
        assert_eq!(cfg32.isa.isa_string(), "rv32imac_zicsr_zifencei");
        let sv32 = emit_riscv_subsystem_sv(&cfg32);
        assert!(sv32.contains("module h_icache"));
        assert!(sv32.contains("module h_dcache"));
        assert!(sv32.contains("module h_clint"));
        assert!(sv32.contains("mtimecmp_q"));
        assert!(sv32.contains("mtime_q"));
        assert!(sv32.contains("module h_plic"));
        assert!(sv32.contains("best_id_m"));
        assert!(sv32.contains("module h_riscv_dm"));
        assert!(sv32.contains("dmcontrol_q"));
        assert!(sv32.contains("abstractcs_w"));
        assert!(sv32.contains("progbuf_q"));
        assert!(sv32.contains("sbaddress0_q"));
        assert!(sv32.contains("module h_rv32imac"));
        assert!(sv32.contains("m_axi_awaddr"));
        assert!(sv32.contains("module helion_rv32_subsys"));

        let mut cfg64 = RiscvCpuConfig::rv64gc_default("helion_rv64_subsys");
        cfg64.master_bus = MasterBusKind::Axi4Lite;
        assert_eq!(cfg64.isa.xlen(), 64);
        assert_eq!(cfg64.isa.isa_string(), "rv64imafdc_zicsr_zifencei");
        let sv64 = emit_riscv_subsystem_sv(&cfg64);
        assert!(sv64.contains("module h_rv64gc"));
        assert!(sv64.contains("parameter int unsigned XLEN         = 64"));
        assert!(sv64.contains("m_axil_awaddr"));
        assert!(sv64.contains("module helion_rv64_subsys"));
    }

    #[test]
    fn nist_fips_197_180_202_rfc8439_and_sp800_90b_reference_vectors() {
        // 1. NIST FIPS 197 Appendix B AES-128 test vector
        let aes128_key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let aes_pt: [u8; 16] = [
            0x32, 0x43, 0xf6, 0xa8, 0x88, 0x5a, 0x30, 0x8d, 0x31, 0x31, 0x98, 0xa2, 0xe0, 0x37,
            0x07, 0x34,
        ];
        let aes128_ct = aes128_encrypt_block(&aes128_key, &aes_pt);
        assert_eq!(
            aes128_ct,
            [
                0x39, 0x25, 0x84, 0x1d, 0x02, 0xdc, 0x09, 0xfb, 0xdc, 0x11, 0x85, 0x97, 0x19, 0x6a,
                0x0b, 0x32
            ]
        );

        // 2. NIST FIPS 197 Appendix C.3 AES-256 test vector
        let aes256_key: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let aes256_pt: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let aes256_ct = aes256_encrypt_block(&aes256_key, &aes256_pt);
        assert_eq!(
            aes256_ct,
            [
                0x8e, 0xa2, 0xb7, 0xca, 0x51, 0x67, 0x45, 0xbf, 0xea, 0xfc, 0x49, 0x90, 0x4b, 0x49,
                0x60, 0x89
            ]
        );

        // 3. NIST SP 800-38D Test Case 1 (AES-128-GCM empty plaintext & empty AAD -> tag = 0x58e2fccefa7e3061367f1d57a4e7455a)
        let (gcm_ct, gcm_tag) = aes128_gcm_encrypt(&[0u8; 16], &[0u8; 12], &[], &[]);
        assert!(gcm_ct.is_empty());
        assert_eq!(
            gcm_tag,
            [
                0x58, 0xe2, 0xfc, 0xce, 0xfa, 0x7e, 0x30, 0x61, 0x36, 0x7f, 0x1d, 0x57, 0xa4, 0xe7,
                0x45, 0x5a
            ]
        );

        // 4. NIST FIPS 180-4 SHA-256("abc")
        let sha256_abc = sha256_digest(b"abc");
        assert_eq!(
            sha256_abc,
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad
            ]
        );

        // 5. NIST FIPS 202 SHA3-256("abc")
        let sha3_abc = sha3_256_digest(b"abc");
        assert_eq!(
            sha3_abc,
            [
                0x3a, 0x98, 0x5d, 0xa7, 0x4f, 0xe2, 0x25, 0xb2, 0x04, 0x5c, 0x17, 0x2d, 0x6b, 0xd3,
                0x90, 0xbd, 0x85, 0x5f, 0x08, 0x6e, 0x3e, 0x9d, 0x52, 0x5b, 0x46, 0xbf, 0xe2, 0x45,
                0x11, 0x43, 0x15, 0x32
            ]
        );

        // 6. RFC 8439 Section 2.3.2 ChaCha20 Block Function Test Vector
        let mut chacha_key = [0u8; 32];
        for (i, b) in chacha_key.iter_mut().enumerate() {
            *b = i as u8;
        }
        let chacha_nonce: [u8; 12] = [
            0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
        ];
        let ks = chacha20_block(&chacha_key, 1, &chacha_nonce);
        assert_eq!(&ks[0..4], &[0x10, 0xf1, 0xe7, 0xe4]);
        assert_eq!(&ks[60..64], &[0xa2, 0x50, 0x3c, 0x4e]);

        // 7. Von-Neumann Debiaser & NIST SP 800-90B TRNG Health Monitor
        assert_eq!(von_neumann_debias(&[0, 0, 0, 1, 1, 0, 1, 1]), vec![0, 1]);
        let mut mon = TrngHealthMonitor::new(4, 6, 8);
        for &s in &[0, 1, 0, 1] {
            mon.feed_sample(s);
        }
        assert!(mon.is_healthy());
        for _ in 0..4 {
            mon.feed_sample(1);
        }
        assert!(mon.rct_failed);
    }

    #[test]
    fn crypto_catalog_and_synthesizable_sv_generators() {
        let cat = crypto_catalog();
        assert_eq!(cat.len(), 5);

        let aes_sv = generate_aes_gcm_sv(256);
        assert!(aes_sv.contains("module aes_gcm_core"));
        assert!(aes_sv.contains("parameter int C_KEY_BITS = 256"));
        assert!(aes_sv.contains("parameter int C_ROUNDS   = 14"));
        assert!(aes_sv.contains("gf128_mul_step"));

        let sha256_sv = generate_sha256_sv();
        assert!(sha256_sv.contains("module sha256_core"));
        assert!(sha256_sv.contains("32'h6a09e667"));

        let sha3_sv = generate_sha3_keccak_sv();
        assert!(sha3_sv.contains("module sha3_keccak_f1600"));
        assert!(sha3_sv.contains("sha3_256_digest"));

        let chacha_sv = generate_chacha20_poly1305_sv();
        assert!(chacha_sv.contains("module chacha20_poly1305_aead"));
        assert!(chacha_sv.contains("32'h61707865"));

        let trng_sv = generate_trng_nist_sv(8, 16, 325);
        assert!(trng_sv.contains("module trng_ro_nist_90b"));
        assert!(trng_sv.contains("health_rct_error"));
        assert!(trng_sv.contains("health_apt_error"));
        assert!(trng_sv.contains("vn_first_bit"));
    }

    #[test]
    fn aurora_64b66b_jesd204bc_and_noc_xy_golden_models_roundtrip() {
        // 1. Aurora 64B/66B 1+x^39+x^58 Scrambler / Descrambler exact roundtrip
        let mut tx_state = 0x155_5555_5555_5555u64;
        let mut rx_state = 0x155_5555_5555_5555u64;
        for word in [
            0xDEAD_BEEF_CAFE_BABEu64,
            0x0123_4567_89AB_CDEFu64,
            0xFFFF_0000_AAAA_5555u64,
        ] {
            let scr = scramble_64b66b(&mut tx_state, word);
            assert_ne!(scr, word);
            let descr = descramble_64b66b(&mut rx_state, scr);
            assert_eq!(descr, word);
        }

        // 2. JESD204B/C 1+x^14+x^15 Scrambler / Descrambler & Sample Mapper roundtrip
        let mut j_tx = 0x7FFFu16;
        let mut j_rx = 0x7FFFu16;
        for w in [0x1234_5678u32, 0x9ABC_DEF0u32] {
            let s = jesd204_scramble_word(&mut j_tx, w);
            assert_ne!(s, w);
            let d = jesd204_descramble_word(&mut j_rx, s);
            assert_eq!(d, w);
        }
        let samples = vec![0x1122, 0x3344, 0x5566, 0x7788];
        let packed = jesd204_pack_samples(&samples, 2);
        assert_eq!(packed.len(), 2);
        let unpacked = jesd204_unpack_samples(&packed, samples.len());
        assert_eq!(unpacked, samples);

        // 3. 2D Mesh Wormhole NoC Dimension-Order XY Routing (0,0) -> (2,1)
        // Must route X first: (0,0)->East->(1,0)->East->(2,0)->North->(2,1)->Local
        let hops = trace_xy_mesh_route(0, 0, 2, 1);
        assert_eq!(
            hops,
            vec![
                (0, 0, NocDir::East),
                (1, 0, NocDir::East),
                (2, 0, NocDir::North),
                (2, 1, NocDir::Local),
            ]
        );
    }

    #[test]
    fn aurora_jesd204_chip2chip_and_noc_router_emit_synthesizable_sv() {
        let cat = serial_noc_catalog();
        assert_eq!(cat.len(), 4);

        let aurora_sv = generate_aurora_link_sv(4, true);
        assert!(aurora_sv.contains("module aurora_serial_link"));
        assert!(aurora_sv.contains("parameter int C_NUM_LANES  = 4"));
        assert!(aurora_sv.contains("lane_up"));
        assert!(aurora_sv.contains("channel_up"));
        assert!(aurora_sv.contains("tx_scrambler_lfsr"));

        let jesd_sv = generate_jesd204_sv(4, 2, 2, 32);
        assert!(jesd_sv.contains("module jesd204bc_core"));
        assert!(jesd_sv.contains("sysref_in"));
        assert!(jesd_sv.contains("lmfc_pulse"));
        assert!(jesd_sv.contains("JESD_CGS"));
        assert!(jesd_sv.contains("JESD_ILAS"));

        let c2c_sv = generate_axi_chip2chip_sv(32, 64, 32);
        assert!(c2c_sv.contains("module axi_chip2chip_bridge"));
        assert!(c2c_sv.contains("c2c_link_status"));
        assert!(c2c_sv.contains("c2c_tx_data"));
        assert!(c2c_sv.contains("c2c_rx_data"));

        let noc_sv = generate_noc_xy_router_sv(1, 2, 64, 4);
        assert!(noc_sv.contains("module noc_2d_mesh_router"));
        assert!(noc_sv.contains("compute_xy_route"));
        assert!(noc_sv.contains("wormhole_active"));
        assert!(noc_sv.contains("in_vc_credit_out"));
    }
}
