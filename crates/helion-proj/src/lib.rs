//! Dual-mode Session, checkpoints `.hckp`, object query.

use helion_bits::{bitgen, Bitstream};
use helion_device::Device;
use helion_ir::Design;
use helion_pack::pack;
use helion_place::place;
use helion_route::route;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Project,
    NonProject,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub mode: Mode,
    pub design: Option<Design>,
    pub bitstream: Option<Bitstream>,
}

impl Session {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            design: None,
            bitstream: None,
        }
    }

    pub fn impl_design(&mut self, d: Design, dev: &Device) -> Result<(), String> {
        let packed = pack(&d, dev)?;
        let placed = place(&packed, dev)?;
        let routed = route(&placed, dev)?;
        let bits = bitgen(dev, &routed)?;
        self.design = Some(d);
        self.bitstream = Some(bits);
        Ok(())
    }

    pub fn blinky_hash(&self) -> Option<u32> {
        self.bitstream.as_ref().map(|b| {
            let mut h = b.idcode;
            for ((bl, maj, min), w) in &b.frames {
                h ^= helion_bits::crc32c(&[*bl, *min])
                    ^ (*maj as u32)
                    ^ (*w as u32)
                    ^ ((*w >> 32) as u32);
            }
            h
        })
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"HCKP");
        v.push(match self.mode {
            Mode::Project => 1,
            Mode::NonProject => 2,
        });
        if let Some(h) = self.blinky_hash() {
            v.extend_from_slice(&h.to_le_bytes());
        }
        v
    }

    pub fn restore(bytes: &[u8]) -> Result<(Mode, u32), String> {
        if bytes.len() < 9 || &bytes[0..4] != b"HCKP" {
            return Err("bad hckp".into());
        }
        let mode = match bytes[4] {
            1 => Mode::Project,
            2 => Mode::NonProject,
            _ => return Err("bad mode".into()),
        };
        let hash = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
        Ok((mode, hash))
    }
}

pub fn get_cells(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.cells
        .iter()
        .filter(|c| filter.map(|f| c.name.contains(f)).unwrap_or(true))
        .map(|c| c.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::Design;

    #[test]
    fn dual_mode_same_hash_and_ckpt() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut proj = Session::new(Mode::Project);
        let mut np = Session::new(Mode::NonProject);
        proj.impl_design(Design::structural_blinky(), &dev).unwrap();
        np.impl_design(Design::structural_blinky(), &dev).unwrap();
        assert_eq!(proj.blinky_hash(), np.blinky_hash());
        let ck = proj.checkpoint();
        let (mode, h) = Session::restore(&ck).unwrap();
        assert_eq!(mode, Mode::Project);
        assert_eq!(Some(h), proj.blinky_hash());
        let cells = get_cells(proj.design.as_ref().unwrap(), Some("lut"));
        assert_eq!(cells, vec!["u_lut"]);
    }
}
