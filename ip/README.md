# Helion IP packages (`.helion`)

Each catalog core ships a **format 1** `.helion` text manifest next to its HDL:

```
format 1
vlnv community:helion:h_gpio:1.0
bus Helion-MM
top h_gpio
file h_gpio.v
```

Bus must be **Helion-MM** or **Helion-ST** (never AXI as a Helion product).

## Catalog

| Core | Bus | Notes |
|------|-----|-------|
| `h_gpio` | Helion-MM | Registered data + direction MM GPIO |
| `h_uart` | Helion-MM | Registered TX shifter + busy/div |
| `h_rv32_hb1` | Helion-MM | PicoRV32 wrap (not Zynq / not AXI) |
| `h_sync_fifo` | Helion-ST | Depth-8 / width-8 sync FIFO (wr/rd ptrs) |
| `h_timer` | Helion-MM | Loadable down-counter + enable/zero |
| `h_pwm` | Helion-MM | Period/duty regs, counter, output compare |
| `h_spi_mm` | Helion-MM | SPI master bit-engine (clk-div/shift/busy) |
| `h_debounce` | Helion-ST | N-stage shift-reg debounce, stable out |
| `h_clkdiv` | Helion-ST | Programmable clock divider (loadable div/en/out) |
| `h_edge_det` | Helion-ST | Rising/falling edge detect, sticky status + clear |
| `h_watchdog` | Helion-MM | Loadable countdown watchdog, enable/timeout/pet |
| `h_crc32` | Helion-ST | Byte-serial CRC32 (init/enable/data_valid) |
| `h_lfsr` | Helion-ST | Programmable-tap LFSR PRBS (enable/load/seed) |
| `h_gray_cnt` | Helion-ST | Binary↔gray counter with enable/load |
| `h_scratch_mm` | Helion-MM | 4×32b MM-mapped scratch regfile (wr/rd strobes) |
| `h_popcount` | Helion-ST | Registered 32b population-count |
| `h_saturate` | Helion-ST | Saturating add/sub + sticky signed OV flags |
| `h_shift_reg` | Helion-ST | Loadable 32b shift reg (dir/serial in/out) |
| `h_compare_mm` | Helion-MM | Threshold/compare regs + sticky match/irq |
| `h_clz` | Helion-ST | Registered 32b count-leading-zeros |
| `h_byte_rev` | Helion-ST | Registered endian byte-reverse (32b↔bytes) |
| `h_arb_rr` | Helion-ST | 4-request round-robin arbiter, grant + sticky mask |
| `h_ctz` | Helion-ST | Registered 32b count-trailing-zeros (pair to h_clz) |
| `h_minmax` | Helion-ST | Registered min/max of two 32b + mode |
| `h_accum_mm` | Helion-MM | Loadable accumulator + clear/add |
| `h_parity` | Helion-ST | Registered even/odd parity + sticky error |
| `h_hamming` | Helion-ST | Registered Hamming distance of two 32b |
| `h_rot_mm` | Helion-MM | MM-programmed rotate left/right by N |
| `h_mask_gen` | Helion-ST | Registered bitmask from width/offset |
| `h_absdiff` | Helion-ST | Registered absolute difference |a-b| on 32b |
| `h_clamp` | Helion-ST | Registered clamp of 32b value to [lo,hi] |
| `h_bin2oh` | Helion-ST | Registered binary→onehot (4b→16b) |
| `h_timer_cap` | Helion-MM | Free-run capture timer + sticky edge capture |
| `h_mux4` | Helion-ST | Registered 4:1 mux 32b + sel |
| `h_addsub_mm` | Helion-MM | MM add/sub with sticky carry/borrow |
| `h_ones_comp` | Helion-ST | Registered ones-complement / invert+inc negate |
| `h_pulse_ext` | Helion-ST | Pulse stretcher/extender with programmable width |
| `h_rng_xorshift` | Helion-ST | Registered xorshift32 PRNG with seed/enable |
| `h_prio_enc` | Helion-ST | Registered 16→4 priority encoder + valid |
| `h_skid_buf` | Helion-ST | 1-deep skid buffer (valid/ready Helion-ST) |
| `h_bcd_inc` | Helion-ST | Registered 4-digit BCD incrementer + carry |
| `h_mailbox_mm` | Helion-MM | Single-slot MM mailbox (write, sticky full/empty, read clears) |
| `h_pwm_deadtime` | Helion-ST | Complementary PWM with programmable deadtime counters |
| `h_lfsr16` | Helion-ST | 16-bit LFSR PRBS (distinct from 8b h_lfsr) |
| `h_edge_cnt` | Helion-ST | Rising-edge counter with clear |
| `h_uart_rx` | Helion-MM | UART RX bit sampler (baud div, start/data/stop, sticky byte) |
| `h_div_restoring` | Helion-ST | Restoring divider step engine (registered quot/rem) |
| `h_sqrt_digit` | Helion-ST | Digit-by-digit integer sqrt step (registered) |
| `h_cordic_step` | Helion-ST | One CORDIC rotation step (registered x/y/z) |
| `h_phase_accum` | Helion-ST | NCO phase accumulator (freq word + MSB carrier) |
| `h_fir_tap` | Helion-ST | Single MAC FIR tap (registered acc += x*coeff) |
| `h_iir_biquad` | Helion-ST | DF1 biquad step (registered, small coeffs) |
| `h_cdc_pulse` | Helion-ST | 2FF toggle-pulse CDC synchronizer (Helion-native) |

