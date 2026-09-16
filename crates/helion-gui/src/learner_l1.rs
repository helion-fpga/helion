//! FM-HEL-L1-GUI — Session stage / constraint provenance mirrors for the status rail.
//!
//! Local mirrors of the Proj L1 contract (`L1-API-FOR-IDE.md`) until `wip/learner-L1`
//! tips `Session::stage()` / `last_timing_provenance()`. Derived from Session Option
//! occupancy + IdeModel timing / user_sdc / honesty. Always compiled (no cfg) so
//! `cargo test -p helion-gui` exercises them by default.

use helion_proj::Session;
use std::fmt;

/// Highest completed Session stage (contract mirror).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionStage {
    Idle,
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
}

impl fmt::Display for SessionStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the closed WNS / period came from (contract mirror).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConstraintProvenance {
    UserXdc,
    DefaultPeriod,
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
}

impl fmt::Display for ConstraintProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageStatus {
    pub stage: SessionStage,
    pub ready: bool,
}

/// Machine-readable illegal transition (contract mirror until Proj tips StageError).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageError {
    pub code: &'static str,
    pub from: SessionStage,
    pub attempted: SessionStage,
    pub missing: SessionStage,
    pub message: String,
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "code={} from={} attempted={} missing={}: {}",
            self.code, self.from, self.attempted, self.missing, self.message
        )
    }
}

impl StageError {
    pub fn prereq(
        from: SessionStage,
        attempted: SessionStage,
        missing: SessionStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: "STAGE_PREREQ",
            from,
            attempted,
            missing,
            message: message.into(),
        }
    }
}

/// Highest completed stage from Session Options (+ optional STA flag from IdeModel).
///
/// Mapping: design→Elaborated, packed→Packed, placed→Placed, routed→Routed,
/// timing_present→Sta, bitstream→Bitgen. Bitgen wins over Sta when both set.
pub fn session_stage_of(session: &Session, timing_present: bool) -> SessionStage {
    if session.bitstream.is_some() {
        return SessionStage::Bitgen;
    }
    if timing_present && session.routed.is_some() {
        return SessionStage::Sta;
    }
    if session.routed.is_some() {
        return SessionStage::Routed;
    }
    if session.placed.is_some() {
        return SessionStage::Placed;
    }
    if session.packed.is_some() {
        return SessionStage::Packed;
    }
    if session.design.is_some() {
        return SessionStage::Elaborated;
    }
    SessionStage::Idle
}

/// Ready/prereq strip for every contract stage (Sim/Lab optional, never ready here).
pub fn stage_status_of(session: &Session, timing_present: bool) -> Vec<StageStatus> {
    let current = session_stage_of(session, timing_present);
    let stages = [
        SessionStage::Idle,
        SessionStage::Elaborated,
        SessionStage::Packed,
        SessionStage::Placed,
        SessionStage::Routed,
        SessionStage::Sta,
        SessionStage::Bitgen,
    ];
    stages
        .into_iter()
        .map(|stage| StageStatus {
            stage,
            // Idle is always "ready" as the baseline; others ready when current >= stage.
            ready: stage == SessionStage::Idle || current >= stage,
        })
        .collect()
}

/// Map user_sdc + honesty label → provenance until `Session::last_timing_provenance`.
pub fn constraint_provenance_of(user_sdc: bool, honesty: &str) -> ConstraintProvenance {
    if honesty.starts_with("no_clock_path") || honesty.contains("no_clock_path") {
        return ConstraintProvenance::NoClockPath;
    }
    if user_sdc {
        ConstraintProvenance::UserXdc
    } else {
        ConstraintProvenance::DefaultPeriod
    }
}

/// Refuse illegal advance with a StageError (GUI-local until Proj can_advance tips).
pub fn can_advance(
    session: &Session,
    timing_present: bool,
    to: SessionStage,
) -> Result<(), StageError> {
    let from = session_stage_of(session, timing_present);
    let missing = match to {
        SessionStage::Idle | SessionStage::Elaborated => return Ok(()),
        SessionStage::Packed | SessionStage::Placed => {
            if session.design.is_some() {
                return Ok(());
            }
            SessionStage::Elaborated
        }
        SessionStage::Routed => {
            if session.placed.is_some() {
                return Ok(());
            }
            SessionStage::Placed
        }
        SessionStage::Sta => {
            if session.routed.is_some() {
                return Ok(());
            }
            SessionStage::Routed
        }
        SessionStage::Bitgen => {
            if session.routed.is_some() {
                return Ok(());
            }
            SessionStage::Routed
        }
        SessionStage::Sim | SessionStage::Lab => return Ok(()),
    };
    let tip = match missing {
        SessionStage::Elaborated => "Synthesize first",
        SessionStage::Placed => "Place first",
        SessionStage::Routed => "Route first",
        other => return Err(StageError::prereq(from, to, other, format!("{other} first"))),
    };
    Err(StageError::prereq(from, to, missing, tip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_proj::Mode;

    #[test]
    fn idle_session_is_idle_default_period() {
        let s = Session::new(Mode::NonProject);
        assert_eq!(session_stage_of(&s, false), SessionStage::Idle);
        assert_eq!(
            constraint_provenance_of(false, "no_body cells=0"),
            ConstraintProvenance::DefaultPeriod
        );
        assert_eq!(
            constraint_provenance_of(false, "no_clock_path cells=1"),
            ConstraintProvenance::NoClockPath
        );
        assert_eq!(
            constraint_provenance_of(true, "WNS_PS=9640"),
            ConstraintProvenance::UserXdc
        );
    }

    #[test]
    fn route_before_place_is_stage_prereq() {
        let s = Session::new(Mode::NonProject);
        let e = can_advance(&s, false, SessionStage::Routed).unwrap_err();
        assert_eq!(e.code, "STAGE_PREREQ");
        assert_eq!(e.missing, SessionStage::Placed);
        assert!(e.message.contains("Place first"), "{}", e.message);
        assert!(e.to_string().contains("code=STAGE_PREREQ"));
    }
}
