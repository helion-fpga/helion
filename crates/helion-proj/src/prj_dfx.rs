//! `.prj` persistence for UG909 reconfigurable partitions.
//!
//! A [`PrjPartition`] names a reconfigurable partition, the `.prj` pblock
//! it occupies, the static-design instance it swaps, and its
//! reconfigurable module variants (each a list of HDL files). The project
//! file carries them as Tcl-shaped lines, replayed in order like the
//! pblock verbs, so a project can append edits and reopen with the same
//! partitions:
//!
//! ```text
//! create_partition rp0 -pblock pblock_rp0 -instance u_rm
//! add_reconfig_module rp0 rm_inv rtl/rm_inv.v
//! add_reconfig_module rp0 rm_buf {rtl/rm buf.v}
//! delete_reconfig_module rp0 rm_buf
//! delete_partition rp0
//! ```
//!
//! `create_partition` / `set_partition` upsert (a later line updates the
//! pblock or instance); `add_reconfig_module` replaces a module of the
//! same name. Words containing whitespace are brace-quoted. [`to_dfx`]
//! resolves a partition against the project's pblocks and the `.prj`
//! location into the [`crate::dfx::DfxPartition`] that `run_dfx` consumes.
//!
//! [`to_dfx`]: PrjPartition::to_dfx

use std::path::{Path, PathBuf};

use crate::PrjPblock;
use crate::dfx::{DfxModule, DfxPartition};

/// One reconfigurable module variant as stored in the `.prj`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrjReconfigModule {
    pub name: String,
    /// HDL files as written (relative to the `.prj` or absolute).
    pub sources: Vec<String>,
}

/// A reconfigurable partition as stored in the `.prj`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrjPartition {
    pub name: String,
    /// Ranged `.prj` pblock the partition occupies.
    pub pblock: Option<String>,
    /// Static-design hierarchy instance that is reconfigured.
    pub instance: Option<String>,
    pub modules: Vec<PrjReconfigModule>,
}

/// `.prj` verbs this module parses.
pub const VERBS: &[&str] = &[
    "create_partition",
    "set_partition",
    "delete_partition",
    "add_reconfig_module",
    "delete_reconfig_module",
];

/// Split a line into words, honoring `{…}` grouping (no nesting needed).
pub fn split_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut braced = false;
    for ch in line.chars() {
        match ch {
            '{' => {
                if depth > 0 {
                    cur.push(ch);
                }
                depth += 1;
                braced = true;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth > 0 {
                    cur.push(ch);
                }
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() || braced {
                    out.push(std::mem::take(&mut cur));
                }
                braced = false;
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || braced {
        out.push(cur);
    }
    out
}

/// Brace-quote a word that contains whitespace (or is empty).
pub fn quote(word: &str) -> String {
    if word.is_empty() || word.chars().any(char::is_whitespace) {
        format!("{{{word}}}")
    } else {
        word.to_string()
    }
}

fn partition_mut<'a>(parts: &'a mut Vec<PrjPartition>, name: &str) -> &'a mut PrjPartition {
    if let Some(i) = parts.iter().position(|p| p.name == name) {
        return &mut parts[i];
    }
    parts.push(PrjPartition {
        name: name.to_string(),
        ..Default::default()
    });
    parts.last_mut().unwrap()
}

/// `-` or empty clears an optional field.
fn opt(v: &str) -> Option<String> {
    (!v.is_empty() && v != "-").then(|| v.to_string())
}

