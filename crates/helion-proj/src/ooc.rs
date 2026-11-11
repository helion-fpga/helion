//! UG905 out-of-context (OOC) hierarchical synthesis for project modules.
//!
//! [`synth_module_ooc`] synthesizes one module of the project on its own
//! (helion-sv elaboration with that module as top) and caches the netlist
//! as a post-synthesis `.hck2` checkpoint keyed by the SHA-256 of the
//! module name plus every source file, so an unchanged module is restored
//! instead of re-synthesized ([`crate::ooc_sha256_hex`], the same content
//! addressing the IP OOC cache uses).
//!
//! [`synth_top_with_ooc`] synthesizes the top with every OOC module
//! replaced by a port-only black-box stub generated from the cached
//! netlist, then stitches the cached netlist into each instance: the
//! module's port buffers are dropped, its port bit nets (`port`,
//! `port_<i>`) bind to the parent's connection nets, and every other cell
//! and net gets the instance prefix (`<inst>_`), matching how the
//! flattener names hierarchy.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{CheckpointSummary, Mode, Session, load_checkpoint, ooc_sha256_hex, save_checkpoint};
use helion_device::Device;
use helion_ir::{CellKind, Design, Endpoint, Net, PortDir, intern, resolve};
use helion_sta::Constraints;

/// One OOC module synthesis (or cache restore).
#[derive(Clone, Debug)]
pub struct OocModuleRun {
    pub module: String,
    pub key: String,
    pub cache_hit: bool,
    pub checkpoint: PathBuf,
    pub cells: usize,
    pub luts: usize,
    pub ffs: usize,
    pub design: Design,
}

fn read_all(files: &[PathBuf]) -> Result<Vec<(String, String)>, String> {
    files
        .iter()
        .map(|p| {
            std::fs::read_to_string(p)
                .map(|t| (p.display().to_string(), t))
                .map_err(|e| format!("{}: {e}", p.display()))
        })
        .collect()
}

/// Content key of `module` over `files`.
pub fn ooc_key(module: &str, files: &[PathBuf]) -> Result<String, String> {
    let mut payload = format!("module={module};");
    for (name, text) in read_all(files)? {
        payload.push_str(&format!("file={name};sha={};", ooc_sha256_hex(text.as_bytes())));
    }
    Ok(ooc_sha256_hex(payload.as_bytes()))
}

/// Synthesize `module` out of context (or restore it from `cache_dir`).
pub fn synth_module_ooc(
    files: &[PathBuf],
    module: &str,
    cache_dir: &Path,
    dev: &Device,
) -> Result<OocModuleRun, String> {
    let key = ooc_key(module, files)?;
    let name = format!("ooc_{module}_{}", &key[..16]);
    let path = cache_dir.join(format!("{name}.hck2"));
    let stats = |d: &Design| {
        (
            d.cells.len(),
            d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count(),
            d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count(),
        )
    };
    if path.is_file() {
        if let Ok(rc) = load_checkpoint(&path, dev) {
            if let Some(d) = rc.session.design {
                let (cells, luts, ffs) = stats(&d);
                return Ok(OocModuleRun { module: module.into(), key, cache_hit: true, checkpoint: path, cells, luts, ffs, design: d });
            }
        }
    }
    let owned = read_all(files)?;
    let refs: Vec<(&str, &str)> = owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let (design, _) = helion_sv::elaborate_sv_sources(&refs, Some(module), &Default::default(), &Default::default())
        .map_err(|e| format!("synth_design -mode out_of_context -top {module}: {e}"))?;
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("{}: {e}", cache_dir.display()))?;
    let mut s = Session::new(Mode::NonProject);
    s.part = dev.part.clone();
    s.synth_design(design.clone());
    let saved = save_checkpoint(&s, &Constraints::default(), &name, cache_dir, &CheckpointSummary::default())?;
    let (cells, luts, ffs) = stats(&design);
    Ok(OocModuleRun { module: module.into(), key, cache_hit: false, checkpoint: saved, cells, luts, ffs, design })
}

/// Bit width of `port` in an OOC netlist (`port_<i>` bit nets, else 1).
fn port_width(d: &Design, port: &str) -> usize {
    let pfx = format!("{port}_");
    let bits = d
        .nets
        .iter()
        .filter_map(|n| resolve(n.name).strip_prefix(&pfx).and_then(|i| i.parse::<usize>().ok()))
        .max();
    bits.map(|m| m + 1).unwrap_or(1)
}

