//! helion-prog — program via sim cable (M14). No board.
use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::prog_sim;

fn main() {
    let dev = Device::load_part("HL10T-C32-1").expect("HAD");
    let bits = Bitstream::empty(&dev);
    let st = prog_sim(&dev, &bits).expect("prog");
    println!(
        "helion-prog sim STAT INIT={} DONE={} EOS={} GWE={} GSR={} GTS={} CRC_ERR={}",
        st.init as u8,
        st.done as u8,
        st.eos as u8,
        st.gwe as u8,
        st.gsr as u8,
        st.gts as u8,
        st.crc_err as u8
    );
}
