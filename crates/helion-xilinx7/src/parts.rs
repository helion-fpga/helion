//! Parts and boards.
//!
//! A part is a full Xilinx part name (`xc7a35tcsg324-1`). Everything the
//! flow needs about it comes from the Project X-Ray database itself:
//! `<family>/mapping/parts.yaml` gives its device, package and speed grade,
//! `<family>/mapping/devices.yaml` the fabric the device is built on
//! (`xc7a35t` uses the `xc7a50t` fabric), and `<family>/<part>/part.json`
//! the IDCODE and frame layout. Any part the database describes works; a
//! part it does not describe is an error naming the parts it does.
//!
//! A board is a part plus the board's pin constraints (`boards/<name>.xdc`
//! in the repository).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A resolved part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// Full part name, e.g. `xc7a35tcsg324-1`.
    pub name: String,
    /// Database family directory name: `artix7`, `zynq7`, ...
    pub family: String,
    /// Device, e.g. `xc7a35t`.
    pub device: String,
    /// Fabric the device is built on, e.g. `xc7a50t`.
    pub fabric: String,
    /// Package, e.g. `csg324`.
    pub package: String,
    pub speedgrade: String,
    /// JTAG IDCODE from `part.json`.
    pub idcode: u32,
    /// `<db_root>/<family>`.
    pub family_dir: PathBuf,
}

impl Part {
    /// `<family>/<fabric>`: `tilegrid.json`, `tileconn.json`.
    pub fn fabric_dir(&self) -> PathBuf {
        self.family_dir.join(&self.fabric)
    }
    /// `<family>/<part>`: `part.json`, `package_pins.csv`.
    pub fn part_dir(&self) -> PathBuf {
        self.family_dir.join(&self.name)
    }
}

/// Short device names accepted for compatibility, with the part they stand
/// for (the parts Helion first targeted).
const SHORT_NAMES: &[(&str, &str)] = &[("xc7a35t", "xc7a35tcsg324-1"), ("xc7z020", "xc7z020clg484-1")];

/// Minimal reader for the database's `mapping/*.yaml`: two-level maps of
/// `key:` lines followed by indented `field: value` lines.
fn read_yaml_map(path: &Path) -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let unquote = |s: &str| s.trim().trim_matches('"').trim_matches('\'').to_string();
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let body = line.split('#').next().unwrap_or("");
        if body.trim().is_empty() {
            continue;
        }
        let indented = body.starts_with(' ') || body.starts_with('\t');
        let Some((k, v)) = body.split_once(':') else { continue };
        if !indented {
            let key = unquote(k);
            out.entry(key.clone()).or_default();
            cur = Some(key);
        } else if let Some(c) = &cur {
            out.get_mut(c).unwrap().insert(unquote(k), unquote(v));
        }
    }
    Ok(out)
}

