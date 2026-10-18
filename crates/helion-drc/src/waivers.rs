//! Vivado UG906 `create_waiver` / `report_waivers` Engine & Actionable Tcl/RTL Remediation Generator.

use crate::taxonomy::lookup_rule;
use crate::{Drc, DrcSeverity, DrcViolation};

/// Single DRC / Methodology / CDC waiver entry created via [`WaiverSet::create_waiver`]
/// or `create_waiver -id ... -objects ... -description ...`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrcWaiver {
    pub waiver_id: usize,
    /// Rule ID (e.g. `"TIMING-18"`, `"CDC-1"`, or wildcard `"TIMING-*"`).
    pub rule_id: String,
    /// Target object patterns (empty or `["*"]` matches all objects for `rule_id`).
    pub objects: Vec<String>,
    /// Mandatory justification string (`-description`).
    pub description: String,
    /// Author / user tag (`-user`, default `"helion"`).
    pub user: String,
    /// Number of violations suppressed by this waiver during [`WaiverSet::apply`].
    pub hit_count: usize,
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p = pattern.trim();
    let t = text.trim();
    if p.is_empty() || p == "*" {
        return true;
    }
    if !p.contains('*') {
        return t == p
            || t.split_whitespace().any(|tok| tok == p)
            || t.contains(p);
    }
    // Simple glob match supporting leading, trailing, or middle '*'.
    let parts: Vec<&str> = p.split('*').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return true;
    }
    let mut rem = t;
    for (idx, part) in parts.iter().enumerate() {
        if idx == 0 && !p.starts_with('*') {
            if !rem.starts_with(part) {
                return false;
            }
            rem = &rem[part.len()..];
        } else if let Some(pos) = rem.find(part) {
            rem = &rem[pos + part.len()..];
        } else {
            return false;
        }
    }
    if !p.ends_with('*') {
        if let Some(last) = parts.last() {
            return t.ends_with(last);
        }
    }
    true
}

impl DrcWaiver {
    /// Returns true if `violation` matches this waiver's `rule_id` and `objects` filter.
    pub fn matches(&self, violation: &DrcViolation) -> bool {
        let rule_pat = self.rule_id.to_ascii_uppercase();
        let v_id = violation.id.to_ascii_uppercase();
        let rule_ok = if let Some(prefix) = rule_pat.strip_suffix('*') {
            v_id.starts_with(prefix)
        } else {
            v_id == rule_pat
        };
        if !rule_ok {
            return false;
        }
        if self.objects.is_empty() || self.objects.iter().any(|o| o == "*") {
            return true;
        }
        self.objects
            .iter()
            .any(|pat| wildcard_match(pat, &violation.objects))
    }

    /// Format as an XDC/Tcl `create_waiver` command.
    pub fn to_tcl_command(&self) -> String {
        let objs = if self.objects.is_empty() {
            "*".to_string()
        } else {
            self.objects.join(" ")
        };
        format!(
            "create_waiver -id {} -objects {{{}}} -user {} -description \"{}\"",
            self.rule_id,
            objs,
            self.user,
            self.description.replace('"', "\\\"")
        )
    }
}

/// A violation suppressed by a [`DrcWaiver`].
#[derive(Clone, Debug)]
pub struct WaivedViolation {
    pub violation: DrcViolation,
    pub waiver_id: usize,
    pub waiver_description: String,
}

/// Result of applying a [`WaiverSet`] to a [`Drc`] report.
#[derive(Clone, Debug)]
pub struct WaivedDrcReport {
    /// Remaining unwaived violations.
    pub active: Drc,
    /// Violations suppressed by matching waivers.
    pub waived: Vec<WaivedViolation>,
}

/// Collection of DRC / Methodology / CDC waivers (`create_waiver` / `report_waivers`).
#[derive(Clone, Debug, Default)]
pub struct WaiverSet {
    pub waivers: Vec<DrcWaiver>,
}

impl WaiverSet {
    pub fn new() -> Self {
        Self {
            waivers: Vec::new(),
        }
    }

    /// Programmatic `create_waiver -id <rule_id> -objects <objects> -description <description>`.
    pub fn create_waiver(
        &mut self,
        rule_id: &str,
        objects: Vec<String>,
        description: &str,
    ) -> Result<usize, String> {
        self.create_waiver_with_user(rule_id, objects, description, "helion")
    }

