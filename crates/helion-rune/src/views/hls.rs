//! High-Level Synthesis model: the open C/C++ kernel, its directives
//! (inline `#pragma HLS` lines), the C synthesis and C/RTL co-simulation
//! workers, and the actions the HLS pane and Tcl verbs (`csynth_design`,
//! `cosim_design`, `add_hls_rtl`) drive.
//!
//! Engine work is `helion_hls::hls_compile_with_config` (CDFG, schedule,
//! binding, SystemVerilog, report) and `helion_hls::run_rtl_cosim_with`
//! (the synthesized netlist simulated in helion-sim against the C
//! reference). Both run on a worker thread; [`poll`] folds results back.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev, ToastKind};

/// HLS result tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HlsTab {
    #[default]
    Report,
    Schedule,
    Rtl,
    Cosim,
}

/// What the worker runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HlsJob {
    Synth,
    Cosim,
}

impl HlsJob {
    pub fn busy(self) -> &'static str {
        match self {
            HlsJob::Synth => "C synthesis is running",
            HlsJob::Cosim => "C/RTL co-simulation is running",
        }
    }
}

enum HlsOut {
    Synth(Result<Box<helion_hls::HlsResult>, String>),
    Cosim(Result<Box<helion_hls::CosimReport>, String>),
}

#[derive(Clone)]
struct Active {
    job: HlsJob,
    rx: Arc<Mutex<mpsc::Receiver<HlsOut>>>,
}

impl std::fmt::Debug for Active {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Active").field("job", &self.job).finish()
    }
}

/// One inline directive line found in the kernel source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectiveRow {
    /// 0-based source line.
    pub line: usize,
    pub kind: String,
    pub options: String,
    /// Enclosing function / loop context (`fir`, `fir/loop@L7`).
    pub scope: String,
    /// Parse error from `helion_hls::parse_pragma_line`.
    pub error: Option<String>,
}

/// HLS pane state.
#[derive(Clone, Debug)]
pub struct HlsState {
    pub source: rune::TextEdit,
    pub file: Option<PathBuf>,
    pub clock_ns: rune::TextEdit,
    /// Co-simulation stimulus: one vector per line, `a=5 b=[1,2,3]`.
    pub vectors: rune::TextEdit,
    pub synth: Option<Result<Box<helion_hls::HlsResult>, String>>,
    /// Source the last synthesis compiled (stale-result detection).
    pub synth_source: String,
    pub cosim: Option<Result<Box<helion_hls::CosimReport>, String>>,
    pub tab: HlsTab,
    pub added_rtl: Option<PathBuf>,
    /// Add Directive form.
    pub new_kind: usize,
    pub new_options: rune::TextEdit,
    active: Option<Active>,
}

impl Default for HlsState {
    fn default() -> Self {
        Self {
            source: rune::TextEdit::new(""),
            file: None,
            clock_ns: rune::TextEdit::new("10.0"),
            vectors: rune::TextEdit::new(""),
            synth: None,
            synth_source: String::new(),
            cosim: None,
            tab: HlsTab::Report,
            added_rtl: None,
            new_kind: 0,
            new_options: rune::TextEdit::new("II=1"),
            active: None,
        }
    }
}

/// Directive kinds the Add Directive form offers.
pub const DIRECTIVE_KINDS: &[&str] = &[
    "PIPELINE",
    "UNROLL",
    "ARRAY_PARTITION",
    "INLINE",
    "DATAFLOW",
    "INTERFACE",
];

/// Starter kernel for File > New Kernel (written to disk by Save).
pub const NEW_KERNEL: &str = "// HLS kernel: the last function is the top.\nint kernel(int a, int b, int c) {\n  return a + b + c;\n}\n";

impl HlsState {
    pub fn busy(&self) -> Option<HlsJob> {
        self.active.as_ref().map(|a| a.job)
    }

    /// Kernel name: the last function in the source (the HLS top).
    pub fn top(&self) -> Option<String> {
        let src = &self.source.text;
        let mut top = None;
        for line in src.lines() {
            let l = line.trim();
            if l.starts_with('#') || l.starts_with("//") {
                continue;
            }
            if let Some(open) = l.find('(') {
                let head = &l[..open];
                let mut words = head.split_whitespace();
                let first = words.next().unwrap_or("");
                if matches!(first, "if" | "for" | "while" | "switch" | "return") {
                    continue;
                }
                if let Some(name) = head.split_whitespace().last()
                    && head.split_whitespace().count() >= 2
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && (l.ends_with('{') || l.contains(')'))
                    && !l.ends_with(';')
                {
                    top = Some(name.to_string());
                }
            }
        }
        top
    }