/// Port-only Verilog stub for an OOC netlist.
pub fn black_box_stub(module: &str, d: &Design) -> String {
    let ports: Vec<String> = d
        .ports
        .iter()
        .map(|p| {
            let n = resolve(p.name);
            let dir = match p.dir {
                PortDir::In => "input",
                PortDir::Out => "output",
                PortDir::Inout => "inout",
            };
            match port_width(d, n) {
                1 => format!("{dir} {n}"),
                w => format!("{dir} [{}:0] {n}", w - 1),
            }
        })
        .collect();
    format!("module {module}({});\nendmodule\n", ports.join(", "))
}

/// Replace `module <name> ... endmodule` in `text` with `stub`; `None`
/// when `text` does not define the module.
fn replace_module(text: &str, name: &str, stub: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut search = 0;
    while let Some(off) = text[search..].find("module") {
        let at = search + off;
        let before_ok = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let rest = &text[at + 6..];
        let ident: String = rest.trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if before_ok && rest.starts_with(char::is_whitespace) && ident == name {
            let end = text[at..].find("endmodule")? + at + "endmodule".len();
            return Some(format!("{}{}{}", &text[..at], stub, &text[end..]));
        }
        search = at + 6;
    }
    None
}

/// Parent net bit name for connection `pnet` (`a` / `cnt[3]` → `cnt_3`).
fn parent_bit(pnet: &str, bit: Option<usize>) -> Result<String, String> {
    if let Some(open) = pnet.find('[') {
        let base = &pnet[..open];
        let idx = pnet[open + 1..].trim_end_matches(']');
        if bit.is_some_and(|b| b > 0) || idx.contains(':') {
            return Err(format!("OOC: connection {pnet} is a part-select (only whole nets or single bits are supported)"));
        }
        return Ok(format!("{base}_{idx}"));
    }
    if pnet.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '_')) {
        return Err(format!("OOC: connection {pnet} is an expression (connect a named net)"));
    }
    Ok(match bit {
        Some(i) => format!("{pnet}_{i}"),
        None => pnet.to_string(),
    })
}

/// Stitch the OOC netlist `child` into `parent` at instance `inst`
/// (`conns`: child port → parent net text).
pub fn stitch(parent: &mut Design, inst: &str, child: &Design, conns: &[(String, String)]) -> Result<usize, String> {
    let ports: HashMap<&str, &str> = conns.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let port_names: Vec<String> = child.ports.iter().map(|p| resolve(p.name).to_string()).collect();
    // Port buffers belong to the OOC top, not to the instance.
    let iobs: std::collections::HashSet<_> = child
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobIn | CellKind::IobOut))
        .map(|c| c.name)
        .collect();
    let rename = |net: &str| -> Result<String, String> {
        for p in &port_names {
            let pnet = ports.get(p.as_str());
            if net == p {
                return pnet.map(|n| parent_bit(n, None)).unwrap_or_else(|| Ok(format!("{inst}_{net}")));
            }
            if let Some(i) = net.strip_prefix(&format!("{p}_")).and_then(|i| i.parse::<usize>().ok()) {
                return match pnet {
                    Some(n) if port_width(child, p) > 1 => parent_bit(n, Some(i)),
                    Some(n) => parent_bit(n, None),
                    None => Ok(format!("{inst}_{net}")),
                };
            }
        }
        Ok(format!("{inst}_{net}"))
    };
    let mut added = 0;
    for c in &child.cells {
        if iobs.contains(&c.name) {
            continue;
        }
        let mut c2 = c.clone();
        c2.name = intern(&format!("{inst}_{}", resolve(c.name)));
        parent.push_cell(c2);
        added += 1;
    }
    for n in &child.nets {
        let eps: Vec<Endpoint> = n
            .endpoints
            .iter()
            .filter(|e| !iobs.contains(&e.cell))
            .map(|e| Endpoint { cell: intern(&format!("{inst}_{}", resolve(e.cell))), pin: e.pin })
            .collect();
        if eps.is_empty() {
            continue;
        }
        let name = rename(resolve(n.name))?;
        parent.merge_net(Net { name: intern(&name), endpoints: eps, attrs: n.attrs.clone() });
    }
    parent.rebuild_indexes();
    Ok(added)
}

