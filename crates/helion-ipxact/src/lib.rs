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