    /// Inline `#pragma HLS` directives, each validated by the backend parser.
    pub fn directives(&self) -> Vec<DirectiveRow> {
        let mut out = Vec::new();
        let mut func = String::new();
        let mut loop_line: Option<usize> = None;
        for (i, raw) in self.source.text.lines().enumerate() {
            let l = raw.trim();
            if l.starts_with("for") || l.starts_with("while") {
                loop_line = Some(i + 1);
            }
            if !l.starts_with('#') && l.contains('(') && l.ends_with('{') && !l.starts_with("for") && !l.starts_with("while") && !l.starts_with("if") {
                if let Some(name) = l.split('(').next().and_then(|h| h.split_whitespace().last()) {
                    func = name.to_string();
                    loop_line = None;
                }
            }
            let lower = l.to_ascii_lowercase();
            if !lower.starts_with("#pragma") || !lower.contains("hls") {
                continue;
            }
            let body = l
                .split_whitespace()
                .skip_while(|w| !w.eq_ignore_ascii_case("hls"))
                .skip(1)
                .collect::<Vec<_>>();
            let kind = body.first().map(|k| k.to_ascii_uppercase()).unwrap_or_default();
            let options = body.get(1..).unwrap_or_default().join(" ");
            let error = match helion_hls::parse_pragma_line(l) {
                Ok(Some(_)) => None,
                Ok(None) => Some("not an HLS directive".into()),
                Err(e) => Some(e),
            };
            out.push(DirectiveRow {
                line: i,
                kind,
                options,
                scope: match loop_line {
                    Some(n) => format!("{func}/loop@L{n}"),
                    None => func.clone(),
                },
                error,
            });
        }
        out
    }

    /// The synthesis result is for different source text than the editor's.
    pub fn stale(&self) -> bool {
        matches!(self.synth, Some(Ok(_))) && self.synth_source != self.source.text
    }

    pub fn result(&self) -> Option<&helion_hls::HlsResult> {
        match &self.synth {
            Some(Ok(r)) => Some(r),
            _ => None,
        }
    }
}

/// Parse the stimulus text into co-simulation vectors.
pub fn parse_vectors(text: &str) -> Result<Vec<helion_hls::CosimTestVector>, String> {
    let mut out = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut v = helion_hls::CosimTestVector::new();
        // Split on whitespace outside brackets.
        let mut toks = Vec::new();
        let mut cur = String::new();
        let mut depth = 0;
        for c in line.chars() {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            if c.is_whitespace() && depth == 0 {
                if !cur.is_empty() {
                    toks.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            toks.push(cur);
        }
        for t in toks {
            let (k, val) = t
                .split_once('=')
                .ok_or_else(|| format!("line {}: `{t}` is not name=value", n + 1))?;
            let num = |s: &str| {
                helion_ipxact::parse_param_number(s.trim())
                    .map(|v| v as i64)
                    .ok_or_else(|| format!("line {}: `{s}` is not a number", n + 1))
            };
            if let Some(inner) = val.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                let vals = inner
                    .split(',')
                    .filter(|x| !x.trim().is_empty())
                    .map(num)
                    .collect::<Result<Vec<_>, _>>()?;
                v = v.with_array(k.trim(), vals);
            } else {
                v = v.with_scalar(k.trim(), num(val)?);
            }
        }
        out.push(v);
    }
    Ok(out)
}

fn config(s: &AppState) -> Result<helion_hls::HlsConfig, String> {
    let clock: f64 = s
        .hls
        .clock_ns
        .text
        .trim()
        .parse()
        .ok()
        .filter(|c: &f64| *c > 0.0 && *c <= 1000.0)
        .ok_or("Clock period must be a number of ns in (0, 1000].")?;
    Ok(helion_hls::HlsConfig {
        target_clock_ns: clock,
        part_name: s.project.part.clone(),
        ..Default::default()
    })
}

/// Why Run C Synthesis is unavailable.
pub fn synth_reason(s: &AppState) -> Option<String> {
    if let Some(j) = s.hls.busy() {
        return Some(format!("{}.", j.busy()));
    }
    if s.hls.source.text.trim().is_empty() {
        return Some("Open or create a C/C++ kernel first.".into());
    }
    config(s).err()
}

/// Why Run C/RTL Co-Simulation is unavailable.
pub fn cosim_reason(s: &AppState) -> Option<String> {
    if let Some(r) = synth_reason(s) {
        return Some(r);
    }
    if s.hls.result().is_none() {
        return Some("Run C synthesis first.".into());
    }
    match parse_vectors(&s.hls.vectors.text) {
        Ok(v) if v.is_empty() => Some("Enter at least one test vector (e.g. a=5 b=7).".into()),
        Ok(_) => None,
        Err(e) => Some(e),
    }
}

