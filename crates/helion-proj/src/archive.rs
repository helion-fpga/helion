//! Archive Project (Vivado `archive_project`): one self-contained ZIP holding
//! the `.prj`, every design source, constraint file and `.helion` IP package
//! it references, with all paths rewritten relative to the archived project
//! so it opens anywhere after extraction. Optionally adds run results /
//! checkpoints from a directory.
//!
//! Layout inside the archive (`<name>/` prefix):
//! - `<name>.prj`            rewritten project file
//! - `srcs/<file>`           design sources (`read_sv`, `read_vhdl`, `add_files`)
//! - `constrs/<file>`        XDC/SDC (`read_xdc`, `read_sdc`)
//! - `ip/<core>/...`         IP packages, manifest rewritten to local files
//! - `runs/...`              optional run results / checkpoints
//! - `ARCHIVE.txt`           what was archived, and from where

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::zip::ZipWriter;
use crate::{load_prj, resolve_prj_path};

/// Options for [`archive_project`].
#[derive(Clone, Debug, Default)]
pub struct ArchiveOptions {
    /// Directory of run results / checkpoints to include under `runs/`.
    pub runs_dir: Option<PathBuf>,
}

/// What [`archive_project`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveReport {
    pub zip: PathBuf,
    pub entries: usize,
    pub bytes: u64,
    pub sources: usize,
    pub constraints: usize,
    pub ip_packages: usize,
    pub run_files: usize,
    /// Referenced files that did not exist (archived project would not open).
    pub missing: Vec<String>,
}

