//! RTL source attribution carried on netlist cells and nets.
//!
//! Elaborators attach `src` = `file:line` to the cells (and nets) they
//! produce from a source construct. When optimization merges cells (const
//! folding, inverter absorption) the surviving cell takes the union of both
//! locations, so `src` may hold several locations joined by `|`
//! (`a.sv:12|a.sv:14`), kept sorted and unique.
//!
//! [`LineProbe`] is the contract between an elaborator that instruments a
//! design for RTL line / branch coverage and a simulator that reads it back:
//! one 1-bit *probe net* per instrumented statement or branch arm, with the
//! probe metadata stored as net attributes (`lcov*`, see [`LineProbe::write`]).

use crate::Attrs;

/// Attribute key holding one or more `file:line` source locations.
pub const SRC_ATTR: &str = "src";

/// Separator between locations in a merged `src` value.
pub const SRC_SEP: char = '|';

/// One `file:line` source location (1-based line).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SrcLoc {
    pub file: String,
    pub line: u32,
}

impl SrcLoc {
    pub fn new(file: impl Into<String>, line: u32) -> Self {
        Self { file: file.into(), line }
    }

    /// Parse `file:line` (the file part may itself contain `:`).
    pub fn parse(s: &str) -> Option<Self> {
        let (file, line) = s.trim().rsplit_once(':')?;
        let line: u32 = line.parse().ok()?;
        if file.is_empty() || line == 0 {
            return None;
        }
        Some(Self::new(file, line))
    }
}

impl std::fmt::Display for SrcLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

impl Attrs {
    /// Every location in this object's `src` attribute (empty when unset or
    /// unparsable).
    pub fn src_locs(&self) -> Vec<SrcLoc> {
        self.get(SRC_ATTR)
            .map(|v| v.split(SRC_SEP).filter_map(SrcLoc::parse).collect())
            .unwrap_or_default()
    }

    /// Add `loc` to `src`, keeping the list sorted and unique.
    pub fn add_src(&mut self, loc: &SrcLoc) {
        let mut locs = self.src_locs();
        if !locs.contains(loc) {
            locs.push(loc.clone());
        }
        self.set_src_locs(locs);
    }

    /// Union `other`'s `src` into this one (cell merge during optimization).
    pub fn merge_src_from(&mut self, other: &Attrs) {
        let theirs = other.src_locs();
        if theirs.is_empty() {
            return;
        }
        let mut locs = self.src_locs();
        for l in theirs {
            if !locs.contains(&l) {
                locs.push(l);
            }
        }
        self.set_src_locs(locs);
    }

    fn set_src_locs(&mut self, mut locs: Vec<SrcLoc>) {
        locs.sort();
        locs.dedup();
        if locs.is_empty() {
            self.map.remove(SRC_ATTR);
            return;
        }
        let v: Vec<String> = locs.iter().map(SrcLoc::to_string).collect();
        self.set(SRC_ATTR, v.join(&SRC_SEP.to_string()));
    }
}

/// What a line-coverage probe net observes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineProbeKind {
    /// Procedural statement: the net is 1 exactly when control reaches the
    /// statement (the conjunction of every enclosing `if` / `case` condition).
    Stmt,
    /// One arm of an `if` (`true` / `false`) or `case` (an item or
    /// `default`): the net is 1 exactly when that arm is selected *and*
    /// control reaches the branch.
    Arm {
        /// Identifies the branch site; all arms of one `if` / `case` share it.
        branch: u64,
        /// `if` or `case`.
        construct: String,
        /// Arm position within the branch (0-based, source order).
        index: u32,
        /// `true`, `false`, the case item text, or `default`.
        label: String,
        /// Line of the `if` / `case` keyword (the probe's `src` is the arm's
        /// own line: a case item, or the `if` line itself).
        site: u32,
    },
    /// Continuous assignment: the net is a copy of one bit of the assigned
    /// signal; the line counts as active when that value changes.
    Assign,
}

