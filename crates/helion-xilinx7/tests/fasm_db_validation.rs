//! Gate: every FASM line the backend emits must resolve against the X-Ray DB.
//!
//! This is the test that was missing. Without it a backend can emit unlimited
//! nonsense -- the e2e harnesses filtered unresolvable lines out before
//! applying them, so "the harness passed" said nothing about the bitstream.
//!
//! A line `TILE.FEATURE` is acceptable iff, for the tile's *type*:
//!   * `TYPE.FEATURE` is a segbits feature (`segbits_<type>.db`), with
//!     per-bit expansion for `NAME[hi:lo]`; or
//!   * `TYPE.FEATURE` is a pseudo-pip (`ppips_<type>.db`), which FASM allows
//!     and `fasm2frames` skips. These carry no configuration bits.
//!
//! Anything else is a fabricated feature: it cannot be assembled, and
//! `FrameStore::apply_fasm` would reject the line. `*_SING`, `*_TBYTESRC` and
//! `*_TBYTETERM` IOI tiles are the trap -- upstream X-Ray ships them with
//! ppips only, so configurable features emitted there are always fabricated.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};
use serde_json::Value;

/// Valid feature names (without the tile-type prefix) per tile type.
pub struct DbFeatures {
    seg: HashMap<String, HashSet<String>>,
    ppips: HashMap<String, HashSet<String>>,
    tile_type: HashMap<String, String>,
}

impl DbFeatures {
    pub fn load(zynq7: &Path) -> Option<Self> {
        let tilegrid = zynq7.join("xc7z020/tilegrid.json");
        let json: Value = serde_json::from_str(&std::fs::read_to_string(&tilegrid).ok()?).ok()?;
        let obj = json.as_object()?;
        let mut tile_type = HashMap::new();
        for (name, info) in obj {
            if let Some(t) = info.get("type").and_then(|v| v.as_str()) {
                tile_type.insert(name.clone(), t.to_string());
            }
        }

        let mut seg: HashMap<String, HashSet<String>> = HashMap::new();
        let mut ppips: HashMap<String, HashSet<String>> = HashMap::new();
        for entry in std::fs::read_dir(zynq7).ok()? {
            let path = entry.ok()?.path();
            let fname = path.file_name()?.to_str()?.to_string();
            if !fname.ends_with(".db") || fname.contains("origin") {
                continue;
            }
            let (tt, dst) = if let Some(r) = fname.strip_prefix("segbits_") {
                (r.trim_end_matches(".db"), &mut seg)
            } else if let Some(r) = fname.strip_prefix("ppips_") {
                (r.trim_end_matches(".db"), &mut ppips)
            } else {
                continue;
            };
            // Tile-type keys are upper-case in the DB; prefix each feature
            // with the type so the lookup matches the DB's own convention.
            let key = tt.to_uppercase();
            let text = std::fs::read_to_string(&path).ok()?;
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let Some(name) = parts.next() else { continue };
                if parts.next().is_none() {
                    continue;
                }
                let bare = name
                    .strip_prefix(&format!("{}.", tt))
                    .unwrap_or(name)
                    .to_string();
                dst.entry(key.clone()).or_default().insert(bare);
            }
        }
        Some(DbFeatures {
            seg,
            ppips,
            tile_type,
        })
    }

    /// Resolve `TILE.FEATURE` to `(tile_type, bare_feature)`, expanding
    /// per-bit `NAME[hi:lo]` vectors.
    pub fn resolves(&self, tile: &str, feature: &str) -> bool {
        let Some(tt) = self.tile_type.get(tile) else {
            return false;
        };
        let tt = tt.to_uppercase();
        let seg = self.seg.get(&tt);
        let ppi = self.ppips.get(&tt);

        // Per-bit vector, e.g. SLICEL_X1.ALUT.INIT[63:0].
        if let Some((base, hilo)) = feature.rsplit_once('[') {
            let hilo = hilo.trim_end_matches(']');
            if let Some((hi, lo)) = hilo.split_once(':') {
                if let (Ok(hi), Ok(lo)) = (hi.parse::<u32>(), lo.parse::<u32>()) {
                    let Some(s) = seg else { return false };
                    return (lo..=hi).all(|i| {
                        s.contains(&format!("{base}[{i:02}]")) || s.contains(&format!("{base}[{i}]"))
                    });
                }
            }
        }
        seg.is_some_and(|s| s.contains(feature)) || ppi.is_some_and(|p| p.contains(feature))
    }
}

