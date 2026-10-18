//! System Monitor (SYSMON / XADC) Die Temperature & Voltage Rail Telemetry (UG480 / UG908 parity).
//!
//! Provides 12-bit XADC transfer functions, min/max/alarm tracking for die temperature
//! and `VCCINT` / `VCCAUX` / `VCCBRAM` voltage rails, a 16-bit DRP register bank,
//! and workload-driven telemetry sampling from a programmed [`Bitstream`].

use crate::ERR_CODE_EMPTY_BITSTREAM;
use helion_bits::Bitstream;
use helion_device::{Device, Far};
use helion_hw::refuse_empty_bitstream;
use std::collections::BTreeMap;

/// Standard XADC / SYSMON DRP register addresses (UG480).
pub mod drp_addr {
    pub const TEMPERATURE: u8 = 0x00;
    pub const VCCINT: u8 = 0x01;
    pub const VCCAUX: u8 = 0x02;
    pub const VCCBRAM: u8 = 0x06;
    pub const MAX_TEMP: u8 = 0x20;
    pub const MAX_VCCINT: u8 = 0x21;
    pub const MAX_VCCAUX: u8 = 0x22;
    pub const MAX_VCCBRAM: u8 = 0x23;
    pub const MIN_TEMP: u8 = 0x24;
    pub const MIN_VCCINT: u8 = 0x25;
    pub const MIN_VCCAUX: u8 = 0x26;
    pub const MIN_VCCBRAM: u8 = 0x27;
    pub const ALARM_FLAGS: u8 = 0x3F;
    pub const CONFIG_REG0: u8 = 0x40;
    pub const CONFIG_REG1: u8 = 0x41;
    pub const ALARM_TEMP_UPPER: u8 = 0x50;
    pub const ALARM_OT_UPPER: u8 = 0x53;
}

/// Convert a 12-bit XADC temperature code (`0..=4095`) to degrees Celsius (UG480 Eq. 2-6):
/// $$T(^\circ\text{C}) = \frac{\text{ADC} \times 503.975}{4096} - 273.15$$
pub fn adc_to_temp_c(adc_code: u16) -> f64 {
    let clamped = (adc_code & 0x0FFF) as f64;
    (clamped * 503.975) / 4096.0 - 273.15
}

/// Convert degrees Celsius to a 12-bit XADC temperature code (`0..=4095`).
pub fn temp_c_to_adc(temp_c: f64) -> u16 {
    let raw = ((temp_c + 273.15) * 4096.0 / 503.975).round();
    raw.clamp(0.0, 4095.0) as u16
}

/// Convert a 12-bit XADC supply sensor code (`0..=4095`) to Volts (UG480 Eq. 2-7, 3.0 V full scale):
/// $$V = \frac{\text{ADC} \times 3.0}{4096}$$
pub fn adc_to_voltage(adc_code: u16) -> f64 {
    let clamped = (adc_code & 0x0FFF) as f64;
    (clamped * 3.0) / 4096.0
}

/// Convert Volts to a 12-bit XADC supply sensor code (`0..=4095`).
pub fn voltage_to_adc(volts: f64) -> u16 {
    let raw = (volts * 4096.0 / 3.0).round();
    raw.clamp(0.0, 4095.0) as u16
}

/// Telemetry reading and min/max/alarm state for a single voltage rail (`VCCINT`, `VCCAUX`, `VCCBRAM`).
#[derive(Clone, Debug, PartialEq)]
pub struct SysmonRailReading {
    pub name: &'static str,
    pub nominal_v: f64,
    pub current_v: f64,
    pub min_v: f64,
    pub max_v: f64,
    pub adc_code: u16,
    pub alarm_low_v: f64,
    pub alarm_high_v: f64,
    pub alarm_active: bool,
}