/// Probe metadata read back from / written to a probe net's attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineProbe {
    pub src: SrcLoc,
    pub kind: LineProbeKind,
    /// Statement lives in an edge-triggered block: it executes on clock
    /// edges only, so it must only be sampled with pre-edge values.
    pub clocked: bool,
}

/// Probe kind attribute (`stmt`, `arm`, `assign`); presence marks a probe net.
pub const LCOV_ATTR: &str = "lcov";
const LCOV_BRANCH: &str = "lcov_branch";
const LCOV_CONSTRUCT: &str = "lcov_construct";
const LCOV_ARM: &str = "lcov_arm";
const LCOV_LABEL: &str = "lcov_label";
const LCOV_CLOCKED: &str = "lcov_clocked";
const LCOV_SITE: &str = "lcov_site";

impl LineProbe {
    /// Store this probe on a net's attributes (also sets `src`).
    pub fn write(&self, a: &mut Attrs) {
        a.add_src(&self.src);
        match &self.kind {
            LineProbeKind::Stmt => a.set(LCOV_ATTR, "stmt"),
            LineProbeKind::Assign => a.set(LCOV_ATTR, "assign"),
            LineProbeKind::Arm { branch, construct, index, label, site } => {
                a.set(LCOV_ATTR, "arm");
                a.set(LCOV_SITE, site.to_string());
                a.set(LCOV_BRANCH, branch.to_string());
                a.set(LCOV_CONSTRUCT, construct.clone());
                a.set(LCOV_ARM, index.to_string());
                a.set(LCOV_LABEL, label.clone());
            }
        }
        a.set(LCOV_CLOCKED, if self.clocked { "1" } else { "0" });
    }

    /// Read a probe back from net attributes (`None` for ordinary nets).
    pub fn read(a: &Attrs) -> Option<Self> {
        let src = a.src_locs().into_iter().next()?;
        let kind = match a.get(LCOV_ATTR)? {
            "stmt" => LineProbeKind::Stmt,
            "assign" => LineProbeKind::Assign,
            "arm" => LineProbeKind::Arm {
                branch: a.get(LCOV_BRANCH)?.parse().ok()?,
                construct: a.get(LCOV_CONSTRUCT).unwrap_or("if").to_string(),
                index: a.get(LCOV_ARM)?.parse().ok()?,
                label: a.get(LCOV_LABEL).unwrap_or("").to_string(),
                site: a.get(LCOV_SITE).and_then(|v| v.parse().ok()).unwrap_or(src.line),
            },
            _ => return None,
        };
        Some(Self { src, kind, clocked: a.flag(LCOV_CLOCKED) })
    }

    /// True when `a` marks a line-coverage probe net.
    pub fn is_probe(a: &Attrs) -> bool {
        a.get(LCOV_ATTR).is_some()
    }

    /// Probes whose condition is constantly true (a statement at the top of
    /// its block) need no net: they are stored on the design instead, one
    /// attribute per probe (`lcov_always.<n>`, see [`LineProbe::always_on`]).
    pub fn write_always(&self, design_attrs: &mut Attrs, n: u64) {
        let (kind, branch, construct, index, label, site) = match &self.kind {
            LineProbeKind::Stmt => ("stmt", 0, "", 0, String::new(), 0),
            LineProbeKind::Assign => ("assign", 0, "", 0, String::new(), 0),
            LineProbeKind::Arm { branch, construct, index, label, site } => {
                ("arm", *branch, construct.as_str(), *index, label.replace('\t', " "), *site)
            }
        };
        let v = format!(
            "{kind}\t{}\t{branch}\t{construct}\t{index}\t{label}\t{site}\t{}",
            u8::from(self.clocked),
            self.src
        );
        design_attrs.set(format!("{LCOV_ALWAYS}{n}"), v);
    }

