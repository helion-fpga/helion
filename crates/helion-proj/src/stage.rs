//! Session stage machine + constraint provenance (FM-HEL-L1).
//!
//! One Session is the source of truth for CLI and IDE. Illegal transitions
//! refuse with [`StageError`] (`code=STAGE_PREREQ`). STA results carry
//! [`ConstraintProvenance`] so a default 10 ns period is never mistaken for
//! a user `create_clock`.

use crate::Session;
use helion_device::Device;
use helion_ir::{CellKind, Design};
use helion_sta::{create_clock, report_timing_routed_xdc, Clock, Constraints, TimingResult};
use std::fmt;

/// Highest completed / attempted CAD stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SessionStage {
    Idle,
    /// Elaborate/map done (design present).
    Elaborated,
    Packed,
    Placed,
    Routed,
    Sta,
    Bitgen,
    Sim,
    Lab,
}

impl SessionStage {
    pub const ALL: [SessionStage; 9] = [
        SessionStage::Idle,
        SessionStage::Elaborated,
        SessionStage::Packed,
        SessionStage::Placed,
        SessionStage::Routed,
        SessionStage::Sta,
        SessionStage::Bitgen,
        SessionStage::Sim,
        SessionStage::Lab,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Elaborated => "Elaborated",
            Self::Packed => "Packed",
            Self::Placed => "Placed",
            Self::Routed => "Routed",
            Self::Sta => "Sta",
            Self::Bitgen => "Bitgen",
            Self::Sim => "Sim",
            Self::Lab => "Lab",
        }
    }

    /// Immediate prerequisite. `Idle` / `Elaborated` have none.
    pub fn prereq(self) -> Option<SessionStage> {
        match self {
            Self::Idle | Self::Elaborated => None,
            Self::Packed => Some(Self::Elaborated),
            Self::Placed => Some(Self::Packed),
            Self::Routed => Some(Self::Placed),
            // STA and bitgen both consume a routed netlist. Bitgen does not
            // require a prior STA report (existing Tcl `write_bitstream` after
            // `route_design` stays legal).
            Self::Sta | Self::Bitgen => Some(Self::Routed),
            Self::Sim | Self::Lab => Some(Self::Bitgen),
        }
    }
}

impl fmt::Display for SessionStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the STA clock period came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ConstraintProvenance {
    /// At least one user `create_clock` / loaded XDC clock.
    UserXdc,
    /// Session injected the default 10 ns period.
    DefaultPeriod,
    /// No usable clock for STA (no user clock and no clocked path).
    NoClockPath,
}

impl ConstraintProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserXdc => "UserXdc",
            Self::DefaultPeriod => "DefaultPeriod",
            Self::NoClockPath => "NoClockPath",
        }
    }

    /// Classify clocks used for STA against the mapped netlist.
    pub fn from_constraints(xdc: &Constraints, design: &Design) -> Self {
        if !xdc.clocks.is_empty() {
            Self::UserXdc
        } else if design_has_clock_path(design) {
            Self::DefaultPeriod
        } else {
            Self::NoClockPath
        }
    }
}

impl fmt::Display for ConstraintProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// FF clock pin (or an HFF cell) is the usable STA clock path.
pub fn design_has_clock_path(design: &Design) -> bool {
    design
        .cells
        .iter()
        .any(|c| matches!(c.kind, CellKind::Hff))
}