## Use

```bash
helion ip list
helion ip show ip/h_gpio/h_gpio.helion
helion ip show ip/h_timer/h_timer.helion
helion ip show ip/h_pwm/h_pwm.helion
helion ip show ip/h_debounce/h_debounce.helion
helion ip show ip/h_clkdiv/h_clkdiv.helion
helion ip show ip/h_edge_det/h_edge_det.helion
helion ip show ip/h_watchdog/h_watchdog.helion
helion ip show ip/h_crc32/h_crc32.helion
helion ip show ip/h_lfsr/h_lfsr.helion
helion ip show ip/h_gray_cnt/h_gray_cnt.helion
helion ip show ip/h_scratch_mm/h_scratch_mm.helion
helion ip show ip/h_popcount/h_popcount.helion
helion ip show ip/h_saturate/h_saturate.helion
helion ip show ip/h_shift_reg/h_shift_reg.helion
helion ip show ip/h_compare_mm/h_compare_mm.helion
helion ip show ip/h_clz/h_clz.helion
helion ip show ip/h_byte_rev/h_byte_rev.helion
helion ip show ip/h_arb_rr/h_arb_rr.helion
helion ip show ip/h_ctz/h_ctz.helion
helion ip show ip/h_minmax/h_minmax.helion
helion ip show ip/h_accum_mm/h_accum_mm.helion
helion ip show ip/h_parity/h_parity.helion
helion ip show ip/h_hamming/h_hamming.helion
helion ip show ip/h_rot_mm/h_rot_mm.helion
helion ip show ip/h_mask_gen/h_mask_gen.helion
helion ip show ip/h_absdiff/h_absdiff.helion
helion ip show ip/h_clamp/h_clamp.helion
helion ip show ip/h_bin2oh/h_bin2oh.helion
helion ip show ip/h_timer_cap/h_timer_cap.helion
helion ip show ip/h_mux4/h_mux4.helion
helion ip show ip/h_addsub_mm/h_addsub_mm.helion
helion ip show ip/h_ones_comp/h_ones_comp.helion
helion ip show ip/h_pulse_ext/h_pulse_ext.helion
helion ip show ip/h_rng_xorshift/h_rng_xorshift.helion
helion ip show ip/h_prio_enc/h_prio_enc.helion
helion ip show ip/h_skid_buf/h_skid_buf.helion
helion ip show ip/h_bcd_inc/h_bcd_inc.helion
helion ip show ip/h_mailbox_mm/h_mailbox_mm.helion
helion ip show ip/h_pwm_deadtime/h_pwm_deadtime.helion
helion ip show ip/h_lfsr16/h_lfsr16.helion
helion ip show ip/h_edge_cnt/h_edge_cnt.helion
helion ip show ip/h_uart_rx/h_uart_rx.helion
helion ip show ip/h_div_restoring/h_div_restoring.helion
helion ip show ip/h_sqrt_digit/h_sqrt_digit.helion
helion ip show ip/h_cordic_step/h_cordic_step.helion
helion ip show ip/h_phase_accum/h_phase_accum.helion
helion ip show ip/h_fir_tap/h_fir_tap.helion
helion ip show ip/h_iir_biquad/h_iir_biquad.helion
helion ip show ip/h_cdc_pulse/h_cdc_pulse.helion
helion project examples/ip_ingest/counter_ip.prj
helion project examples/ip_ingest/h_timer_ip.prj
helion project examples/ip_ingest/h_pwm_ip.prj
helion project examples/ip_ingest/h_debounce_ip.prj
helion project examples/ip_ingest/h_clkdiv_ip.prj
helion project examples/ip_ingest/h_edge_det_ip.prj
helion project examples/ip_ingest/h_watchdog_ip.prj
helion project examples/ip_ingest/h_crc32_ip.prj
helion project examples/ip_ingest/h_lfsr_ip.prj
helion project examples/ip_ingest/h_gray_cnt_ip.prj
helion project examples/ip_ingest/h_scratch_mm_ip.prj
helion project examples/ip_ingest/h_popcount_ip.prj
helion project examples/ip_ingest/h_saturate_ip.prj
helion project examples/ip_ingest/h_shift_reg_ip.prj
helion project examples/ip_ingest/h_compare_mm_ip.prj
helion project examples/ip_ingest/h_clz_ip.prj
helion project examples/ip_ingest/h_byte_rev_ip.prj
helion project examples/ip_ingest/h_arb_rr_ip.prj
helion project examples/ip_ingest/h_ctz_ip.prj
helion project examples/ip_ingest/h_minmax_ip.prj
helion project examples/ip_ingest/h_accum_mm_ip.prj
helion project examples/ip_ingest/h_parity_ip.prj
helion project examples/ip_ingest/h_hamming_ip.prj
helion project examples/ip_ingest/h_rot_mm_ip.prj
helion project examples/ip_ingest/h_mask_gen_ip.prj
helion project examples/ip_ingest/h_absdiff_ip.prj
helion project examples/ip_ingest/h_clamp_ip.prj
helion project examples/ip_ingest/h_bin2oh_ip.prj
helion project examples/ip_ingest/h_timer_cap_ip.prj
helion project examples/ip_ingest/h_mux4_ip.prj
helion project examples/ip_ingest/h_addsub_mm_ip.prj
helion project examples/ip_ingest/h_ones_comp_ip.prj
helion project examples/ip_ingest/h_pulse_ext_ip.prj
helion project examples/ip_ingest/h_rng_xorshift_ip.prj
helion project examples/ip_ingest/h_prio_enc_ip.prj
helion project examples/ip_ingest/h_skid_buf_ip.prj
helion project examples/ip_ingest/h_bcd_inc_ip.prj
helion project examples/ip_ingest/h_mailbox_mm_ip.prj
helion project examples/ip_ingest/h_pwm_deadtime_ip.prj
helion project examples/ip_ingest/h_lfsr16_ip.prj
helion project examples/ip_ingest/h_edge_cnt_ip.prj
helion project examples/ip_ingest/h_uart_rx_ip.prj
helion project examples/ip_ingest/h_div_restoring_ip.prj
helion project examples/ip_ingest/h_sqrt_digit_ip.prj
helion project examples/ip_ingest/h_cordic_step_ip.prj
helion project examples/ip_ingest/h_phase_accum_ip.prj
helion project examples/ip_ingest/h_fir_tap_ip.prj
helion project examples/ip_ingest/h_iir_biquad_ip.prj
helion project examples/ip_ingest/h_cdc_pulse_ip.prj
# in a .prj:
#   read_ip ip/h_crc32/h_crc32.helion
#   read_ip ip/h_gray_cnt/h_gray_cnt.helion
#   read_ip ip/h_scratch_mm/h_scratch_mm.helion
#   read_ip ip/h_popcount/h_popcount.helion
#   read_ip ip/h_saturate/h_saturate.helion
#   read_ip ip/h_shift_reg/h_shift_reg.helion
#   read_ip ip/h_compare_mm/h_compare_mm.helion
#   read_ip ip/h_clz/h_clz.helion
#   read_ip ip/h_byte_rev/h_byte_rev.helion
#   read_ip ip/h_arb_rr/h_arb_rr.helion
#   read_ip ip/h_ctz/h_ctz.helion
#   read_ip ip/h_minmax/h_minmax.helion
#   read_ip ip/h_accum_mm/h_accum_mm.helion
#   read_ip ip/h_parity/h_parity.helion
#   read_ip ip/h_hamming/h_hamming.helion
#   read_ip ip/h_rot_mm/h_rot_mm.helion
#   read_ip ip/h_mask_gen/h_mask_gen.helion
#   read_ip ip/h_absdiff/h_absdiff.helion
#   read_ip ip/h_clamp/h_clamp.helion
#   read_ip ip/h_bin2oh/h_bin2oh.helion
#   read_ip ip/h_timer_cap/h_timer_cap.helion
#   read_ip ip/h_mux4/h_mux4.helion
#   read_ip ip/h_addsub_mm/h_addsub_mm.helion
#   read_ip ip/h_ones_comp/h_ones_comp.helion
#   read_ip ip/h_pulse_ext/h_pulse_ext.helion
#   read_ip ip/h_rng_xorshift/h_rng_xorshift.helion
#   read_ip ip/h_prio_enc/h_prio_enc.helion
#   read_ip ip/h_skid_buf/h_skid_buf.helion
#   read_ip ip/h_bcd_inc/h_bcd_inc.helion
#   read_ip ip/h_mailbox_mm/h_mailbox_mm.helion
#   read_ip ip/h_pwm_deadtime/h_pwm_deadtime.helion
#   read_ip ip/h_lfsr16/h_lfsr16.helion
#   read_ip ip/h_edge_cnt/h_edge_cnt.helion
#   read_ip ip/h_uart_rx/h_uart_rx.helion
#   read_ip ip/h_div_restoring/h_div_restoring.helion
#   read_ip ip/h_sqrt_digit/h_sqrt_digit.helion
#   read_ip ip/h_cordic_step/h_cordic_step.helion
#   read_ip ip/h_phase_accum/h_phase_accum.helion
#   read_ip ip/h_fir_tap/h_fir_tap.helion
#   read_ip ip/h_iir_biquad/h_iir_biquad.helion
#   read_ip ip/h_cdc_pulse/h_cdc_pulse.helion
```

`read_ip` expands package `file` / `xdc` entries into the project source and constraint lists before synth.