/// Families present under `db_root` (directories with `mapping/parts.yaml`).
pub fn families(db_root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(db_root)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().join("mapping/parts.yaml").is_file())
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Resolve `name` (a full part name, or `xc7a35t` / `xc7z020`) against the
/// database at `db_root`.
pub fn resolve(db_root: &Path, name: &str) -> Result<Part, String> {
    let full = SHORT_NAMES.iter().find(|(s, _)| *s == name).map(|(_, f)| *f).unwrap_or(name);
    let fams = families(db_root);
    if fams.is_empty() {
        return Err(format!("no Project X-Ray database families under {} (set HELION_XILINX_DB)", db_root.display()));
    }
    for fam in &fams {
        let family_dir = db_root.join(fam);
        let parts = read_yaml_map(&family_dir.join("mapping/parts.yaml"))?;
        let Some(p) = parts.get(full) else { continue };
        let device = p.get("device").cloned().ok_or_else(|| format!("{fam} parts.yaml: {full} has no device"))?;
        let devices = read_yaml_map(&family_dir.join("mapping/devices.yaml"))?;
        let fabric = devices
            .get(&device)
            .and_then(|d| d.get("fabric").cloned())
            .ok_or_else(|| format!("{fam} devices.yaml: no fabric for {device}"))?;
        let part_json = family_dir.join(full).join("part.json");
        let text = std::fs::read_to_string(&part_json)
            .map_err(|e| format!("part {full} is in {fam}/mapping/parts.yaml but {} is missing: {e}", part_json.display()))?;
        let pj: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", part_json.display()))?;
        let idcode = pj["idcode"].as_u64().ok_or_else(|| format!("{}: no idcode", part_json.display()))? as u32;
        if !family_dir.join(&fabric).join("tilegrid.json").is_file() {
            return Err(format!("part {full}: fabric {fabric} has no tilegrid.json in {}", family_dir.display()));
        }
        return Ok(Part {
            name: full.to_string(),
            family: fam.clone(),
            device,
            fabric,
            package: p.get("package").cloned().unwrap_or_default(),
            speedgrade: p.get("speedgrade").cloned().unwrap_or_default(),
            idcode,
            family_dir,
        });
    }
    // Name the parts the database does have (those with a part directory).
    let mut have = Vec::new();
    for fam in &fams {
        if let Ok(parts) = read_yaml_map(&db_root.join(fam).join("mapping/parts.yaml")) {
            have.extend(parts.keys().filter(|k| db_root.join(fam).join(k).join("part.json").is_file()).cloned());
        }
    }
    Err(format!(
        "unknown part {name}: not in the Project X-Ray database at {} (families {}; {} parts, e.g. {})",
        db_root.display(),
        fams.join(", "),
        have.len(),
        have.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
    ))
}

/// A board: its part and its pin constraints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    /// Name used on the command line.
    pub name: &'static str,
    /// Other accepted names.
    pub aliases: &'static [&'static str],
    pub part: &'static str,
    /// Pin constraints, relative to the repository root.
    pub xdc: &'static str,
    /// The board's own name.
    pub title: &'static str,
}

/// Boards with constraints in the repository. Pins come from the vendors'
/// master constraint files and are checked against the database's package
/// pin tables by `tests/boards.rs`.
pub const BOARDS: &[Board] = &[
    Board { name: "zedboard", aliases: &["zed"], part: "xc7z020clg484-1", xdc: "boards/zedboard.xdc", title: "Avnet/Digilent ZedBoard" },
    Board { name: "arty-a7-35", aliases: &["arty", "arty-a7"], part: "xc7a35tcsg324-1", xdc: "boards/arty-a7-35.xdc", title: "Digilent Arty A7-35T" },
    Board { name: "basys3", aliases: &["basys-3"], part: "xc7a35tcpg236-1", xdc: "boards/basys3.xdc", title: "Digilent Basys 3" },
    Board { name: "nexys-a7-100t", aliases: &["nexys-a7", "nexys4-ddr"], part: "xc7a100tcsg324-1", xdc: "boards/nexys-a7-100t.xdc", title: "Digilent Nexys A7-100T" },
];

/// Look a board up by name or alias.
pub fn board(name: &str) -> Result<&'static Board, String> {
    let n = name.to_ascii_lowercase();
    BOARDS
        .iter()
        .find(|b| b.name == n || b.aliases.contains(&n.as_str()))
        .ok_or_else(|| format!("unknown board {name} (known: {})", BOARDS.iter().map(|b| b.name).collect::<Vec<_>>().join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_maps_parse() {
        let dir = std::env::temp_dir().join(format!("helion_parts_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("m.yaml");
        std::fs::write(&p, "# devices\n\"xc7a35t\":\n  fabric: \"xc7a50t\"\nxc7a35tcsg324-1:\n  device: xc7a35t\n  speedgrade: '1'\n").unwrap();
        let m = read_yaml_map(&p).unwrap();
        assert_eq!(m["xc7a35t"]["fabric"], "xc7a50t");
        assert_eq!(m["xc7a35tcsg324-1"]["speedgrade"], "1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn boards_resolve_by_alias() {
        assert_eq!(board("Arty").unwrap().part, "xc7a35tcsg324-1");
        assert_eq!(board("zed").unwrap().name, "zedboard");
        assert!(board("kc705").unwrap_err().contains("known: zedboard"));
    }
}