fn spawn(s: &mut AppState, job: HlsJob, work: impl FnOnce() -> HlsOut + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    s.hls.active = Some(Active {
        job,
        rx: Arc::new(Mutex::new(rx)),
    });
}

/// Run C Synthesis (`csynth_design`) on the worker.
pub fn start_synth(s: &mut AppState) -> Result<String, String> {
    if let Some(r) = synth_reason(s) {
        return Err(format!("csynth_design: {r}"));
    }
    let cfg = config(s)?;
    let src = s.hls.source.text.clone();
    s.hls.synth_source = src.clone();
    s.hls.cosim = None;
    s.navigate(CenterPane::Hls);
    spawn(s, HlsJob::Synth, move || {
        HlsOut::Synth(helion_hls::hls_compile_with_config(&src, &cfg).map(Box::new))
    });
    Ok(format!(
        "csynth_design {} clock={}ns (running)",
        s.hls.top().unwrap_or_default(),
        s.hls.clock_ns.text.trim()
    ))
}

/// Run C/RTL Co-Simulation (`cosim_design`) on the worker.
pub fn start_cosim(s: &mut AppState) -> Result<String, String> {
    if let Some(r) = cosim_reason(s) {
        return Err(format!("cosim_design: {r}"));
    }
    let cfg = config(s)?;
    let vectors = parse_vectors(&s.hls.vectors.text)?;
    let src = s.hls.source.text.clone();
    let n = vectors.len();
    s.navigate(CenterPane::Hls);
    spawn(s, HlsJob::Cosim, move || {
        HlsOut::Cosim(helion_hls::run_rtl_cosim_with(&src, &cfg, &vectors).map(Box::new))
    });
    Ok(format!("cosim_design vectors={n} (running)"))
}

fn line(s: &mut AppState, text: String, sev: MsgSev) {
    s.console.lines.push(ConsoleLine { text, sev });
}

/// Fold a finished worker result into the state.
pub fn poll(s: &mut AppState, tx: &mut rune::Tx) -> bool {
    let Some(active) = s.hls.active.clone() else {
        return false;
    };
    let got = active.rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
    match got {
        Ok(out) => {
            s.hls.active = None;
            deliver(s, out);
            true
        }
        Err(mpsc::TryRecvError::Empty) => {
            tx.wake_after(std::time::Duration::from_millis(60));
            false
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            s.hls.active = None;
            line(s, format!("ERROR: {} stopped unexpectedly", active.job.busy()), MsgSev::Error);
            true
        }
    }
}

fn deliver(s: &mut AppState, out: HlsOut) {
    match out {
        HlsOut::Synth(r) => {
            match &r {
                Ok(h) => {
                    let rep = &h.report;
                    let msg = format!(
                        "csynth_design {} latency={}..{} II={} est_clock={:.2}ns DSP={} BRAM18={} FF={} LUT={}",
                        rep.function_name,
                        rep.min_latency_cycles,
                        rep.max_latency_cycles,
                        rep.ii,
                        rep.estimated_clock_ns,
                        rep.dsp_count,
                        rep.bram18_count,
                        rep.ff_count,
                        rep.lut_count
                    );
                    line(s, msg.clone(), MsgSev::Info);
                    s.toasts.push(ToastKind::Success, "C synthesis complete", &msg);
                }
                Err(e) => {
                    line(s, format!("ERROR: csynth_design: {e}"), MsgSev::Error);
                    s.toasts.push(ToastKind::Error, "C synthesis failed", e);
                }
            }
            s.hls.synth = Some(r);
            s.hls.tab = HlsTab::Report;
        }
        HlsOut::Cosim(r) => {
            match &r {
                Ok(c) => {
                    let pass = c.status == helion_hls::CosimStatus::Pass;
                    let msg = format!(
                        "cosim_design {} {} {}/{} vectors latency={}..{}",
                        c.function_name,
                        if pass { "PASS" } else { "FAIL" },
                        c.vectors_passed,
                        c.vectors_tested,
                        c.min_latency_cycles,
                        c.max_latency_cycles
                    );
                    line(s, msg.clone(), if pass { MsgSev::Info } else { MsgSev::Error });
                    if pass {
                        s.toasts.push(ToastKind::Success, "Co-simulation passed", &msg);
                    } else {
                        s.toasts.push(ToastKind::Error, "Co-simulation failed", &msg);
                    }
                }
                Err(e) => {
                    line(s, format!("ERROR: cosim_design: {e}"), MsgSev::Error);
                    s.toasts.push(ToastKind::Error, "Co-simulation failed", e);
                }
            }
            s.hls.cosim = Some(r);
            s.hls.tab = HlsTab::Cosim;
        }
    }
}