    pub fn create_waiver_with_user(
        &mut self,
        rule_id: &str,
        objects: Vec<String>,
        description: &str,
        user: &str,
    ) -> Result<usize, String> {
        let rule_id = rule_id.trim().to_ascii_uppercase();
        if rule_id.is_empty() {
            return Err("create_waiver: -id cannot be empty".into());
        }
        // Validate rule_id exists in taxonomy unless wildcard `*-*`.
        if !rule_id.contains('*') && lookup_rule(&rule_id).is_none() {
            return Err(format!(
                "create_waiver: unknown rule ID '{rule_id}' in UG906/UG949 catalog"
            ));
        }
        let desc = description.trim();
        if desc.is_empty() {
            return Err("create_waiver: -description is required by UG906 methodology".into());
        }
        let waiver_id = self.waivers.len() + 1;
        self.waivers.push(DrcWaiver {
            waiver_id,
            rule_id,
            objects,
            description: desc.to_string(),
            user: user.trim().to_string(),
            hit_count: 0,
        });
        Ok(waiver_id)
    }

    /// Parse and execute a Vivado Tcl `create_waiver` command string:
    /// `create_waiver -id TIMING-18 -objects {led clk} -description "Async board LED"`
    pub fn parse_create_waiver_tcl(&mut self, tcl_cmd: &str) -> Result<usize, String> {
        let tokens = tokenize_tcl(tcl_cmd);
        if tokens.is_empty() {
            return Err("create_waiver: empty command".into());
        }
        let head = tokens[0].as_str();
        if head != "create_waiver" {
            return Err(format!(
                "create_waiver: expected 'create_waiver', got '{head}'"
            ));
        }
        let mut id: Option<String> = None;
        let mut objects: Vec<String> = Vec::new();
        let mut description: Option<String> = None;
        let mut user = "helion".to_string();

        let mut i = 1usize;
        while i < tokens.len() {
            match tokens[i].as_str() {
                "-id" | "-of_objects" => {
                    i += 1;
                    let val = tokens
                        .get(i)
                        .ok_or("create_waiver: missing value after -id")?;
                    id = Some(val.clone());
                }
                "-objects" => {
                    i += 1;
                    let raw = tokens
                        .get(i)
                        .ok_or("create_waiver: missing value after -objects")?;
                    let cleaned = strip_tcl_get_wrapper(raw);
                    for item in cleaned.split_whitespace() {
                        if !item.is_empty() {
                            objects.push(item.to_string());
                        }
                    }
                }
                "-description" | "-desc" => {
                    i += 1;
                    let val = tokens
                        .get(i)
                        .ok_or("create_waiver: missing value after -description")?;
                    description = Some(val.clone());
                }
                "-user" => {
                    i += 1;
                    if let Some(u) = tokens.get(i) {
                        user = u.clone();
                    }
                }
                _ => {}
            }
            i += 1;
        }

        let rule_id = id.ok_or("create_waiver: missing required -id <rule_id>")?;
        let desc =
            description.ok_or("create_waiver: missing required -description <text>")?;
        self.create_waiver_with_user(&rule_id, objects, &desc, &user)
    }

    /// Filter `drc` against all registered waivers, incrementing waiver `hit_count`s
    /// and returning both the active unwaived [`Drc`] and the list of waived violations.
    pub fn apply(&mut self, drc: &Drc) -> WaivedDrcReport {
        let mut active = Drc::default();
        let mut waived = Vec::new();

        for item in &drc.items {
            if let Some(w) = self.waivers.iter_mut().find(|w| w.matches(item)) {
                w.hit_count += 1;
                waived.push(WaivedViolation {
                    violation: item.clone(),
                    waiver_id: w.waiver_id,
                    waiver_description: w.description.clone(),
                });
            } else {
                active.add_with_severity(
                    &item.id,
                    item.severity,
                    item.objects.clone(),
                    item.message.clone(),
                );
            }
        }

        WaivedDrcReport { active, waived }
    }

    /// Format a Vivado UG906 `report_waivers` summary report.
    pub fn report_waivers(&self) -> String {
        let total_hits: usize = self.waivers.iter().map(|w| w.hit_count).sum();
        let mut lines = vec![format!(
            "report_waivers total_waivers={} total_suppressed={}",
            self.waivers.len(),
            total_hits
        )];
        for w in &self.waivers {
            let objs = if w.objects.is_empty() {
                "*".to_string()
            } else {
                w.objects.join(",")
            };
            lines.push(format!(
                "WAIVER#{} id={} objects={} hits={} user={} description=\"{}\"",
                w.waiver_id, w.rule_id, objs, w.hit_count, w.user, w.description
            ));
        }
        lines.join("\n")
    }

    /// Export all waivers as an executable XDC/Tcl script.
    pub fn to_tcl_script(&self) -> String {
        let mut s = String::from("# Helion DRC / Methodology Waivers (UG906)\n");
        for w in &self.waivers {
            s.push_str(&w.to_tcl_command());
            s.push('\n');
        }
        s
    }
}