/// Synthesize `files` with every module in `ooc` black-boxed, then stitch
/// the OOC netlists into their instances. Returns the design plus one log
/// line per stitched instance.
pub fn synth_top_with_ooc(files: &[PathBuf], ooc: &[OocModuleRun]) -> Result<(Design, Vec<String>), String> {
    let mut owned = read_all(files)?;
    for m in ooc {
        let stub = black_box_stub(&m.module, &m.design);
        let mut found = false;
        for (_, text) in owned.iter_mut() {
            if let Some(t) = replace_module(text, &m.module, &stub) {
                *text = t;
                found = true;
            }
        }
        if !found {
            return Err(format!("OOC module {} is not defined in the sources", m.module));
        }
    }
    let refs: Vec<(&str, &str)> = owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let mut design = helion_sv::synth_sv_sources(&refs)?;
    let mut log = Vec::new();
    for m in ooc {
        let insts: Vec<(String, Vec<(String, String)>)> = design
            .instances
            .iter()
            .filter(|i| resolve(i.module) == m.module)
            .map(|i| {
                (
                    resolve(i.name).to_string(),
                    i.conns.iter().map(|(a, b)| (resolve(*a).to_string(), resolve(*b).to_string())).collect(),
                )
            })
            .collect();
        if insts.is_empty() {
            log.push(format!("OOC module {} has no instance in the top", m.module));
        }
        for (inst, conns) in insts {
            let n = stitch(&mut design, &inst, &m.design, &conns)?;
            log.push(format!(
                "read_checkpoint -cell {inst} {} cells={n} cache_hit={}",
                m.checkpoint.display(),
                m.cache_hit
            ));
        }
    }
    Ok((design, log))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: &str = "module top(input clk, input [1:0] sel, output [3:0] led);\n  wire [3:0] q;\n  child u_c (.clk(clk), .sel(sel), .q(q));\n  assign led = q ^ 4'b0101;\nendmodule\n";
    const CHILD: &str = "module child(input clk, input [1:0] sel, output reg [3:0] q);\n  always @(posedge clk) q <= q + {2'b0, sel};\nendmodule\n";

    fn setup(name: &str) -> (PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("helion-ooc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let top = dir.join("top.v");
        let child = dir.join("child.v");
        std::fs::write(&top, TOP).unwrap();
        std::fs::write(&child, CHILD).unwrap();
        (dir, vec![top, child])
    }

    #[test]
    fn ooc_module_caches_and_invalidates_on_edit() {
        let (dir, files) = setup("cache");
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let a = synth_module_ooc(&files, "child", &dir.join("cache"), &dev).unwrap();
        assert!(!a.cache_hit && a.ffs == 4 && a.checkpoint.is_file());
        let b = synth_module_ooc(&files, "child", &dir.join("cache"), &dev).unwrap();
        assert!(b.cache_hit, "unchanged sources restore the checkpoint");
        assert_eq!(b.cells, a.cells);
        std::fs::write(&files[1], CHILD.replace("q + {2'b0, sel}", "q - {2'b0, sel}")).unwrap();
        let c = synth_module_ooc(&files, "child", &dir.join("cache"), &dev).unwrap();
        assert!(!c.cache_hit, "an edited module re-synthesizes");
        assert_ne!(c.key, a.key);
    }

    #[test]
    fn stub_matches_ports_and_widths() {
        let (dir, files) = setup("stub");
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let m = synth_module_ooc(&files, "child", &dir.join("cache"), &dev).unwrap();
        let stub = black_box_stub("child", &m.design);
        assert!(stub.contains("input clk") && stub.contains("input [1:0] sel") && stub.contains("output [3:0] q"), "{stub}");
        assert!(replace_module(TOP, "child", &stub).is_none());
        assert!(replace_module(CHILD, "child", &stub).unwrap().contains("endmodule"));
    }

    #[test]
    fn top_reuses_the_ooc_netlist_and_drives_the_instance_outputs() {
        let (dir, files) = setup("top");
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let m = synth_module_ooc(&files, "child", &dir.join("cache"), &dev).unwrap();
        let (d, log) = synth_top_with_ooc(&files, std::slice::from_ref(&m)).unwrap();
        assert!(log.iter().any(|l| l.contains("-cell u_c")), "{log:?}");
        assert_eq!(d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count(), 4);
        // Every parent q bit is now driven by the stitched counter FFs.
        for i in 0..4 {
            let n = d.net(&format!("q_{i}")).unwrap_or_else(|| panic!("q_{i}"));
            assert!(
                n.endpoints.iter().any(|e| resolve(e.pin) == "Q" && resolve(e.cell).starts_with("u_c_")),
                "q_{i} driven by the OOC netlist"
            );
        }
        // The stitched design implements end to end.
        let mut s = Session::new(Mode::Project);
        s.impl_design(d, &dev).unwrap();
        assert!(s.bitstream.is_some());
    }
}