/// Block until the worker finishes (tests, headless scenes).
pub fn wait(s: &mut AppState) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while let Some(active) = s.hls.active.clone() {
        if std::time::Instant::now() > deadline {
            break;
        }
        let got = active
            .rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recv_timeout(std::time::Duration::from_millis(50));
        if let Ok(out) = got {
            s.hls.active = None;
            deliver(s, out);
        }
    }
}

/// File > New Kernel: a fresh starter kernel (results cleared).
pub fn new_kernel(s: &mut AppState) {
    let clock = s.hls.clock_ns.clone();
    s.hls = HlsState {
        source: rune::TextEdit::new(NEW_KERNEL),
        clock_ns: clock,
        ..Default::default()
    };
    s.navigate(CenterPane::Hls);
}

/// Load a kernel from disk into the HLS editor.
pub fn open_kernel(s: &mut AppState, path: &std::path::Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    s.hls.source = rune::TextEdit::new(&text);
    s.hls.file = Some(path.to_path_buf());
    s.hls.synth = None;
    s.hls.cosim = None;
    s.navigate(CenterPane::Hls);
    Ok(format!("open_hls {} ({} lines)", path.display(), text.lines().count()))
}

/// Save the kernel (to its file, or `<gen>/hls/<top>.cpp` when new).
pub fn save_kernel(s: &mut AppState) -> Result<String, String> {
    let path = match &s.hls.file {
        Some(p) => p.clone(),
        None => {
            let top = s.hls.top().unwrap_or_else(|| "kernel".into());
            crate::views::ip_model::gen_dir(s).join("hls").join(format!("{top}.cpp"))
        }
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, &s.hls.source.text).map_err(|e| format!("{}: {e}", path.display()))?;
    s.hls.file = Some(path.clone());
    Ok(format!("save_hls {}", path.display()))
}

/// Why Add RTL to Project is unavailable.
pub fn add_rtl_reason(s: &AppState) -> Option<String> {
    if s.hls.result().is_none() {
        return Some("Run C synthesis first.".into());
    }
    if s.hls.stale() {
        return Some("The kernel changed since synthesis; re-run C synthesis.".into());
    }
    None
}

/// Write the generated SystemVerilog and add it to the project's design sources.
pub fn add_rtl_to_project(s: &mut AppState) -> Result<String, String> {
    if let Some(r) = add_rtl_reason(s) {
        return Err(format!("add_hls_rtl: {r}"));
    }
    let r = s.hls.result().expect("checked");
    let top = r.cdfg.function_name.clone();
    let sv = r.sv.clone();
    let path = crate::views::ip_model::gen_dir(s)
        .join("hls")
        .join(&top)
        .join(format!("{top}.sv"));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, &sv).map_err(|e| format!("{}: {e}", path.display()))?;
    let (n, prj) = crate::project::commit_add_sources(
        s,
        std::slice::from_ref(&path),
        crate::project::FileSet::Design,
    )?;
    s.hls.added_rtl = Some(path.clone());
    Ok(format!(
        "add_hls_rtl {} added={n}{}",
        path.display(),
        prj.map(|p| format!(" prj={}", p.display())).unwrap_or_default()
    ))
}

/// Remove the directive on source line `line`.
pub fn remove_directive(s: &mut AppState, line_no: usize) {
    let lines: Vec<&str> = s.hls.source.text.lines().collect();
    if line_no >= lines.len() {
        return;
    }
    let text = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != line_no)
        .map(|(_, l)| *l)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    s.hls.source = rune::TextEdit::new(&text);
}

/// Insert `#pragma HLS <kind> <options>` after the top function's opening
/// brace (or inside its first loop for PIPELINE/UNROLL).
pub fn add_directive(s: &mut AppState) -> Result<String, String> {
    let kind = DIRECTIVE_KINDS
        .get(s.hls.new_kind)
        .copied()
        .ok_or("pick a directive")?;
    let opts = s.hls.new_options.text.trim().to_string();
    let pragma = format!("#pragma HLS {kind}{}{opts}", if opts.is_empty() { "" } else { " " });
    if let Err(e) = helion_hls::parse_pragma_line(&pragma) {
        return Err(format!("{pragma}: {e}"));
    }
    let top = s.hls.top().ok_or("no top function in the kernel")?;
    let lines: Vec<String> = s.hls.source.text.lines().map(str::to_string).collect();
    let fn_line = lines
        .iter()
        .position(|l| l.contains(&format!("{top}(")) && l.trim_end().ends_with('{'))
        .ok_or_else(|| format!("cannot find the body of {top}"))?;
    let mut at = fn_line + 1;
    if matches!(kind, "PIPELINE" | "UNROLL")
        && let Some(lp) = lines[fn_line + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with("for") && l.trim_end().ends_with('{'))
    {
        at = fn_line + 1 + lp + 1;
    }
    let indent: String = lines
        .get(at)
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_else(|| "  ".into());
    let mut out = lines.clone();
    out.insert(at, format!("{indent}{pragma}"));
    s.hls.source = rune::TextEdit::new(&(out.join("\n") + "\n"));
    Ok(format!("set_directive {pragma} (line {})", at + 1))
}