/// Shared CLI/Session classifier.
pub fn constraint_provenance(xdc: &Constraints, design: &Design) -> ConstraintProvenance {
    ConstraintProvenance::from_constraints(xdc, design)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageStatus {
    pub stage: SessionStage,
    pub ready: bool,
}

/// STA result plus constraint provenance. Same numeric fields as `TimingResult`.
#[derive(Clone, Debug)]
pub struct TimingReport {
    pub design: String,
    pub clocks: Vec<Clock>,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub endpoints: usize,
    pub r2r_ps: i64,
    pub iob_ps: i64,
    pub setup_ps: i64,
    pub hold_ps: i64,
    pub hold_slack_ps: i64,
    pub route_ps: i64,
    pub clk_net_ps: i64,
    pub provenance: ConstraintProvenance,
    /// Typically [`SessionStage::Sta`] when produced.
    pub stage: SessionStage,
}

impl TimingReport {
    pub fn from_timing(
        design: &str,
        t: TimingResult,
        provenance: ConstraintProvenance,
    ) -> Self {
        Self {
            design: design.to_string(),
            clocks: t.clocks,
            wns_ps: t.wns_ps,
            tns_ps: t.tns_ps,
            endpoints: t.endpoints,
            r2r_ps: t.r2r_ps,
            iob_ps: t.iob_ps,
            setup_ps: t.setup_ps,
            hold_ps: t.hold_ps,
            hold_slack_ps: t.hold_slack_ps,
            route_ps: t.route_ps,
            clk_net_ps: t.clk_net_ps,
            provenance,
            stage: SessionStage::Sta,
        }
    }

    pub fn empty(design: &str, provenance: ConstraintProvenance) -> Self {
        Self {
            design: design.to_string(),
            clocks: Vec::new(),
            wns_ps: 0,
            tns_ps: 0,
            endpoints: 0,
            r2r_ps: 0,
            iob_ps: 0,
            setup_ps: 0,
            hold_ps: 0,
            hold_slack_ps: 0,
            route_ps: 0,
            clk_net_ps: 0,
            provenance,
            stage: SessionStage::Sta,
        }
    }

    /// Headless line: existing WNS tokens plus `provenance=` / `stage=`.
    pub fn text(&self) -> String {
        format!(
            "report_timing {} WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={} provenance={} stage={}",
            self.design,
            self.wns_ps,
            self.tns_ps,
            self.setup_ps,
            self.hold_ps,
            self.hold_slack_ps,
            self.endpoints,
            self.r2r_ps,
            self.iob_ps,
            self.route_ps,
            self.provenance.as_str(),
            self.stage.as_str()
        )
    }
}

/// Machine-readable illegal transition refuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageError {
    pub code: &'static str,
    pub from: SessionStage,
    pub attempted: SessionStage,
    pub missing: SessionStage,
    pub message: String,
}

pub const STAGE_PREREQ: &str = "STAGE_PREREQ";
pub const STA_FAIL: &str = "STA_FAIL";

impl StageError {
    pub fn prereq(
        from: SessionStage,
        attempted: SessionStage,
        missing: SessionStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: STAGE_PREREQ,
            from,
            attempted,
            missing,
            message: message.into(),
        }
    }
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "code={} from={} attempted={} missing={} {}",
            self.code,
            self.from.as_str(),
            self.attempted.as_str(),
            self.missing.as_str(),
            self.message
        )
    }
}

impl std::error::Error for StageError {}

impl From<StageError> for String {
    fn from(e: StageError) -> Self {
        e.to_string()
    }
}

impl Session {
    /// Highest completed stage (Lab > Sim > Bitgen > Sta > Routed > … > Idle).
    pub fn stage(&self) -> SessionStage {
        SessionStage::ALL
            .into_iter()
            .rev()
            .find(|s| self.stage_completed(*s))
            .unwrap_or(SessionStage::Idle)
    }

    pub fn stage_completed(&self, stage: SessionStage) -> bool {
        match stage {
            SessionStage::Idle => true,
            SessionStage::Elaborated => self.design.is_some(),
            SessionStage::Packed => self.packed.is_some(),
            SessionStage::Placed => self.placed.is_some(),
            SessionStage::Routed => self.routed.is_some(),
            SessionStage::Sta => self.timing_provenance.get().is_some(),
            SessionStage::Bitgen => self
                .bitstream
                .as_ref()
                .map(|b| !b.frames.is_empty())
                .unwrap_or(false),
            SessionStage::Sim => self.sim_done,
            SessionStage::Lab => self.lab_done,
        }
    }