/// Fail with a readable list of every fabricated line.
pub fn assert_no_fabricated(db: &DbFeatures, fasm: &[String], what: &str) {
    let mut bad: Vec<&String> = Vec::new();
    for line in fasm {
        let left = line.split('=').next().unwrap_or("").trim();
        let Some((tile, feature)) = left.split_once('.') else {
            bad.push(line);
            continue;
        };
        if !db.resolves(tile, feature) {
            bad.push(line);
        }
    }
    assert!(
        bad.is_empty(),
        "{what}: {} of {} FASM lines do not resolve against the X-Ray DB \
         (fabricated features cannot be assembled):\n  {}",
        bad.len(),
        fasm.len(),
        bad.iter()
            .take(25)
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

fn db_root(zynq7: &Path) -> Option<std::path::PathBuf> {
    zynq7.parent().map(Path::to_path_buf)
}

/// The blinky, end to end, with no line filtered out. Every emitted feature
/// must be one the DB can assemble.
#[test]
fn blinky_fasm_is_fully_resolvable() {
    let root = default_db_root();
    let zynq7 = root.join("zynq7");
    let Some(db) = DbFeatures::load(&zynq7) else {
        eprintln!("SKIP: X-Ray DB not present at {}", zynq7.display());
        return;
    };
    let Some(_) = db_root(&zynq7) else { return };

    let mut device = Device::load_xc7z020(&root).expect("load xc7z020");
    let nodes = build_node_map(&mut device);
    let src = std::fs::read_to_string("../../examples/zedboard_blinky.sv").expect("blinky sv");
    let mut design = helion_sv::synth_sv(&src, "zedboard_blinky.sv").expect("synth");
    let xdc = std::fs::read_to_string("../../examples/zedboard.xdc").expect("xdc");

    // Apply the LED/clk LOCs from the XDC.
    let pins = parse_package_pins(&xdc);
    for port in &mut design.ports {
        let key = port.name.clone();
        if let Some(p) = pins.get(&key) {
            port.attrs.set("LOC", p);
        }
    }
    let mut packed = pack(&design).expect("pack");
    for (i, iob) in packed.iobs.iter_mut().enumerate() {
        if let Some(p) = pins.get(&iob.port) {
            iob.loc = Some(p.clone());
        } else if iob.port == "led" {
            // Bus port: resolve led[n] by net name.
            let bit = iob
                .net
                .rsplit('_')
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(i);
            if let Some(p) = pins.get(&format!("led[{bit}]")) {
                iob.loc = Some(p.clone());
            }
        }
        iob.iostandard = "LVCMOS33".to_string();
    }

    let xdev = XDevice::new(&device, nodes);
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");
    let bs = emit_fasm(&routed, &xdev).expect("emit");
    assert_no_fabricated(&db, &bs.fasm, "zedboard blinky");
}

/// `set_property PACKAGE_PIN <pin> [get_ports {<port>}]`, per-bit aware.
pub fn parse_package_pins(xdc: &str) -> HashMap<String, String> {
    let mut pins = HashMap::new();
    for line in xdc.lines() {
        let line = line.trim();
        if !line.starts_with("set_property PACKAGE_PIN") {
            continue;
        }
        let mut parts = line.split_whitespace();
        parts.next();
        parts.next();
        let Some(pin) = parts.next() else { continue };
        let rest: String = parts.collect::<Vec<_>>().join(" ");
        if let Some(start) = rest.find('{') {
            if let Some(end) = rest[start..].find('}') {
                let port = rest[start + 1..start + end].trim().to_string();
                pins.insert(port, pin.to_string());
            }
        }
    }
    pins
}
