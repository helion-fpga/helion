//! ILA: mark net → extra LUTFF in netlist/bitstream → arm → capture.

use helion_bits::bitgen;
use helion_device::Device;
use helion_fabric::Fabric;
use helion_ir::{CellKind, Design};
use helion_pack::pack;
use helion_place::place;
use helion_route::route;

#[derive(Clone, Debug)]
pub struct IlaCapture {
    pub net: String,
    pub samples: Vec<bool>,
}

/// Insert an ILA probe on `net`: identity LUT+FF so the bitstream gains BLE1 INIT.
pub fn insert_ila(design: &mut Design, net: &str) -> Result<(), String> {
    if !design.nets.iter().any(|n| n.name == net) {
        return Err(format!("mark_debug: no net {net}"));
    }
    if design
        .cells
        .iter()
        .any(|c| matches!(&c.kind, CellKind::Ila { net: n } if n == net))
    {
        return Ok(());
    }
    design.add_cell(
        format!("ila_{net}"),
        CellKind::Ila { net: net.into() },
    );
    // Buffer LUT: O = I0 → INIT 0xAAAA… so capture tracks the marked net.
    design.add_cell(
        format!("ila_{net}_lut"),
        CellKind::Lut6 {
            init: 0xAAAA_AAAA_AAAA_AAAA,
        },
    );
    design.add_cell(format!("ila_{net}_ff"), CellKind::Hff);
    design.connect(net, format!("ila_{net}_lut"), "I0");
    let dnet = format!("ila_{net}_d");
    design.connect(&dnet, format!("ila_{net}_lut"), "O");
    design.connect(&dnet, format!("ila_{net}_ff"), "D");
    design.connect(format!("ila_{net}_q"), format!("ila_{net}_ff"), "Q");
    Ok(())
}

/// Insert an ILA on every net with IR attr `mark_debug`.
pub fn insert_marked(design: &mut Design) -> Result<usize, String> {
    let nets = design.marked_debug_nets();
    for n in &nets {
        insert_ila(design, n)?;
    }
    Ok(nets.len())
}

pub fn compile(dev: &Device, design: &Design) -> Result<(helion_route::Routed, helion_bits::Bitstream), String> {
    let packed = pack(design, dev)?;
    let placed = place(&packed, dev)?;
    let routed = route(&placed, dev)?;
    let bits = bitgen(dev, &routed)?;
    Ok((routed, bits))
}

pub fn insert_arm_capture(
    dev: &Device,
    design: &Design,
    net: &str,
    n: usize,
) -> Result<IlaCapture, String> {
    let mut d = design.clone();
    let (_, bits0) = compile(dev, &d)?;
    insert_ila(&mut d, net)?;
    let (routed, bits1) = compile(dev, &d)?;
    if bits0.frames == bits1.frames {
        return Err("ILA insert was a no-op (bitstream unchanged)".into());
    }
    if routed.placed.packed.lutffs.len() < 2 {
        return Err("ILA did not pack an extra LUTFF".into());
    }
    let mut fab = Fabric::new(dev);
    fab.program(&bits1)?;
    fab.finish_startup();
    // Probe the marked net's driver (user LUTFF BLE0). ILA extra LUTFF is in the
    // bitstream (BLE1); capture is of the marked net, not the probe flop.
    let (site, ble) = routed.placed.lutff_sites[0];
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        fab.step_user();
        samples.push(fab.ble_q(site.x, site.y, ble as u32));
    }
    if samples.iter().all(|&s| s == samples[0]) {
        return Err("ILA capture is constant — marked net did not toggle".into());
    }
    Ok(IlaCapture {
        net: net.into(),
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::Design;

    #[test]
    fn ila_insert_changes_bitstream_and_captures() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let cap = insert_arm_capture(&dev, &d, "q", 8).unwrap();
        assert_eq!(cap.net, "q");
        assert!(cap.samples.iter().any(|&b| b) && cap.samples.iter().any(|&b| !b));
    }

    #[test]
    fn ila_unknown_net_fails() {
        let mut d = Design::structural_blinky();
        assert!(insert_ila(&mut d, "no_such").is_err());
    }

    #[test]
    fn mark_debug_attr_inserts_ila() {
        let mut d = Design::structural_blinky();
        d.mark_debug("q").unwrap();
        assert_eq!(insert_marked(&mut d).unwrap(), 1);
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Ila { .. })));
    }
}
