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

fn ila_cell_names(net: &str) -> [String; 3] {
    [
        format!("ila_{net}"),
        format!("ila_{net}_lut"),
        format!("ila_{net}_ff"),
    ]
}

fn ila_aux_nets(net: &str) -> [String; 2] {
    [format!("ila_{net}_d"), format!("ila_{net}_q")]
}

/// Drop a prior `insert_ila` for `net` so baseline vs probe bitstreams can differ.
/// `Session::mark_debug` / `insert_marked` already inject the probe; arm must still work.
pub fn strip_ila(design: &mut Design, net: &str) {
    let cells = ila_cell_names(net);
    let aux = ila_aux_nets(net);
    design.cells.retain(|c| !cells.iter().any(|n| n == &c.name));
    design.nets.retain(|n| !aux.iter().any(|a| a == &n.name));
    for n in &mut design.nets {
        n.endpoints
            .retain(|e| !cells.iter().any(|c| c == &e.cell));
    }
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
    // Baseline without this probe — even if mark_debug / (re)implement already inserted it.
    let mut baseline = design.clone();
    strip_ila(&mut baseline, net);
    let (_, bits0) = compile(dev, &baseline)?;
    let packed0 = pack(&baseline, dev)?;

    let mut d = design.clone();
    // Rebuild probe cleanly (idempotent with a prior Session::mark_debug insert).
    strip_ila(&mut d, net);
    insert_ila(&mut d, net)?;
    let (routed, bits1) = compile(dev, &d)?;
    if bits0.frames == bits1.frames {
        return Err("ILA insert was a no-op (bitstream unchanged)".into());
    }
    if routed.placed.packed.lutffs.len() <= packed0.lutffs.len() {
        return Err("ILA did not pack an extra LUTFF".into());
    }
    let mut fab = Fabric::new(dev);
    fab.program(&bits1)?;
    fab.finish_startup();
    // Probe the marked net's driver LUTFF (q_net / IOB from_net), not always site[0].
    // ILA extra LUTFF is in the bitstream; capture is of the marked net, not the probe flop.
    let (site, ble) = probe_site(&routed.placed, design, net)?;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        fab.step_user();
        // Comb mid-nets (LUT-only BLE) must sample LUT O, not stuck FF Q.
        samples.push(fab.ble_out(site.x, site.y, ble as u32));
    }
    if samples.iter().all(|&s| s == samples[0]) {
        return Err(format!(
            "ILA capture is constant on {net} over {n} samples — increase ila_window or pick a faster net"
        ));
    }
    Ok(IlaCapture {
        net: net.into(),
        samples,
    })
}

/// Resolve the fabric (site, BLE) that drives `net` (FF Q, or IOB PAD via from_net).
fn probe_site(
    placed: &helion_place::Placed,
    design: &Design,
    net: &str,
) -> Result<(helion_device::Site, u8), String> {
    let packed = &placed.packed;
    if let Some(i) = packed.lutffs.iter().position(|l| l.q_net == net) {
        return placed
            .lutff_sites
            .get(i)
            .copied()
            .ok_or_else(|| format!("ILA: no site for q_net {net}"));
    }
    // PAD / port net: follow IOB I ← from_net → that LUTFF's Q.
    if let Some(iob) = packed.iobs.iter().find(|iob| {
        design.net_on(&iob.cell, "PAD") == Some(net)
    }) {
        if let Some(i) = packed.lutffs.iter().position(|l| l.q_net == iob.from_net) {
            return placed
                .lutff_sites
                .get(i)
                .copied()
                .ok_or_else(|| format!("ILA: no site for IOB from_net {}", iob.from_net));
        }
        return Err(format!(
            "ILA: PAD {net} driven by {}, but that net is not a packed FF Q",
            iob.from_net
        ));
    }
    // Comb / other: match LUT O net packed as q_net for LUT-only clusters.
    if let Some(i) = packed
        .lutffs
        .iter()
        .position(|l| l.q_net == net || l.lut_cell == net)
    {
        return placed
            .lutff_sites
            .get(i)
            .copied()
            .ok_or_else(|| format!("ILA: no site for {net}"));
    }
    Err(format!(
        "ILA: no packed LUTFF drives net {net} (need FF Q or IOB PAD)"
    ))
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

    #[test]
    fn capture_reads_marked_net_not_site0() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let c0 = insert_arm_capture(&dev, &d, "q0", 16).unwrap();
        let c3 = insert_arm_capture(&dev, &d, "q3", 16).unwrap();
        let led = insert_arm_capture(&dev, &d, "led", 16).unwrap();
        let b0: String = c0.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
        let b3: String = c3.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
        let bl: String = led.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
        assert_eq!(b0, "1010101010101010", "LSB toggles every cycle: {b0}");
        assert_eq!(b3, "0000000111111110", "MSB matches gold LED stream: {b3}");
        assert_eq!(bl, b3, "PAD led follows q3 driver");
        assert_ne!(b0, b3, "must not stub every probe as site[0]");
    }

    /// Real flow: mark_debug / insert_marked then arm must NOT hit "bitstream unchanged".
    #[test]
    fn mark_debug_then_arm_inserts_probe_not_noop() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_counter();
        d.mark_debug("q3").unwrap();
        assert_eq!(insert_marked(&mut d).unwrap(), 1);
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(&c.kind, CellKind::Ila { net } if net == "q3"))
        );
        let cap = insert_arm_capture(&dev, &d, "q3", 16).expect(
            "mark_debug → arm must capture; must not Err(bitstream unchanged)",
        );
        let bits: String = cap.samples.iter().map(|b| if *b { '1' } else { '0' }).collect();
        assert_eq!(bits, "0000000111111110", "MSB gold after pre-insert: {bits}");
    }

    #[test]
    fn strip_ila_removes_probe_cells() {
        let mut d = Design::structural_blinky();
        insert_ila(&mut d, "q").unwrap();
        assert!(d.cells.iter().any(|c| c.name.starts_with("ila_q")));
        strip_ila(&mut d, "q");
        assert!(!d.cells.iter().any(|c| c.name.starts_with("ila_q")));
        assert!(!d.nets.iter().any(|n| n.name.starts_with("ila_q")));
        // Marked net keeps user endpoints (FF Q), not ILA sink.
        let q = d.net("q").unwrap();
        assert!(!q.endpoints.iter().any(|e| e.cell.starts_with("ila_")));
    }
}
