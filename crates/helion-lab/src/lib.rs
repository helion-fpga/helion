//! Lab profile: program + STAT via sim cable. Must not depend on pack/place/route/map.

use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::hw_server_program;

pub fn lab_program_empty() -> Result<String, String> {
    let dev = Device::load_part("HL10T-C32-1")?;
    let st = hw_server_program(&dev, &Bitstream::empty(&dev))?;
    Ok(format!(
        "lab STAT INIT={} DONE={} GWE={}",
        st.init as u8, st.done as u8, st.gwe as u8
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_has_no_synth_dep_and_programs() {
        let s = lab_program_empty().unwrap();
        assert!(s.contains("DONE=1"));
    }
}