/// Apply one `.prj` line. `Ok(false)` when the line is not a DFX verb.
pub fn apply(parts: &mut Vec<PrjPartition>, line: &str) -> Result<bool, String> {
    let words = split_words(line);
    let Some(cmd) = words.first().map(String::as_str) else {
        return Ok(false);
    };
    if !VERBS.contains(&cmd) {
        return Ok(false);
    }
    let args = &words[1..];
    let name = args
        .first()
        .filter(|a| !a.starts_with('-'))
        .ok_or_else(|| format!("{cmd}: need a partition name"))?;
    match cmd {
        "create_partition" | "set_partition" => {
            let mut pblock = None;
            let mut instance = None;
            let mut positional = Vec::new();
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "-pblock" | "-instance" => {
                        let v = args
                            .get(i + 1)
                            .ok_or_else(|| format!("{cmd}: {} needs a value", args[i]))?;
                        if args[i] == "-pblock" {
                            pblock = Some(opt(v));
                        } else {
                            instance = Some(opt(v));
                        }
                        i += 2;
                        continue;
                    }
                    a if a.starts_with('-') && a != "-" => return Err(format!("{cmd}: unknown option {a}")),
                    a => positional.push(a.to_string()),
                }
                i += 1;
            }
            if let Some(v) = positional.first() {
                pblock.get_or_insert(opt(v));
            }
            if let Some(v) = positional.get(1) {
                instance.get_or_insert(opt(v));
            }
            let p = partition_mut(parts, name);
            if let Some(v) = pblock {
                p.pblock = v;
            }
            if let Some(v) = instance {
                p.instance = v;
            }
        }
        "delete_partition" => {
            for n in args.iter().filter(|a| !a.starts_with('-')) {
                parts.retain(|p| &p.name != n);
            }
        }
        "add_reconfig_module" => {
            let module = args
                .get(1)
                .ok_or_else(|| format!("{cmd}: need a module name"))?
                .clone();
            let sources: Vec<String> = args[2..].iter().filter(|a| !a.is_empty()).cloned().collect();
            if sources.is_empty() {
                return Err(format!("{cmd}: {module} needs at least one HDL file"));
            }
            let p = partition_mut(parts, name);
            match p.modules.iter_mut().find(|m| m.name == module) {
                Some(m) => m.sources = sources,
                None => p.modules.push(PrjReconfigModule {
                    name: module,
                    sources,
                }),
            }
        }
        "delete_reconfig_module" => {
            let module = args
                .get(1)
                .ok_or_else(|| format!("{cmd}: need a module name"))?;
            if let Some(p) = parts.iter_mut().find(|p| &p.name == name) {
                p.modules.retain(|m| &m.name != module);
            }
        }
        _ => unreachable!(),
    }
    Ok(true)
}

/// A deleted pblock no longer backs any partition.
pub fn release_pblock(parts: &mut [PrjPartition], pblock: &str) {
    for p in parts.iter_mut() {
        if p.pblock.as_deref() == Some(pblock) {
            p.pblock = None;
        }
    }
}

impl PrjPartition {
    /// The `create_partition` line for this partition's binding.
    pub fn line(&self) -> String {
        let mut out = format!("create_partition {}", quote(&self.name));
        if let Some(pb) = &self.pblock {
            out.push_str(&format!(" -pblock {}", quote(pb)));
        }
        if let Some(i) = &self.instance {
            out.push_str(&format!(" -instance {}", quote(i)));
        }
        out
    }

    /// The `add_reconfig_module` line for `m`.
    pub fn module_line(&self, m: &PrjReconfigModule) -> String {
        let mut out = format!("add_reconfig_module {} {}", quote(&self.name), quote(&m.name));
        for s in &m.sources {
            out.push(' ');
            out.push_str(&quote(s));
        }
        out
    }

