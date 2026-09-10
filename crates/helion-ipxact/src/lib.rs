//! IEEE 1685-style pack/reimport (minimal XML). Helion-MM/ST, not AXI.

use std::path::Path;

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

/// UG893 IP Catalog contents: packed Helion-MM/ST cores from helion-ipxact.
pub fn catalog() -> Vec<IpCore> {
    vec![
        pack_uart(),
        pack_gpio(),
        pack_rv32(),
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
    ]
}

impl IpCore {
    /// IEEE 1685 VLNV (`vendor:library:name:version`).
    pub fn vlnv(&self) -> String {
        format!("{}:{}:{}:{}", self.vendor, self.library, self.name, self.version)
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
        assert!(cat.iter().any(|c| c.name == "h_sync_fifo"));
        assert!(cat.iter().any(|c| c.name == "h_timer"));
        assert!(cat.iter().any(|c| c.name == "h_pwm"));
        assert!(cat.iter().any(|c| c.name == "h_spi_mm"));
        assert!(cat.iter().any(|c| c.name == "h_debounce"));
        assert!(cat.iter().any(|c| c.name == "h_clkdiv"));
        assert!(cat.iter().all(|c| c.bus != "AXI"));
        assert!(cat.iter().all(|c| !c.bus.to_ascii_lowercase().contains("axi")));
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
        assert_eq!(pack_scratch_mm().vlnv(), "community:helion:h_scratch_mm:1.0");
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
        assert_eq!(pack_compare_mm().vlnv(), "community:helion:h_compare_mm:1.0");
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
        assert_eq!(pack_rng_xorshift().vlnv(), "community:helion:h_rng_xorshift:1.0");
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
        assert_eq!(pack_mailbox_mm().vlnv(), "community:helion:h_mailbox_mm:1.0");
        assert_eq!(pack_pwm_deadtime().vlnv(), "community:helion:h_pwm_deadtime:1.0");
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
        assert_eq!(pack_div_restoring().vlnv(), "community:helion:h_div_restoring:1.0");
        assert_eq!(pack_sqrt_digit().vlnv(), "community:helion:h_sqrt_digit:1.0");
        assert_eq!(pack_cordic_step().vlnv(), "community:helion:h_cordic_step:1.0");
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
        assert_eq!(pack_phase_accum().vlnv(), "community:helion:h_phase_accum:1.0");
        assert_eq!(pack_fir_tap().vlnv(), "community:helion:h_fir_tap:1.0");
        assert_eq!(pack_iir_biquad().vlnv(), "community:helion:h_iir_biquad:1.0");
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
        assert_eq!(pack_pwm_center().vlnv(), "community:helion:h_pwm_center:1.0");
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
        assert_eq!(pack_manchester_enc().vlnv(), "community:helion:h_manchester_enc:1.0");
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
        assert_eq!(pack_traffic_light().vlnv(), "community:helion:h_traffic_light:1.0");
        assert_eq!(pack_scoreboard().vlnv(), "community:helion:h_scoreboard:1.0");
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
                pkg.format = v
                    .parse()
                    .map_err(|_| format!("format: bad version {v}"))?;
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
                    return Err(format!("vlnv: expected vendor:library:name:version, got {v}"));
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
        assert!(parse_helion(
            r#"
format 1
name bad
bus AXI
file a.v
"#
        )
        .unwrap_err()
        .contains("not AXI"));
    }

    #[test]
    fn catalog_cores_roundtrip_helion_text() {
        for ip in catalog() {
            let body = to_helion_manifest(&ip, Some(&ip.name), &[&format!("{}.v", ip.name)]);
            let pkg = parse_helion(&body).unwrap();
            assert_eq!(pkg.to_ip_core().unwrap(), ip);
        }
    }
}