impl SysmonRailReading {
    pub fn new(
        name: &'static str,
        nominal_v: f64,
        alarm_low_v: f64,
        alarm_high_v: f64,
    ) -> Self {
        let adc = voltage_to_adc(nominal_v);
        let quant_v = adc_to_voltage(adc);
        Self {
            name,
            nominal_v,
            current_v: quant_v,
            min_v: quant_v,
            max_v: quant_v,
            adc_code: adc,
            alarm_low_v,
            alarm_high_v,
            alarm_active: false,
        }
    }

    pub fn update(&mut self, volts: f64) {
        let adc = voltage_to_adc(volts);
        let v = adc_to_voltage(adc);
        self.adc_code = adc;
        self.current_v = v;
        if v < self.min_v {
            self.min_v = v;
        }
        if v > self.max_v {
            self.max_v = v;
        }
        self.alarm_active = v < self.alarm_low_v || v > self.alarm_high_v;
    }
}

/// Active SYSMON / XADC alarm flags (`0x3F` Flag Register).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SysmonAlarms {
    pub user_temp_alarm: bool,
    pub over_temp_shutdown: bool,
    pub vccint_alarm: bool,
    pub vccaux_alarm: bool,
    pub vccbram_alarm: bool,
}

impl SysmonAlarms {
    pub fn any(&self) -> bool {
        self.user_temp_alarm
            || self.over_temp_shutdown
            || self.vccint_alarm
            || self.vccaux_alarm
            || self.vccbram_alarm
    }

    pub fn flag_word(&self) -> u16 {
        (u16::from(self.user_temp_alarm))
            | (u16::from(self.vccint_alarm) << 1)
            | (u16::from(self.vccaux_alarm) << 2)
            | (u16::from(self.over_temp_shutdown) << 3)
            | (u16::from(self.vccbram_alarm) << 4)
    }
}

/// Snapshot of SYSMON / XADC die temperature and voltage rail telemetry.
#[derive(Clone, Debug, PartialEq)]
pub struct SysmonTelemetry {
    pub die_temp_c: f64,
    pub min_temp_c: f64,
    pub max_temp_c: f64,
    pub temp_adc_code: u16,
    pub user_temp_limit_c: f64,
    pub ot_shutdown_limit_c: f64,
    pub vccint: SysmonRailReading,
    pub vccaux: SysmonRailReading,
    pub vccbram: SysmonRailReading,
    pub alarms: SysmonAlarms,
    pub sample_count: usize,
}

impl SysmonTelemetry {
    pub fn summary_line(&self) -> String {
        format!(
            "SYSMON TEMP={:.2}C (min={:.2}C max={:.2}C adc={:#05x}) VCCINT={:.3}V VCCAUX={:.3}V VCCBRAM={:.3}V ALARMS={:#04x}",
            self.die_temp_c,
            self.min_temp_c,
            self.max_temp_c,
            self.temp_adc_code,
            self.vccint.current_v,
            self.vccaux.current_v,
            self.vccbram.current_v,
            self.alarms.flag_word()
        )
    }
}

/// System Monitor (SYSMON / XADC) controller with 16-bit DRP register file and alarm comparator bank.
#[derive(Clone, Debug)]
pub struct SysmonMonitor {
    pub telemetry: SysmonTelemetry,
    drp_regs: BTreeMap<u8, u16>,
}

impl Default for SysmonMonitor {
    fn default() -> Self {
        Self::new(25.0)
    }
}