/// Tcl verbs owned by the HLS surface.
pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "csynth_design" => {
            if let Some(clk) = args.iter().position(|a| a == "-clock").and_then(|i| args.get(i + 1)) {
                s.hls.clock_ns = rune::TextEdit::new(clk);
            }
            start_synth(s)
        }
        "cosim_design" => {
            if let Some(v) = args.iter().position(|a| a == "-vectors").and_then(|i| args.get(i + 1)) {
                s.hls.vectors = rune::TextEdit::new(&v.replace(';', "\n"));
            }
            start_cosim(s)
        }
        "open_hls" => {
            let p = args.first().ok_or("usage: open_hls file.c")?;
            open_kernel(s, std::path::Path::new(p))
        }
        "add_hls_rtl" => add_rtl_to_project(s),
        _ => Err(format!("{name}: not an HLS verb")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_parse_scalars_arrays_and_errors() {
        let v = parse_vectors("a=5 b=0x10\n// c\nx=[1, 2,3] y=-4\n").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].inputs["b"], vec![16]);
        assert_eq!(v[1].inputs["x"], vec![1, 2, 3]);
        assert!(parse_vectors("a5").is_err());
        assert!(parse_vectors("a=zz").is_err());
    }

    #[test]
    fn directives_are_listed_validated_and_editable() {
        let mut s = crate::init();
        s.hls.source = rune::TextEdit::new(
            "int f(int a[4]) {\n  int s = 0;\n  for (int i = 0; i < 4; i++) {\n    s += a[i];\n  }\n  return s;\n}\n",
        );
        assert_eq!(s.hls.top().as_deref(), Some("f"));
        assert!(s.hls.directives().is_empty());
        s.hls.new_kind = 0;
        s.hls.new_options = rune::TextEdit::new("II=1");
        add_directive(&mut s).unwrap();
        let d = s.hls.directives();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, "PIPELINE");
        assert_eq!(d[0].scope, "f/loop@L3", "inserted inside the loop");
        assert!(d[0].error.is_none());
        s.hls.new_kind = 0;
        s.hls.new_options = rune::TextEdit::new("II=banana");
        assert!(add_directive(&mut s).is_err());
        remove_directive(&mut s, d[0].line);
        assert!(s.hls.directives().is_empty());
    }

    #[test]
    fn synth_cosim_and_add_rtl_run_the_real_engines() {
        let mut s = crate::init();
        s.project.name = format!("hls-{}", std::process::id());
        assert!(synth_reason(&s).is_some());
        s.hls.source = rune::TextEdit::new(NEW_KERNEL);
        assert!(cosim_reason(&s).unwrap().contains("synthesis first"));
        start_synth(&mut s).unwrap();
        assert_eq!(s.hls.busy(), Some(HlsJob::Synth));
        assert!(synth_reason(&s).is_some(), "busy disables re-run");
        wait(&mut s);
        let r = s.hls.result().expect("synthesized");
        assert!(r.sv.contains("module kernel"));
        s.hls.vectors = rune::TextEdit::new("a=1 b=2 c=3\na=-5 b=7 c=100\n");
        start_cosim(&mut s).unwrap();
        wait(&mut s);
        let c = s.hls.cosim.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(c.status, helion_hls::CosimStatus::Pass, "{}", c.report_text);
        assert_eq!(c.vector_results[1].rtl_outputs["return"], vec![102]);
        let before = s.project_files.files.len();
        add_rtl_to_project(&mut s).unwrap();
        assert_eq!(s.project_files.files.len(), before + 1);
        s.hls.source = rune::TextEdit::new(&format!("{NEW_KERNEL}\n"));
        assert!(add_rtl_reason(&s).unwrap().contains("changed"));
        let _ = std::fs::remove_dir_all(crate::views::ip_model::gen_dir(&s));
    }
}
