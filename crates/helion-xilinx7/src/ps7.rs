//! The Zynq-7000 processing system (`PS7`) as a hard block of the fabric.
//!
//! The `PS7_X0Y0` site's pins reach the fabric through the PSS tiles and
//! `INT_INTERFACE_PSS_L` over always-on pips (X-Ray `ppips_pss*.db`, no
//! bits), each ending on one INT tile wire: `IMUX`/`CLK` for an input,
//! `LOGIC_OUTS` for an output. The router takes those pips like any other.
//! The bitstream configures nothing in the PS itself (the boot code does),
//! so a PS7 adds routing and ties only.
//!
//! Ports come from X-Ray's `ps7_ports.json`, `normal` class only: those are
//! the ports of the PS7 cell. The `mio` ports (MIO, DDR, PS_CLK, PS_PORB,
//! PS_SRSTB) are the PS's own package pins, and the `test`/`debug` ones are
//! not on the cell (they have no fabric path at all). Every `normal` input
//! left unconnected is held at 0 in its INT tile: an unrouted `IMUX` reads
//! 1, which would raise all 16 PL-to-PS interrupts. An input on a `CLK`
//! wire (a port's clock) is grounded through `FAN_BOUNCE`/`FAN_ALT`/`GFAN`,
//! as Vivado's board-preset PS7 build (2017.3) grounds all 21 unused ones.
//! That build holds EMIOUART0RX/EMIOUART1RX at 1 instead: its wrapper
//! connects them to 1 (UART idle), which an RTL that instantiates PS7 says
//! by tying them to 1.

use crate::db::parse::PpipKind;
use crate::device::{default_db_root, Device};
use crate::place::Placed7;
use crate::route::RouteQuery;
use helion_ir::{intern, resolve, Design};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

/// The PS7 site of a Zynq-7000 part (there is one).
pub const SITE: &str = "PS7_X0Y0";

/// One `normal`-class port of the PS7 cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub input: bool,
    pub width: u32,
}

impl Port {
    /// Site pins, one per bit: the port's name, with the bit index on a bus.
    pub fn pins(&self) -> Vec<String> {
        if self.width == 1 {
            vec![self.name.clone()]
        } else {
            (0..self.width).map(|k| format!("{}{k}", self.name)).collect()
        }
    }
}

/// A packed PS7.
#[derive(Clone, Debug, Default)]
pub struct PackedPs7 {
    pub name: String,
    /// `(site pin, net, is_output)`.
    pub pins: Vec<(String, String, bool)>,
    /// Inputs on a constant net: `(site pin, value)`.
    pub ties: Vec<(String, bool)>,
}

/// Parse `ps7_ports.json` (`{name: {class, direction, width, min, max}}`)
/// into the `normal`-class ports, by name.
pub fn parse_ports(text: &str) -> Result<Vec<Port>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("ps7_ports.json: {e}"))?;
    let map = v.as_object().ok_or("ps7_ports.json: not an object")?;
    let mut out = Vec::new();
    for (name, p) in map {
        if p["class"].as_str() != Some("normal") {
            continue;
        }
        if p["min"].as_u64() != Some(0) {
            return Err(format!("ps7_ports.json: {name} does not start at bit 0"));
        }
        let input = match p["direction"].as_str() {
            Some("input") => true,
            Some("output") => false,
            d => return Err(format!("ps7_ports.json: {name}: direction {d:?}")),
        };
        let width = p["width"].as_u64().filter(|&w| w > 0).ok_or_else(|| format!("ps7_ports.json: {name}: no width"))? as u32;
        out.push(Port { name: name.clone(), input, width });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The PS7 ports of the database (`<db>/zynq7/ps7_ports.json`).
pub fn ports() -> Result<&'static [Port], String> {
    static P: OnceLock<Result<Vec<Port>, String>> = OnceLock::new();
    P.get_or_init(|| {
        let path = default_db_root().join("zynq7/ps7_ports.json");
        parse_ports(&std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?)
    })
    .as_deref()
    .map_err(Clone::clone)
}