impl SysmonMonitor {
    /// Initialize SYSMON at `ambient_c` (°C) with nominal rails (`VCCINT=1.0V`, `VCCAUX=1.8V`, `VCCBRAM=1.0V`).
    pub fn new(ambient_c: f64) -> Self {
        let temp_adc = temp_c_to_adc(ambient_c);
        let quant_temp = adc_to_temp_c(temp_adc);
        let user_limit = 85.0;
        let ot_limit = 125.0;
        let vccint = SysmonRailReading::new("VCCINT", 1.00, 0.97, 1.03);
        let vccaux = SysmonRailReading::new("VCCAUX", 1.80, 1.71, 1.89);
        let vccbram = SysmonRailReading::new("VCCBRAM", 1.00, 0.95, 1.05);

        let mut mon = Self {
            telemetry: SysmonTelemetry {
                die_temp_c: quant_temp,
                min_temp_c: quant_temp,
                max_temp_c: quant_temp,
                temp_adc_code: temp_adc,
                user_temp_limit_c: user_limit,
                ot_shutdown_limit_c: ot_limit,
                vccint,
                vccaux,
                vccbram,
                alarms: SysmonAlarms::default(),
                sample_count: 0,
            },
            drp_regs: BTreeMap::new(),
        };
        mon.write_drp(drp_addr::ALARM_TEMP_UPPER, temp_c_to_adc(user_limit) << 4);
        mon.write_drp(drp_addr::ALARM_OT_UPPER, temp_c_to_adc(ot_limit) << 4);
        mon.sync_drp_from_telemetry();
        mon
    }

    /// Update sensor measurements (`die_temp_c`, `vccint_v`, `vccaux_v`, `vccbram_v`)
    /// and evaluate all UG480 alarm thresholds and min/max tracking registers.
    pub fn ingest_sample(
        &mut self,
        die_temp_c: f64,
        vccint_v: f64,
        vccaux_v: f64,
        vccbram_v: f64,
    ) -> &SysmonTelemetry {
        let temp_adc = temp_c_to_adc(die_temp_c);
        let t = adc_to_temp_c(temp_adc);
        self.telemetry.temp_adc_code = temp_adc;
        self.telemetry.die_temp_c = t;
        if t < self.telemetry.min_temp_c {
            self.telemetry.min_temp_c = t;
        }
        if t > self.telemetry.max_temp_c {
            self.telemetry.max_temp_c = t;
        }

        self.telemetry.vccint.update(vccint_v);
        self.telemetry.vccaux.update(vccaux_v);
        self.telemetry.vccbram.update(vccbram_v);

        self.telemetry.alarms = SysmonAlarms {
            user_temp_alarm: t >= self.telemetry.user_temp_limit_c,
            over_temp_shutdown: t >= self.telemetry.ot_shutdown_limit_c,
            vccint_alarm: self.telemetry.vccint.alarm_active,
            vccaux_alarm: self.telemetry.vccaux.alarm_active,
            vccbram_alarm: self.telemetry.vccbram.alarm_active,
        };
        self.telemetry.sample_count += 1;
        self.sync_drp_from_telemetry();
        &self.telemetry
    }

    /// Compute die thermal rise and supply IR droop from a programmed [`Bitstream`]'s resource
    /// utilization and `toggle_rate` (`0.0..=1.0`). Refuses empty bitstreams with
    /// [`ERR_CODE_EMPTY_BITSTREAM`].
    pub fn sample_bitstream(
        &mut self,
        _dev: &Device,
        bits: &Bitstream,
        ambient_c: f64,
        toggle_rate: f64,
    ) -> Result<SysmonTelemetry, String> {
        refuse_empty_bitstream(bits).map_err(|e| {
            if e.contains(ERR_CODE_EMPTY_BITSTREAM) {
                format!("sysmon: {e}")
            } else {
                format!("{ERR_CODE_EMPTY_BITSTREAM}: sysmon: {e}")
            }
        })?;

        let mut clb_frames = 0usize;
        let mut bram_frames = 0usize;
        let mut dsp_frames = 0usize;
        let mut iob_frames = 0usize;
        for ((block, _, _), &word) in &bits.frames {
            if word == 0 {
                continue;
            }
            match *block {
                Far::CLB_IO_CLK => clb_frames += 1,
                Far::BRAM => bram_frames += 1,
                Far::DSP => dsp_frames += 1,
                Far::IOB => iob_frames += 1,
                _ => {}
            }
        }

        let activity = toggle_rate.clamp(0.0, 2.0);
        // Dynamic power model (Watts) -> junction-to-ambient thermal resistance (12.5 C/W).
        let p_static_w = 0.35;
        let p_dyn_w = (clb_frames as f64 * 0.012
            + bram_frames as f64 * 0.025
            + dsp_frames as f64 * 0.030
            + iob_frames as f64 * 0.015)
            * (0.25 + 0.75 * activity);
        let total_power_w = p_static_w + p_dyn_w;
        let junction_temp_c = ambient_c + total_power_w * 12.5;

        // IR drop on rails proportional to dynamic current draw.
        let vccint_v = (1.00 - (clb_frames + dsp_frames) as f64 * 0.0008 * (0.5 + activity))
            .clamp(0.85, 1.05);
        let vccaux_v = (1.80 - iob_frames as f64 * 0.0012 * (0.5 + activity)).clamp(1.65, 1.90);
        let vccbram_v =
            (1.00 - bram_frames as f64 * 0.0015 * (0.5 + activity)).clamp(0.85, 1.05);

        Ok(self
            .ingest_sample(junction_temp_c, vccint_v, vccaux_v, vccbram_v)
            .clone())
    }