    /// Resolve against the project's pblocks and the `.prj` location
    /// (relative module sources resolve like `read_sv`).
    pub fn to_dfx(&self, pblocks: &[PrjPblock], prj_path: Option<&Path>) -> Result<DfxPartition, String> {
        let pb_name = self
            .pblock
            .as_ref()
            .ok_or_else(|| format!("partition {}: no pblock", self.name))?;
        let pblock = pblocks
            .iter()
            .find(|p| &p.name == pb_name)
            .cloned()
            .ok_or_else(|| format!("partition {}: no pblock {pb_name}", self.name))?;
        if !pblock.ranged {
            return Err(format!("partition {}: pblock {pb_name} has no range", self.name));
        }
        let instance = self
            .instance
            .clone()
            .ok_or_else(|| format!("partition {}: no instance", self.name))?;
        let modules = self
            .modules
            .iter()
            .map(|m| DfxModule {
                name: m.name.clone(),
                sources: m
                    .sources
                    .iter()
                    .map(|s| match prj_path {
                        Some(p) => crate::resolve_prj_path(p, s),
                        None => PathBuf::from(s),
                    })
                    .collect(),
            })
            .collect();
        Ok(DfxPartition {
            name: self.name.clone(),
            pblock,
            instance,
            modules,
        })
    }
}

/// `.prj` text for `parts` (one binding line plus one line per module).
pub fn format(parts: &[PrjPartition]) -> String {
    let mut out = String::new();
    for p in parts {
        out.push_str(&p.line());
        out.push('\n');
        for m in &p.modules {
            out.push_str(&p.module_line(m));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_honor_braces() {
        assert_eq!(
            split_words("add_reconfig_module rp0 m {a b.v} c.v {}"),
            vec!["add_reconfig_module", "rp0", "m", "a b.v", "c.v", ""]
        );
    }

    #[test]
    fn verbs_upsert_replace_and_delete() {
        let mut parts = Vec::new();
        assert!(!apply(&mut parts, "create_pblock pb0").unwrap());
        apply(&mut parts, "create_partition rp0 pb0 u_rm").unwrap();
        assert_eq!(parts[0].pblock.as_deref(), Some("pb0"));
        assert_eq!(parts[0].instance.as_deref(), Some("u_rm"));
        apply(&mut parts, "set_partition rp0 -instance u_other").unwrap();
        assert_eq!(parts[0].pblock.as_deref(), Some("pb0"), "unnamed fields stay");
        assert_eq!(parts[0].instance.as_deref(), Some("u_other"));
        apply(&mut parts, "add_reconfig_module rp0 inv a.v").unwrap();
        apply(&mut parts, "add_reconfig_module rp0 inv b.v {c d.v}").unwrap();
        assert_eq!(parts[0].modules.len(), 1);
        assert_eq!(parts[0].modules[0].sources, vec!["b.v", "c d.v"]);
        assert!(apply(&mut parts, "add_reconfig_module rp0 empty").is_err());
        apply(&mut parts, "add_reconfig_module rp1 buf x.v").unwrap();
        assert_eq!(parts.len(), 2, "a module line creates its partition");
        apply(&mut parts, "delete_reconfig_module rp0 inv").unwrap();
        assert!(parts[0].modules.is_empty());
        apply(&mut parts, "create_partition rp0 -pblock -").unwrap();
        assert_eq!(parts[0].pblock, None, "`-` clears");
        apply(&mut parts, "delete_partition rp1").unwrap();
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn to_dfx_resolves_pblock_and_sources() {
        let pb = PrjPblock {
            name: "pb0".into(),
            x0: 1,
            y0: 1,
            x1: 2,
            y1: 2,
            ranged: true,
            cells: vec![],
        };
        let p = PrjPartition {
            name: "rp0".into(),
            pblock: Some("pb0".into()),
            instance: Some("u_rm".into()),
            modules: vec![PrjReconfigModule {
                name: "inv".into(),
                sources: vec!["/abs/inv.v".into()],
            }],
        };
        let d = p.to_dfx(std::slice::from_ref(&pb), None).unwrap();
        assert_eq!(d.pblock, pb);
        assert_eq!(d.modules[0].sources, vec![PathBuf::from("/abs/inv.v")]);
        assert!(p.to_dfx(&[], None).unwrap_err().contains("no pblock pb0"));
        let unranged = PrjPblock { ranged: false, ..pb };
        assert!(p.to_dfx(&[unranged], None).unwrap_err().contains("no range"));
    }
}