fn tokenize_tcl(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '"' {
            chars.next();
            let mut s = String::new();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next_c) = chars.next() {
                        s.push(next_c);
                    }
                } else if c == '"' {
                    break;
                } else {
                    s.push(c);
                }
            }
            out.push(s);
        } else if ch == '{' || ch == '[' {
            let open = ch;
            let close = if open == '{' { '}' } else { ']' };
            chars.next();
            let mut depth = 1usize;
            let mut s = String::new();
            while let Some(c) = chars.next() {
                if c == open {
                    depth += 1;
                    s.push(c);
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    s.push(c);
                } else {
                    s.push(c);
                }
            }
            out.push(s);
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                s.push(c);
                chars.next();
            }
            out.push(s);
        }
    }
    out
}

fn strip_tcl_get_wrapper(raw: &str) -> String {
    let t = raw.trim();
    for prefix in [
        "get_ports",
        "get_cells",
        "get_nets",
        "get_pins",
        "get_clocks",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let inner = rest.trim().trim_start_matches('{').trim_end_matches('}');
            return inner.trim().to_string();
        }
    }
    t.trim_start_matches('{')
        .trim_end_matches('}')
        .trim()
        .to_string()
}

/// Actionable Tcl and RTL remediation item for a single [`DrcViolation`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViolationRemediation {
    pub rule_id: String,
    pub severity: DrcSeverity,
    pub objects: String,
    pub title: String,
    pub ug_reference: &'static str,
    /// Concrete XDC/Tcl commands to resolve the violation.
    pub tcl_fix_commands: Vec<String>,
    /// RTL modification recommendation.
    pub rtl_guidance: String,
    /// Ready-to-run `create_waiver` command if the violation is intentional.
    pub waiver_tcl: String,
}

/// Complete remediation plan and Tcl fix script for a [`Drc`] report.
#[derive(Clone, Debug)]
pub struct RemediationReport {
    pub items: Vec<ViolationRemediation>,
}

impl RemediationReport {
    /// Emit a standalone executable `.tcl` / `.xdc` remediation script.
    pub fn to_tcl_script(&self) -> String {
        let mut s = String::from(
            "# Helion Automatic DRC & UG949 Methodology Remediation Script\n",
        );
        for (idx, item) in self.items.iter().enumerate() {
            s.push_str(&format!(
                "\n# [{}] {} ({}, {}) on objects '{}': {}\n",
                idx + 1,
                item.rule_id,
                item.severity.as_str(),
                item.ug_reference,
                if item.objects.is_empty() {
                    "-"
                } else {
                    &item.objects
                },
                item.title
            ));
            s.push_str(&format!("# RTL Guidance: {}\n", item.rtl_guidance));
            for cmd in &item.tcl_fix_commands {
                s.push_str(cmd);
                s.push('\n');
            }
            s.push_str(&format!("# Or waive if intentional: {}\n", item.waiver_tcl));
        }
        s
    }
}

/// Build an actionable [`ViolationRemediation`] for a single [`DrcViolation`].
pub fn remediate_violation(v: &DrcViolation) -> ViolationRemediation {
    let obj_target = if v.objects.trim().is_empty() {
        "*".to_string()
    } else {
        v.objects.trim().to_string()
    };

    if let Some(def) = lookup_rule(&v.id) {
        let tcl_expanded = def.tcl_remediation_template.replace("<OBJ>", &obj_target);
        let tcl_fix_commands: Vec<String> = tcl_expanded
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let waiver_tcl = format!(
            "create_waiver -id {} -objects {{{}}} -description \"Reviewed {}: {}\"",
            def.id, obj_target, def.id, def.title
        );
        ViolationRemediation {
            rule_id: def.id,
            severity: v.severity,
            objects: v.objects.clone(),
            title: def.title,
            ug_reference: def.ug_reference,
            tcl_fix_commands,
            rtl_guidance: def.rtl_remediation_hint,
            waiver_tcl,
        }
    } else {
        let waiver_tcl = format!(
            "create_waiver -id {} -objects {{{}}} -description \"Reviewed {}\"",
            v.id, obj_target, v.id
        );
        ViolationRemediation {
            rule_id: v.id.clone(),
            severity: v.severity,
            objects: v.objects.clone(),
            title: "Custom DRC check".to_string(),
            ug_reference: "UG906",
            tcl_fix_commands: vec![format!("# Inspect object {{{obj_target}}} for {}", v.id)],
            rtl_guidance: "Review netlist connectivity and XDC constraints.".to_string(),
            waiver_tcl,
        }
    }
}

/// Generate an actionable [`RemediationReport`] covering every violation in `drc`.
pub fn generate_remediation_script(drc: &Drc) -> RemediationReport {
    let items = drc.items.iter().map(remediate_violation).collect();
    RemediationReport { items }
}