/// The ports as the front end takes them: `(name, is_input, width)`.
pub fn sv_ports() -> Result<Vec<(String, bool, u32)>, String> {
    Ok(ports()?.iter().map(|p| (p.name.clone(), p.input, p.width)).collect())
}

/// The port a site pin belongs to (`MAXIGP0ARADDR7` -> `MAXIGP0ARADDR`).
pub fn port_of<'a>(ports: &'a [Port], pin: &str) -> Option<&'a Port> {
    if let Some(p) = ports.iter().find(|p| p.width == 1 && p.name == pin) {
        return Some(p);
    }
    let base = pin.trim_end_matches(|c: char| c.is_ascii_digit());
    let k: u32 = pin[base.len()..].parse().ok()?;
    ports.iter().find(|p| p.width > 1 && p.name == base && k < p.width)
}

/// Pack PS7 cell `cell`: its pins from the nets on it, directions from the database.
pub fn pack(cell: &str, netlist: &Design) -> Result<PackedPs7, String> {
    let ports = ports()?;
    let c = intern(cell);
    let mut out = PackedPs7 { name: cell.to_string(), ..Default::default() };
    for n in &netlist.nets {
        for e in n.endpoints.iter().filter(|e| e.cell == c) {
            let pin = resolve(e.pin);
            let port = port_of(ports, pin).ok_or_else(|| format!("pack: PS7 {cell}: no pin `{pin}` (MIO, DDR and PS_* pins are the PS's own package pins)"))?;
            match resolve(n.name) {
                "0" | "gnd" | "GND" | "$false" | "const0" if port.input => out.ties.push((pin.to_string(), false)),
                "1" | "vcc" | "VCC" | "$true" | "const1" if port.input => out.ties.push((pin.to_string(), true)),
                net => out.pins.push((pin.to_string(), net.to_string(), !port.input)),
            }
        }
    }
    out.pins.sort();
    out.ties.sort();
    Ok(out)
}

/// Every routed endpoint of the placed PS7: `(net, site, site pin, is_driver)`.
pub fn endpoints(placed: &Placed7) -> Vec<(String, String, String, bool)> {
    let mut out = Vec::new();
    for p in &placed.packed.ps7s {
        let Some(pp) = placed.ps7s.iter().find(|q| q.name == p.name) else { continue };
        for (pin, net, is_out) in &p.pins {
            out.push((net.clone(), pp.site.clone(), pin.clone(), *is_out));
        }
    }
    out
}

/// The INT tile wire a hard-block site pin reaches over always-on pips:
/// forward from an output, backward from an input, across node stitching
/// (a PS7 pin crosses up to two PSS tiles and an `INT_INTERFACE_PSS_L`).
pub fn int_wire(device: &Device, rq: &dyn RouteQuery, site: &str, pin: &str, output: bool) -> Option<(String, String)> {
    let start = rq.site_pin_wire(site, pin)?;
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut queue = VecDeque::from([start]);
    while let Some((t, w)) = queue.pop_front() {
        let node: Vec<(String, String)> = rq.node_wires(&t, &w);
        for (nt, nw) in std::iter::once((t, w)).chain(node) {
            let ti = device.tile_by_name(&nt)?;
            let ty = device.tile_type_name(ti)?;
            if ty == "INT_L" || ty == "INT_R" {
                return Some((nt, nw));
            }
            if !seen.insert((nt.clone(), nw.clone())) {
                continue;
            }
            let names = device.wires(ti)?;
            let pips = if output { device.pips_from(ti, &nw) } else { device.pips_into(ti, &nw) };
            for p in pips {
                let (d, s) = (&names[p.dst as usize], &names[p.src as usize]);
                let key = format!("{ty}.{d}.{s}");
                if device.ppips(ty).is_some_and(|ps| ps.iter().any(|pp| pp.kind == PpipKind::Always && pp.name == key)) {
                    queue.push_back((nt.clone(), if output { d.clone() } else { s.clone() }));
                }
            }
        }
    }
    None
}