    fn sync_drp_from_telemetry(&mut self) {
        let t = &self.telemetry;
        // UG480: 12-bit ADC value is MSB-aligned in bits [15:4] of the 16-bit DRP status registers.
        self.drp_regs
            .insert(drp_addr::TEMPERATURE, t.temp_adc_code << 4);
        self.drp_regs
            .insert(drp_addr::VCCINT, t.vccint.adc_code << 4);
        self.drp_regs
            .insert(drp_addr::VCCAUX, t.vccaux.adc_code << 4);
        self.drp_regs
            .insert(drp_addr::VCCBRAM, t.vccbram.adc_code << 4);

        self.drp_regs
            .insert(drp_addr::MAX_TEMP, temp_c_to_adc(t.max_temp_c) << 4);
        self.drp_regs.insert(
            drp_addr::MAX_VCCINT,
            voltage_to_adc(t.vccint.max_v) << 4,
        );
        self.drp_regs.insert(
            drp_addr::MAX_VCCAUX,
            voltage_to_adc(t.vccaux.max_v) << 4,
        );
        self.drp_regs.insert(
            drp_addr::MAX_VCCBRAM,
            voltage_to_adc(t.vccbram.max_v) << 4,
        );

        self.drp_regs
            .insert(drp_addr::MIN_TEMP, temp_c_to_adc(t.min_temp_c) << 4);
        self.drp_regs.insert(
            drp_addr::MIN_VCCINT,
            voltage_to_adc(t.vccint.min_v) << 4,
        );
        self.drp_regs.insert(
            drp_addr::MIN_VCCAUX,
            voltage_to_adc(t.vccaux.min_v) << 4,
        );
        self.drp_regs.insert(
            drp_addr::MIN_VCCBRAM,
            voltage_to_adc(t.vccbram.min_v) << 4,
        );

        self.drp_regs
            .insert(drp_addr::ALARM_FLAGS, t.alarms.flag_word());
    }

    /// Read a 16-bit XADC DRP register (`addr` in `0x00..=0x7F`).
    pub fn read_drp(&self, addr: u8) -> u16 {
        self.drp_regs.get(&addr).copied().unwrap_or(0)
    }

    /// Write a 16-bit XADC DRP configuration/alarm register (`addr` in `0x40..=0x7F`).
    pub fn write_drp(&mut self, addr: u8, val: u16) {
        self.drp_regs.insert(addr, val);
        if addr == drp_addr::ALARM_TEMP_UPPER {
            self.telemetry.user_temp_limit_c = adc_to_temp_c(val >> 4);
        } else if addr == drp_addr::ALARM_OT_UPPER {
            self.telemetry.ot_shutdown_limit_c = adc_to_temp_c(val >> 4);
        }
    }
}