fn unique_name(used: &mut BTreeMap<String, usize>, dir: &str, base: &str) -> String {
    let n = used.entry(format!("{dir}/{base}")).or_insert(0);
    *n += 1;
    if *n == 1 {
        format!("{dir}/{base}")
    } else {
        let p = Path::new(base);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(base);
        match p.extension().and_then(|e| e.to_str()) {
            Some(ext) => format!("{dir}/{stem}_{}.{ext}", *n - 1),
            None => format!("{dir}/{base}_{}", *n - 1),
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.is_file() {
            out.push(p);
        }
    }
    Ok(())
}

fn strip_braces(t: &str) -> &str {
    t.trim_matches(|c: char| "{}[]".contains(c))
}

/// Archive `prj` into `zip_path`. Fails when the `.prj` itself is unreadable;
/// referenced files that are missing are listed in the report (and the
/// archive still records their original lines) rather than silently dropped.
pub fn archive_project(prj: &Path, zip_path: &Path, opts: &ArchiveOptions) -> Result<ArchiveReport, String> {
    let text = std::fs::read_to_string(prj).map_err(|e| format!("{}: {e}", prj.display()))?;
    let parsed = load_prj(&text)?;
    let name = parsed
        .project_name
        .clone()
        .or_else(|| prj.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into());
    let root = format!("{name}/");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut used = BTreeMap::new();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut missing = Vec::new();
    let (mut n_src, mut n_con, mut n_ip) = (0, 0, 0);

    let mut take = |given: &str, dir: &str, files: &mut Vec<(String, Vec<u8>)>| -> Option<String> {
        if let Some(rel) = map.get(given) {
            return Some(rel.clone());
        }
        let path = resolve_prj_path(prj, given);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let base = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into());
                let rel = unique_name(&mut used, dir, &base);
                files.push((format!("{root}{rel}"), bytes));
                map.insert(given.to_string(), rel.clone());
                Some(rel)
            }
            Err(_) => {
                missing.push(given.to_string());
                None
            }
        }
    };

    for s in &parsed.sources {
        if take(s, "srcs", &mut files).is_some() {
            n_src += 1;
        }
    }
    for c in &parsed.constraint_files {
        if take(c, "constrs", &mut files).is_some() {
            n_con += 1;
        }
    }
    // IP packages: the manifest plus each listed file, flattened into
    // ip/<core>/ with the manifest's file lines rewritten to match.
    for ip in &parsed.ip_packages {
        let path = resolve_prj_path(prj, ip);
        let Ok(pkg) = helion_ipxact::load_helion(&path) else {
            missing.push(ip.clone());
            continue;
        };
        let dir = format!("ip/{}", pkg.name);
        let mut manifest = String::new();
        let mtext = std::fs::read_to_string(&pkg.manifest_path).unwrap_or_default();
        let resolved: Vec<PathBuf> = pkg
            .resolve_files()
            .unwrap_or_default()
            .into_iter()
            .chain(pkg.resolve_constraints().unwrap_or_default())
            .collect();
        let listed: Vec<&String> = pkg.files.iter().chain(pkg.constraints.iter()).collect();
        let mut local: BTreeMap<String, String> = BTreeMap::new();
        for (orig, p) in listed.iter().zip(resolved.iter()) {
            if let Ok(bytes) = std::fs::read(p) {
                let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let rel = unique_name(&mut used, &dir, &base);
                files.push((format!("{root}{rel}"), bytes));
                local.insert((*orig).clone(), rel.trim_start_matches(&format!("{dir}/")).to_string());
            } else {
                missing.push(p.display().to_string());
            }
        }
        for line in mtext.lines() {
            let mut toks = line.split_whitespace();
            let verb = toks.next().unwrap_or("");
            let arg = toks.next().unwrap_or("");
            match local.get(arg) {
                Some(l) if !verb.starts_with('#') => manifest.push_str(&format!("{verb} {l}\n")),
                _ => {
                    manifest.push_str(line);
                    manifest.push('\n');
                }
            }
        }
        let mname = pkg
            .manifest_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "package.helion".into());
        let rel = format!("{dir}/{mname}");
        files.push((format!("{root}{rel}"), manifest.into_bytes()));
        map.insert(ip.clone(), rel);
        n_ip += 1;
    }

    // Optional run results / checkpoints.
    let mut run_files = 0;
    let mut ckpt_map: BTreeMap<String, String> = BTreeMap::new();
    if let Some(runs) = &opts.runs_dir
        && runs.is_dir()
    {
        let mut all = Vec::new();
        walk(runs, &mut all)?;
        for p in all {
            let rel = p.strip_prefix(runs).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            files.push((format!("{root}runs/{rel}"), bytes));
            run_files += 1;
            ckpt_map.insert(p.display().to_string(), format!("runs/{rel}"));
        }
    }

    // Rewrite the .prj line by line.
    let mut out = format!(
        "# Archived by Helion archive_project from {}\n",
        prj.display()
    );
    for line in text.lines() {
        let body = line.split('#').next().unwrap_or("").trim();
        let mut toks = body.split_whitespace();
        let verb = toks.next().unwrap_or("");
        let rewritten = match verb {
            "create_project" => Some(format!("create_project {name}")),
            "read_sv" | "read_vhdl" | "read_c" | "read_verilog" | "sv" | "vhdl" | "c" | "read_xdc"
            | "read_sdc" | "xdc" | "sdc" | "read_ip" | "ip" | "add_files" => {
                let mut parts = vec![verb.to_string()];
                for t in toks {
                    let key = strip_braces(t);
                    parts.push(map.get(key).cloned().unwrap_or_else(|| t.to_string()));
                }
                Some(parts.join(" "))
            }
            "write_checkpoint" => {
                let arg = toks.next().unwrap_or("");
                let abs = resolve_prj_path(prj, arg).display().to_string();
                Some(match ckpt_map.get(&abs) {
                    Some(rel) => format!("write_checkpoint {rel}"),
                    None => format!("# write_checkpoint {arg} (not archived)"),
                })
            }
            _ => None,
        };
        out.push_str(&rewritten.unwrap_or_else(|| line.to_string()));
        out.push('\n');
    }
    let prj_name = format!("{name}.prj");
    files.insert(0, (format!("{root}{prj_name}"), out.into_bytes()));
    let mut info = format!(
        "Helion project archive\nproject: {name}\nsource prj: {}\npart: {}\nsources: {n_src}\nconstraints: {n_con}\nip packages: {n_ip}\nrun files: {run_files}\n",
        prj.display(),
        parsed.part
    );
    for m in &missing {
        info.push_str(&format!("MISSING: {m}\n"));
    }
    files.push((format!("{root}ARCHIVE.txt"), info.into_bytes()));

    if let Some(parent) = zip_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let f = std::fs::File::create(zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let mut z = ZipWriter::new(std::io::BufWriter::new(f));
    for (n, data) in &files {
        z.add(n, data)?;
    }
    z.finish()?;
    let bytes = std::fs::metadata(zip_path).map(|m| m.len()).unwrap_or(0);
    Ok(ArchiveReport {
        zip: zip_path.to_path_buf(),
        entries: files.len(),
        bytes,
        sources: n_src,
        constraints: n_con,
        ip_packages: n_ip,
        run_files,
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_extracts_to_an_openable_relative_project() {
        let tmp = std::env::temp_dir().join(format!("helion-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src_dir = tmp.join("elsewhere/rtl");
        std::fs::create_dir_all(&src_dir).unwrap();
        let proj = tmp.join("proj");
        std::fs::create_dir_all(proj.join("runs/impl_1")).unwrap();
        std::fs::write(src_dir.join("top.v"), "module top(input a, output y); assign y = a; endmodule\n").unwrap();
        std::fs::write(src_dir.join("pins.xdc"), "set_property PACKAGE_PIN P1 [get_ports a]\n").unwrap();
        std::fs::write(proj.join("runs/impl_1/top.bit"), [1u8, 2, 3]).unwrap();
        let prj = proj.join("demo.prj");
        std::fs::write(
            &prj,
            format!(
                "create_project demo {}\npart HL10T-C32-1\ntop top\nread_sv {}\nread_xdc {}\nread_sv {}\n",
                proj.display(),
                src_dir.join("top.v").display(),
                src_dir.join("pins.xdc").display(),
                tmp.join("gone.v").display()
            ),
        )
        .unwrap();
        let zip = tmp.join("out/demo.zip");
        let rep = archive_project(
            &prj,
            &zip,
            &ArchiveOptions {
                runs_dir: Some(proj.join("runs")),
            },
        )
        .unwrap();
        assert_eq!((rep.sources, rep.constraints, rep.run_files), (1, 1, 1));
        assert_eq!(rep.missing.len(), 1, "missing source reported, not dropped silently");
        let x = tmp.join("x");
        crate::zip::extract_zip(&std::fs::read(&zip).unwrap(), &x).unwrap();
        let text = std::fs::read_to_string(x.join("demo/demo.prj")).unwrap();
        assert!(text.contains("read_sv srcs/top.v"), "{text}");
        assert!(text.contains("read_xdc constrs/pins.xdc"));
        assert!(!text.contains(&src_dir.display().to_string()), "no absolute source paths");
        let parsed = load_prj(&text).unwrap();
        let prj2 = x.join("demo/demo.prj");
        assert!(resolve_prj_path(&prj2, &parsed.sources[0]).is_file());
        assert!(x.join("demo/runs/impl_1/top.bit").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ip_packages_are_flattened_with_rewritten_manifests() {
        let tmp = std::env::temp_dir().join(format!("helion-archive-ip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let ip = helion_ipxact::ip_repo_dir().join("h_rv32_hb1/h_rv32_hb1.helion");
        let prj = tmp.join("cpu.prj");
        std::fs::write(&prj, format!("part HL10T-C32-1\nread_ip {}\n", ip.display())).unwrap();
        let zip = tmp.join("cpu.zip");
        let rep = archive_project(&prj, &zip, &ArchiveOptions::default()).unwrap();
        assert_eq!(rep.ip_packages, 1);
        let x = tmp.join("x");
        crate::zip::extract_zip(&std::fs::read(&zip).unwrap(), &x).unwrap();
        let pkg = helion_ipxact::load_helion(&x.join("cpu/ip/h_rv32_hb1/h_rv32_hb1.helion")).unwrap();
        assert_eq!(pkg.resolve_files().unwrap().len(), 2, "wrapper + picorv32 copied locally");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