/// FASM lines that hold every PS7 input not on a net: a constant net's
/// value, else 0 (see the module doc).
pub fn tie_lines(placed: &Placed7, nets: &[crate::route::RoutedNet7], device: &Device, rq: &dyn RouteQuery) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    if placed.packed.ps7s.is_empty() {
        return Ok(lines);
    }
    let routed = crate::bram::routed_wires(nets);
    for p in &placed.packed.ps7s {
        let site = &placed.ps7s.iter().find(|q| q.name == p.name).ok_or_else(|| format!("tie: PS7 {} is not placed", p.name))?.site;
        let used: HashSet<&str> = p.pins.iter().map(|x| x.0.as_str()).collect();
        let tied: HashMap<&str, bool> = p.ties.iter().map(|(n, v)| (n.as_str(), *v)).collect();
        for pin in ports()?.iter().filter(|q| q.input).flat_map(Port::pins) {
            if used.contains(pin.as_str()) {
                continue;
            }
            let (int, w) = int_wire(device, rq, site, &pin, false).ok_or_else(|| format!("tie: {site}.{pin}: no INT-tile wire found"))?;
            let value = tied.get(pin.as_str()).copied().unwrap_or(false);
            lines.extend(crate::bram::tie_int(device, &routed, Vec::new(), &int, &w, value, &format!("{site}.{pin}"), &pin)?);
        }
    }
    lines.sort();
    lines.dedup();
    Ok(lines)
}

/// Slow-corner timing of the PS7's clocked ports, from X-Ray's
/// `timings/PSS2.sdf`: per port, its clock port and the clock-to-out
/// (outputs) or setup (inputs) in ps.
#[derive(Clone, Debug, Default)]
pub struct Arcs {
    pub clk_to_out: HashMap<String, (String, u32)>,
    pub setup: HashMap<String, (String, u32)>,
}

/// Parse the PS7 SDF: `(IOPATH CLK PORT (r::r)(f::f))` and
/// `(SETUP PORT (posedge CLK) (a::b))`, each taken at its largest value.
pub fn parse_sdf(text: &str) -> Arcs {
    let max_ps = |s: &str| -> u32 {
        let v = s.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).filter_map(|x| x.parse::<f64>().ok()).fold(0.0f64, f64::max);
        (v * 1000.0).round() as u32
    };
    let mut a = Arcs::default();
    for l in text.lines() {
        let l = l.trim();
        if let Some(r) = l.strip_prefix("(IOPATH ") {
            let mut t = r.splitn(3, ' ');
            if let (Some(clk), Some(port), Some(rest)) = (t.next(), t.next(), t.next()) {
                let e = a.clk_to_out.entry(port.to_string()).or_insert((clk.to_string(), 0));
                e.1 = e.1.max(max_ps(rest));
            }
        } else if let Some(r) = l.strip_prefix("(SETUP ") {
            let mut t = r.splitn(2, ' ');
            if let (Some(port), Some(rest)) = (t.next(), t.next()) {
                let Some(clk) = rest.strip_prefix("(posedge ").and_then(|x| x.split(')').next()) else { continue };
                let delay = rest.split_once(')').map(|x| x.1).unwrap_or("");
                let e = a.setup.entry(port.to_string()).or_insert((clk.to_string(), 0));
                e.1 = e.1.max(max_ps(delay));
            }
        }
    }
    a
}