    /// Every always-on probe stored on a design by [`LineProbe::write_always`].
    pub fn always_on(design_attrs: &Attrs) -> Vec<Self> {
        design_attrs
            .map
            .iter()
            .filter(|(k, _)| k.starts_with(LCOV_ALWAYS))
            .filter_map(|(_, v)| {
                let f: Vec<&str> = v.splitn(8, '\t').collect();
                if f.len() != 8 {
                    return None;
                }
                let kind = match f[0] {
                    "stmt" => LineProbeKind::Stmt,
                    "assign" => LineProbeKind::Assign,
                    "arm" => LineProbeKind::Arm {
                        branch: f[2].parse().ok()?,
                        construct: f[3].to_string(),
                        index: f[4].parse().ok()?,
                        label: f[5].to_string(),
                        site: f[6].parse().ok()?,
                    },
                    _ => return None,
                };
                Some(Self { src: SrcLoc::parse(f[7])?, kind, clocked: f[1] == "1" })
            })
            .collect()
    }
}

/// Design attribute key prefix of always-on probes.
pub const LCOV_ALWAYS: &str = "lcov_always.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn src_parse_and_display_round_trip() {
        let l = SrcLoc::parse("C:/rtl/top.sv:42").unwrap();
        assert_eq!(l, SrcLoc::new("C:/rtl/top.sv", 42));
        assert_eq!(l.to_string(), "C:/rtl/top.sv:42");
        assert!(SrcLoc::parse("top.sv").is_none());
        assert!(SrcLoc::parse("top.sv:0").is_none());
        assert!(SrcLoc::parse(":3").is_none());
    }

    #[test]
    fn add_and_merge_keep_sorted_unique_lists() {
        let mut a = Attrs::default();
        a.add_src(&SrcLoc::new("b.sv", 3));
        a.add_src(&SrcLoc::new("a.sv", 9));
        a.add_src(&SrcLoc::new("b.sv", 3));
        assert_eq!(a.get(SRC_ATTR), Some("a.sv:9|b.sv:3"));
        let mut b = Attrs::default();
        b.add_src(&SrcLoc::new("a.sv", 1));
        b.add_src(&SrcLoc::new("a.sv", 9));
        a.merge_src_from(&b);
        assert_eq!(a.get(SRC_ATTR), Some("a.sv:1|a.sv:9|b.sv:3"));
        let mut empty = Attrs::default();
        empty.merge_src_from(&Attrs::default());
        assert!(empty.get(SRC_ATTR).is_none());
    }

    #[test]
    fn line_probe_attrs_round_trip() {
        let p = LineProbe {
            src: SrcLoc::new("dut.sv", 7),
            kind: LineProbeKind::Arm {
                branch: 12,
                construct: "case".into(),
                index: 2,
                label: "2'd3".into(),
                site: 5,
            },
            clocked: true,
        };
        let mut a = Attrs::default();
        p.write(&mut a);
        assert!(LineProbe::is_probe(&a));
        assert_eq!(LineProbe::read(&a), Some(p));
        let s = LineProbe { src: SrcLoc::new("dut.sv", 3), kind: LineProbeKind::Stmt, clocked: false };
        let mut b = Attrs::default();
        s.write(&mut b);
        assert_eq!(LineProbe::read(&b), Some(s));
        assert_eq!(LineProbe::read(&Attrs::default()), None);
    }

    #[test]
    fn always_on_probes_round_trip_through_design_attrs() {
        let a = LineProbe { src: SrcLoc::new("d.sv", 5), kind: LineProbeKind::Stmt, clocked: true };
        let b = LineProbe {
            src: SrcLoc::new("d.sv", 9),
            kind: LineProbeKind::Arm { branch: 4, construct: "case".into(), index: 0, label: "a\tb".into(), site: 8 },
            clocked: false,
        };
        let mut d = Attrs::default();
        a.write_always(&mut d, 1);
        b.write_always(&mut d, 2);
        d.set("other", "x");
        let got = LineProbe::always_on(&d);
        assert_eq!(got.len(), 2);
        assert!(got.contains(&a));
        assert!(got.iter().any(|p| matches!(&p.kind, LineProbeKind::Arm { label, .. } if label == "a b")));
    }
}