    /// All stages with `ready` = already complete or prerequisite satisfied.
    pub fn stage_status(&self) -> Vec<StageStatus> {
        SessionStage::ALL
            .into_iter()
            .map(|stage| StageStatus {
                stage,
                ready: self.stage_completed(stage) || self.can_advance(stage).is_ok(),
            })
            .collect()
    }

    pub fn print_stages_text(&self) -> String {
        self.stage_status()
            .into_iter()
            .map(|s| {
                format!(
                    "stage={} ready={}",
                    s.stage.as_str(),
                    u8::from(s.ready)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn last_timing_provenance(&self) -> Option<ConstraintProvenance> {
        self.timing_provenance.get()
    }

    /// Record STA provenance without re-running the engine (CLI compile ingest).
    pub fn record_timing_provenance(&self, p: ConstraintProvenance) {
        self.timing_provenance.set(Some(p));
    }

    /// Refuse an illegal transition with a stable `code=STAGE_PREREQ`.
    pub fn can_advance(&self, to: SessionStage) -> Result<(), StageError> {
        if self.stage_completed(to) {
            return Ok(());
        }
        match to.prereq() {
            None => Ok(()),
            Some(missing) if self.stage_completed(missing) => Ok(()),
            Some(missing) => Err(StageError::prereq(
                self.stage(),
                to,
                missing,
                self.prereq_message(to),
            )),
        }
    }

    fn prereq_message(&self, to: SessionStage) -> String {
        match to {
            SessionStage::Packed => "pack_design: no design".into(),
            SessionStage::Placed => {
                if self.design.is_none() {
                    "place_design: no design".into()
                } else {
                    "place_design: not packed".into()
                }
            }
            SessionStage::Routed => "route_design: not placed".into(),
            SessionStage::Sta => "report_timing: not routed".into(),
            SessionStage::Bitgen => {
                if self.design.is_none() {
                    "write_bitstream: no design".into()
                } else {
                    "write_bitstream: not routed".into()
                }
            }
            SessionStage::Sim | SessionStage::Lab => {
                "program_hw: no bitstream — run write_bitstream / Implement".into()
            }
            SessionStage::Idle | SessionStage::Elaborated => String::new(),
        }
    }

    fn refuse(&self, to: SessionStage) -> Result<(), String> {
        self.can_advance(to).map_err(|e| e.to_string())
    }

    pub fn clear_timing(&self) {
        self.timing_provenance.set(None);
    }

    pub(crate) fn clear_post_place(&mut self) {
        self.routed = None;
        self.bitstream = None;
        self.sim_done = false;
        self.lab_done = false;
        self.programmed = false;
        self.clear_timing();
    }

    pub(crate) fn clear_post_route(&mut self) {
        self.bitstream = None;
        self.sim_done = false;
        self.lab_done = false;
        self.programmed = false;
        self.clear_timing();
    }

    /// Map to packed BELs. Requires [`SessionStage::Elaborated`].
    pub fn pack_design(&mut self, dev: &Device) -> Result<(), String> {
        self.refuse(SessionStage::Packed)?;
        let d = self.design.as_ref().ok_or("pack_design: no design")?;
        let packed = helion_pack::pack(d, dev)?;
        self.packed = Some(packed);
        self.placed = None;
        self.clear_post_place();
        Ok(())
    }

    /// Run one legal step (`pack` / `place` / `route` / `sta` / `bitgen`).
    pub fn advance(&mut self, to: SessionStage, dev: &Device) -> Result<(), StageError> {
        self.can_advance(to)?;
        match to {
            SessionStage::Idle | SessionStage::Elaborated => Ok(()),
            SessionStage::Packed => self
                .pack_design(dev)
                .map_err(|message| self.advance_wrap(to, message)),
            SessionStage::Placed => self
                .place_design(dev)
                .map_err(|message| self.advance_wrap(to, message)),
            SessionStage::Routed => self
                .route_design(dev)
                .map_err(|message| self.advance_wrap(to, message)),
            SessionStage::Sta => self
                .report_timing_report(dev, &Constraints::default())
                .map(|_| ())
                .map_err(|e| e),
            SessionStage::Bitgen => self
                .write_bitstream(dev)
                .map(|_| ())
                .map_err(|message| self.advance_wrap(to, message)),
            SessionStage::Sim | SessionStage::Lab => Ok(()),
        }
    }

    fn advance_wrap(&self, attempted: SessionStage, message: String) -> StageError {
        if let Some(err) = parse_stage_error_line(&message) {
            return err;
        }
        StageError {
            code: STAGE_PREREQ,
            from: self.stage(),
            attempted,
            missing: attempted.prereq().unwrap_or(SessionStage::Idle),
            message,
        }
    }

    /// Structured STA for GUI. Records [`last_timing_provenance`].
    pub fn report_timing_report(
        &self,
        dev: &Device,
        xdc: &Constraints,
    ) -> Result<TimingReport, StageError> {
        let _ = dev;
        self.can_advance(SessionStage::Sta)?;
        let d = self.design.as_ref().ok_or_else(|| {
            StageError::prereq(
                self.stage(),
                SessionStage::Sta,
                SessionStage::Elaborated,
                "report_timing: no design",
            )
        })?;
        let r = self.routed.as_ref().ok_or_else(|| {
            StageError::prereq(
                self.stage(),
                SessionStage::Sta,
                SessionStage::Routed,
                "report_timing: not routed",
            )
        })?;
        let provenance = ConstraintProvenance::from_constraints(xdc, d);
        self.timing_provenance.set(Some(provenance));
        if provenance == ConstraintProvenance::NoClockPath {
            return Ok(TimingReport::empty(&d.name, provenance));
        }
        let mut clks = xdc.clocks.clone();
        if clks.is_empty() {
            create_clock(&mut clks, "clk", 10_000, "clk");
        }
        let t = report_timing_routed_xdc(d, r, &clks, xdc).map_err(|message| StageError {
            code: STA_FAIL,
            from: self.stage(),
            attempted: SessionStage::Sta,
            missing: SessionStage::Routed,
            message,
        })?;
        Ok(TimingReport::from_timing(&d.name, t, provenance))
    }
}

fn parse_stage_error_line(s: &str) -> Option<StageError> {
    if !s.starts_with("code=") {
        return None;
    }
    let mut code = STAGE_PREREQ;
    let mut from = SessionStage::Idle;
    let mut attempted = SessionStage::Idle;
    let mut missing = SessionStage::Idle;
    let mut rest = String::new();
    for (i, tok) in s.split_whitespace().enumerate() {
        if let Some(v) = tok.strip_prefix("code=") {
            code = match v {
                "STA_FAIL" => STA_FAIL,
                _ => STAGE_PREREQ,
            };
        } else if let Some(v) = tok.strip_prefix("from=") {
            from = parse_stage_name(v).unwrap_or(SessionStage::Idle);
        } else if let Some(v) = tok.strip_prefix("attempted=") {
            attempted = parse_stage_name(v).unwrap_or(SessionStage::Idle);
        } else if let Some(v) = tok.strip_prefix("missing=") {
            missing = parse_stage_name(v).unwrap_or(SessionStage::Idle);
        } else if i >= 4 {
            if !rest.is_empty() {
                rest.push(' ');
            }
            rest.push_str(tok);
        }
    }
    Some(StageError {
        code,
        from,
        attempted,
        missing,
        message: rest,
    })
}

fn parse_stage_name(s: &str) -> Option<SessionStage> {
    SessionStage::ALL.into_iter().find(|st| st.as_str() == s)
}