/// [`Arcs`] of the database (`<db>/zynq7/timings/PSS2.sdf`).
pub fn arcs() -> Result<&'static Arcs, String> {
    static A: OnceLock<Result<Arcs, String>> = OnceLock::new();
    A.get_or_init(|| {
        let path = default_db_root().join("zynq7/timings/PSS2.sdf");
        std::fs::read_to_string(&path).map(|t| parse_sdf(&t)).map_err(|e| format!("{}: {e}", path.display()))
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Timing ends of a packed PS7: launches `(net, clock net, clock-to-out ps,
/// pin)` and captures `(net, clock net, setup ps, pin)`. A port whose clock
/// pin is not on a net is left out (nothing clocks it).
#[allow(clippy::type_complexity)]
pub fn timing_ends(p: &PackedPs7) -> Result<(Vec<(&str, &str, u32, &str)>, Vec<(&str, &str, u32, &str)>), String> {
    let (ports, arcs) = (ports()?, arcs()?);
    let clock = |port: &str| p.pins.iter().find(|(pin, _, out)| !out && pin == port).map(|x| x.1.as_str());
    let (mut launch, mut capture) = (Vec::new(), Vec::new());
    for (pin, net, is_out) in &p.pins {
        let Some(port) = port_of(ports, pin) else { continue };
        let arc = if *is_out { arcs.clk_to_out.get(&port.name) } else { arcs.setup.get(&port.name) };
        let Some((clk, ps)) = arc else { continue };
        let Some(clk_net) = clock(clk) else { continue };
        if *is_out { &mut launch } else { &mut capture }.push((net.as_str(), clk_net, *ps, pin.as_str()));
    }
    Ok((launch, capture))
}

/// Period (ps) of each PS7 fabric clock on a net (`FCLKCLK<n>`), from the
/// XDC's `create_clock -period <ns> ... [get_pins <cell>/FCLKCLK[<n>]]`: the
/// PS programs the rate (ps7_init), so only the XDC can say what it is.
pub fn fclk_periods(xdc: &str, ps7s: &[PackedPs7]) -> Result<HashMap<String, u32>, String> {
    let mut out = HashMap::new();
    for p in ps7s {
        for (pin, net, _) in p.pins.iter().filter(|(pin, _, is_out)| *is_out && pin.starts_with("FCLKCLK")) {
            let bit = &pin["FCLKCLK".len()..];
            let line = xdc
                .lines()
                .map(|l| l.split('#').next().unwrap_or(""))
                .find(|l| l.trim_start().starts_with("create_clock") && l.contains("get_pins") && l.contains(&format!("FCLKCLK[{bit}]")))
                .ok_or_else(|| format!("XDC: PS7 {}/FCLKCLK[{bit}] drives net `{net}` but has no create_clock", p.name))?;
            let toks: Vec<&str> = line.split_whitespace().collect();
            let ns = toks
                .iter()
                .position(|t| *t == "-period")
                .and_then(|i| toks.get(i + 1))
                .and_then(|v| v.parse::<f64>().ok())
                .ok_or_else(|| format!("XDC: create_clock for {}/FCLKCLK[{bit}] has no -period", p.name))?;
            out.insert(net.clone(), (ns * 1000.0).round() as u32);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xdevice::{build_node_map, XDevice};

    #[test]
    fn ports_are_the_normal_class_with_bus_pins() {
        let text = r#"{"MIO": {"class": "mio", "direction": "inout", "max": 53, "min": 0, "width": 54},
            "FCLKCLK": {"class": "normal", "direction": "output", "max": 3, "min": 0, "width": 4},
            "MAXIGP0ACLK": {"class": "normal", "direction": "input", "max": 0, "min": 0, "width": 1},
            "TESTPSSTDI": {"class": "test", "direction": "input", "max": 0, "min": 0, "width": 1}}"#;
        let p = parse_ports(text).unwrap();
        assert_eq!(p.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["FCLKCLK", "MAXIGP0ACLK"]);
        assert_eq!(p[0].pins(), ["FCLKCLK0", "FCLKCLK1", "FCLKCLK2", "FCLKCLK3"]);
        assert_eq!(port_of(&p, "FCLKCLK3").map(|x| x.name.as_str()), Some("FCLKCLK"));
        assert!(port_of(&p, "FCLKCLK4").is_none() && port_of(&p, "FCLKCLK").is_none() && port_of(&p, "MIO0").is_none());
        assert_eq!(port_of(&p, "MAXIGP0ACLK").map(|x| x.input), Some(true));
        // The database's own table: 325 inputs (1774 pins) and 274 outputs (1401 pins).
        let db = ports().unwrap();
        let count = |i: bool| (db.iter().filter(|p| p.input == i).count(), db.iter().filter(|p| p.input == i).map(|p| p.width).sum::<u32>());
        assert_eq!((count(true), count(false)), ((325, 1774), (274, 1401)));
    }

    #[test]
    fn sdf_takes_the_slow_corner_maximum() {
        let a = parse_sdf(
            "(IOPATH MAXIGP0ACLK MAXIGP0AWID (0.380::0.506)(1.090::1.453))\n\
             (HOLD MAXIGP0WREADY (posedge MAXIGP0ACLK) (0.000::0.000))\n\
             (SETUP MAXIGP0WREADY (posedge MAXIGP0ACLK) (0.662::0.883))\n",
        );
        assert_eq!(a.clk_to_out["MAXIGP0AWID"], ("MAXIGP0ACLK".to_string(), 1453));
        assert_eq!(a.setup["MAXIGP0WREADY"], ("MAXIGP0ACLK".to_string(), 883));
        let db = arcs().unwrap();
        assert_eq!(db.clk_to_out["MAXIGP0ARADDR"], ("MAXIGP0ACLK".to_string(), 1438));
        assert_eq!(db.setup["MAXIGP0RDATA"], ("MAXIGP0ACLK".to_string(), 550));
        assert!(!db.clk_to_out.contains_key("FCLKCLK") && !db.setup.contains_key("IRQF2P"), "FCLK and interrupts are not clocked ports");
    }

    #[test]
    fn clocked_ports_launch_and_capture_on_their_clock() {
        let mut p = PackedPs7 {
            name: "ps7".into(),
            pins: vec![
                ("MAXIGP0ACLK".into(), "clk".into(), false),
                ("MAXIGP0RDATA0".into(), "d0".into(), false),
                ("MAXIGP0ARADDR0".into(), "a0".into(), true),
                ("FCLKRESETN0".into(), "rstn".into(), true),
                ("IRQF2P0".into(), "irq".into(), false),
            ],
            ties: Vec::new(),
        };
        let (launch, capture) = timing_ends(&p).unwrap();
        assert_eq!(launch, [("a0", "clk", 1438, "MAXIGP0ARADDR0")], "FCLK_RESET0_N has no clock");
        assert_eq!(capture, [("d0", "clk", 550, "MAXIGP0RDATA0")], "interrupts have no clock");
        p.pins.remove(0);
        let (launch, capture) = timing_ends(&p).unwrap();
        assert!(launch.is_empty() && capture.is_empty(), "an unclocked port is not timed");
    }

    #[test]
    fn fclk_period_comes_from_its_create_clock() {
        let p = PackedPs7 { name: "ps7".into(), pins: vec![("FCLKCLK0".into(), "fclk_0".into(), true), ("FCLKCLKTRIGN1".into(), "t".into(), false)], ties: Vec::new() };
        let xdc = "# create_clock -period 5 [get_pins ps7/FCLKCLK[0]]\ncreate_clock -period 20.000 -name f [get_pins ps7/FCLKCLK[1]]\ncreate_clock -period 10.000 -name f0 [get_pins ps7/FCLKCLK[0]]\n";
        assert_eq!(fclk_periods(xdc, std::slice::from_ref(&p)).unwrap(), HashMap::from([("fclk_0".to_string(), 10_000)]));
        let e = fclk_periods("create_clock -period 8 [get_ports clk]\n", &[p]).unwrap_err();
        assert!(e.contains("FCLKCLK[0]") && e.contains("no create_clock"), "{e}");
    }

    #[test]
    fn pins_reach_their_int_tiles_and_unused_inputs_tie_to_ground() {
        let mut dev = Device::load_xc7z020(&default_db_root()).expect("load xc7z020");
        let maps = build_node_map(&mut dev);
        let x = XDevice::new(&dev, maps);
        let w = |pin: &str, out: bool| int_wire(&dev, &x, SITE, pin, out).map(|(t, w)| format!("{t}.{w}"));
        // Straight through PSS2; through PSS1 and INT_INTERFACE_PSS_L; a port clock.
        assert_eq!(w("MAXIGP0RDATA0", false).as_deref(), Some("INT_L_X18Y90.IMUX_L5"));
        assert_eq!(w("FCLKCLK0", true).as_deref(), Some("INT_L_X18Y89.LOGIC_OUTS_L1"));
        assert_eq!(w("MAXIGP0ACLK", false).as_deref(), Some("INT_L_X18Y95.CLK_L0"));
        // Every database pin reaches a distinct INT wire of its kind.
        let mut seen = HashSet::new();
        for p in ports().unwrap() {
            for pin in p.pins() {
                let (t, w) = int_wire(&dev, &x, SITE, &pin, !p.input).unwrap_or_else(|| panic!("{pin}: no INT wire"));
                assert!(if p.input { w.starts_with("IMUX_L") || w.starts_with("CLK_L") } else { w.starts_with("LOGIC_OUTS_L") }, "{pin}: {t}.{w}");
                assert!(seen.insert((t, w)), "{pin} shares its INT wire");
            }
        }
        assert_eq!(seen.len(), 3175);

        let placed = Placed7 {
            packed: crate::pack::Packed7 {
                ps7s: vec![PackedPs7 {
                    name: "ps7".into(),
                    pins: vec![("MAXIGP0ACLK".into(), "clk".into(), false), ("MAXIGP0RDATA0".into(), "d0".into(), false)],
                    ties: vec![("IRQF2P1".into(), true)],
                }],
                ..Default::default()
            },
            ps7s: vec![crate::place::PlacedPs7 { name: "ps7".into(), tile: "PSS2_X32Y105".into(), site: SITE.into() }],
            ..Default::default()
        };
        let lines = tie_lines(&placed, &[], &dev, &x).unwrap();
        let has = |l: &str| lines.iter().any(|x| x.starts_with(l));
        // IRQF2P0 (INT_L_X18Y95.IMUX_L0) unconnected: held at 0 through a GFAN.
        assert!(has("INT_L_X18Y95.IMUX_L0.GFAN") && has("INT_L_X18Y95.GFAN"), "{:?}", &lines[..8]);
        assert!(!has("INT_L_X18Y90.IMUX_L5."), "a connected input is the router's");
        let irq1 = int_wire(&dev, &x, SITE, "IRQF2P1", false).unwrap();
        assert!(!has(&format!("{}.{}.", irq1.0, irq1.1)), "a 1 is the INT tile's default");
        // Every other IMUX input has its GFAN line: 1774 pins, 2 on nets (one a clock), 1 at 1, 21 unused clocks.
        assert_eq!(lines.iter().filter(|l| l.contains(".IMUX_L")).count(), 1774 - 2 - 1 - 21);
        // The 21 unused port clocks are grounded exactly as in Vivado's PS7 build (zed_ps7.bit, 2017.3).
        let clk: HashSet<&str> = lines.iter().filter(|l| l.contains(".CLK_L")).map(String::as_str).collect();
        let vivado = [54, 62, 70, 78, 85, 86, 87, 88, 89, 105, 110, 117, 118, 121, 122, 127, 135, 139, 142, 146, 148];
        let want: Vec<String> = vivado.iter().map(|y| format!("INT_L_X18Y{y}.CLK_L0.FAN_BOUNCE5")).collect();
        assert_eq!(clk, want.iter().map(String::as_str).collect::<HashSet<_>>());
        assert!(vivado.iter().all(|y| has(&format!("INT_L_X18Y{y}.FAN_ALT5.GFAN1")) && has(&format!("INT_L_X18Y{y}.GFAN1.GND_WIRE"))));
        // A port clock tied to 0 is grounded the same way; to 1 it cannot be (no VCC default on CLK).
        let mut t = placed.clone();
        t.packed.ps7s[0].ties.push(("MAXIGP1ACLK".into(), false));
        assert!(tie_lines(&t, &[], &dev, &x).unwrap().contains(&"INT_L_X18Y105.CLK_L0.FAN_BOUNCE5".to_string()));
        t.packed.ps7s[0].ties.last_mut().unwrap().1 = true;
        let e = tie_lines(&t, &[], &dev, &x).unwrap_err();
        assert!(e.contains("MAXIGP1ACLK") && e.contains("tied to 1"), "{e}");
    }
}
