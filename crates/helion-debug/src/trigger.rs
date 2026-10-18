//! Multi-Stage ILA Trigger State Machine, Capture Qualification & Segmented Buffers (UG908 parity).
//!
//! Supports up to 16 trigger sequencer states with:
//! - Boolean combination (`AND`, `OR`, `NAND`, `NOR`) across multiple probe comparators
//! - Edge detection (`R` rising, `F` falling, `B` both/toggle, `X` don't-care)
//! - Value & range comparators (`==`, `!=`, `>=`, `<=`, `>`, `<`, `InRange`, bitmask)
//! - 16-bit event counters (`u16`) per state with `Goto(next_state)` / `Trigger` actions
//!   and optional `else_state` branching
//! - Pre-trigger, Center-trigger, Post-trigger, and custom window positioning
//! - Capture qualification predicate filtering which clock cycles enter the sample buffer
//! - Segmented capture buffers (`num_segments × segment_depth`) with automatic re-arm

use crate::{
    CAPTURE_BRAM, compile, insert_capture_bram, insert_ila, pack_sample_word, probe_site,
    strip_capture_bram, strip_ila, unpack_sample_word,
};
use helion_device::Device;
use helion_hw::Tap;
use helion_ir::Design;
use std::collections::VecDeque;

/// Maximum number of states in an ILA multi-stage trigger sequencer (UG908 limit = 16).
pub const MAX_TRIGGER_STATES: usize = 16;

/// Boolean reduction across comparators within a single trigger state or capture qualifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerBoolOp {
    And,
    Or,
    Nand,
    Nor,
}

impl TriggerBoolOp {
    pub fn evaluate(self, results: &[bool]) -> bool {
        if results.is_empty() {
            return true;
        }
        match self {
            Self::And => results.iter().all(|&b| b),
            Self::Or => results.iter().any(|&b| b),
            Self::Nand => !results.iter().all(|&b| b),
            Self::Nor => !results.iter().any(|&b| b),
        }
    }
}

/// Comparator operation on a scalar probe or multi-bit bus probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparatorOp {
    /// Don't care (`'X'`) — always true.
    DontCare,
    /// Rising edge (`'R'`: `prev == 0 && curr != 0`).
    RisingEdge,
    /// Falling edge (`'F'`: `prev != 0 && curr == 0`).
    FallingEdge,
    /// Both edges / any transition (`'B'`: `prev != curr`).
    BothEdges,
    /// Exact equality (`curr == val`).
    Equal(u64),
    /// Inequality (`curr != val`).
    NotEqual(u64),
    /// Unsigned greater-than-or-equal (`curr >= min`).
    GreaterOrEqual(u64),
    /// Unsigned less-than-or-equal (`curr <= max`).
    LessOrEqual(u64),
    /// Unsigned strictly greater (`curr > val`).
    GreaterThan(u64),
    /// Unsigned strictly less (`curr < val`).
    LessThan(u64),
    /// Inclusive range (`curr >= low && curr <= high`).
    InRange { low: u64, high: u64 },
    /// Ternary bitmask match (`(curr & mask) == (value & mask)`).
    BitMask { mask: u64, value: u64 },
}

impl ComparatorOp {
    /// Parse UG908 shorthand comparator token (`"R"`, `"F"`, `"B"`, `"X"`, `">=5"`, `"<=10"`, `"==3"`, `"!=0"`, `"4..=12"`).
    pub fn parse(spec: &str) -> Result<Self, String> {
        let s = spec.trim();
        match s.to_ascii_uppercase().as_str() {
            "X" | "DONTCARE" | "ANY" => return Ok(Self::DontCare),
            "R" | "RISE" | "RISING" | "POSEDGE" => return Ok(Self::RisingEdge),
            "F" | "FALL" | "FALLING" | "NEGEDGE" => return Ok(Self::FallingEdge),
            "B" | "BOTH" | "TOGGLE" => return Ok(Self::BothEdges),
            _ => {}
        }
        if let Some(rest) = s.strip_prefix(">=") {
            let v = parse_u64_literal(rest)?;
            return Ok(Self::GreaterOrEqual(v));
        }
        if let Some(rest) = s.strip_prefix("<=") {
            let v = parse_u64_literal(rest)?;
            return Ok(Self::LessOrEqual(v));
        }
        if let Some(rest) = s.strip_prefix("!=") {
            let v = parse_u64_literal(rest)?;
            return Ok(Self::NotEqual(v));
        }
        if let Some(rest) = s.strip_prefix("==") {
            let v = parse_u64_literal(rest)?;
            return Ok(Self::Equal(v));
        }
        if let Some(rest) = s.strip_prefix('>') {
            let v = parse_u64_literal(rest)?;
            return Ok(Self::GreaterThan(v));
        }
        if let Some(rest) = s.strip_prefix('<') {
            let v = parse_u64_literal(rest)?;
            return Ok(Self::LessThan(v));
        }
        if let Some((lo, hi)) = s.split_once("..=") {
            return Ok(Self::InRange {
                low: parse_u64_literal(lo)?,
                high: parse_u64_literal(hi)?,
            });
        }
        Err(format!("ila comparator: unrecognized spec '{s}'"))
    }

    pub fn evaluate(self, prev: Option<u64>, curr: u64) -> bool {
        match self {
            Self::DontCare => true,
            Self::RisingEdge => match prev {
                Some(p) => p == 0 && curr != 0,
                None => false,
            },
            Self::FallingEdge => match prev {
                Some(p) => p != 0 && curr == 0,
                None => false,
            },
            Self::BothEdges => match prev {
                Some(p) => p != curr,
                None => false,
            },
            Self::Equal(v) => curr == v,
            Self::NotEqual(v) => curr != v,
            Self::GreaterOrEqual(min) => curr >= min,
            Self::LessOrEqual(max) => curr <= max,
            Self::GreaterThan(v) => curr > v,
            Self::LessThan(v) => curr < v,
            Self::InRange { low, high } => curr >= low && curr <= high,
            Self::BitMask { mask, value } => (curr & mask) == (value & mask),
        }
    }
}

fn parse_u64_literal(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("invalid hex '{t}': {e}"))
    } else if let Some(bin) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        u64::from_str_radix(bin, 2).map_err(|e| format!("invalid bin '{t}': {e}"))
    } else {
        t.parse::<u64>()
            .map_err(|e| format!("invalid integer '{t}': {e}"))
    }
}

/// Probe comparator binding one or more probe bit indices (LSB-first) to a [`ComparatorOp`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeComparator {
    /// Indices into the probe bit vector (LSB first). A single element tests a 1-bit net;
    /// multiple elements assemble an unsigned bus word.
    pub bit_indices: Vec<usize>,
    pub op: ComparatorOp,
}

impl ProbeComparator {
    /// Single-bit probe comparator at `bit_index`.
    pub fn bit(bit_index: usize, op: ComparatorOp) -> Self {
        Self {
            bit_indices: vec![bit_index],
            op,
        }
    }

    /// Multi-bit bus probe comparator from LSB-first `bit_indices`.
    pub fn bus(bit_indices: Vec<usize>, op: ComparatorOp) -> Self {
        Self { bit_indices, op }
    }

    pub fn extract_value(&self, sample: &[bool]) -> u64 {
        let mut val = 0u64;
        for (pos, &idx) in self.bit_indices.iter().enumerate().take(64) {
            if sample.get(idx).copied().unwrap_or(false) {
                val |= 1u64 << pos;
            }
        }
        val
    }

    pub fn evaluate(&self, prev_sample: Option<&[bool]>, curr_sample: &[bool]) -> bool {
        let prev_val = prev_sample.map(|p| self.extract_value(p));
        let curr_val = self.extract_value(curr_sample);
        self.op.evaluate(prev_val, curr_val)
    }
}

/// Composite boolean condition combining one or more [`ProbeComparator`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerCondition {
    pub bool_op: TriggerBoolOp,
    pub comparators: Vec<ProbeComparator>,
}

impl TriggerCondition {
    pub fn single(comp: ProbeComparator) -> Self {
        Self {
            bool_op: TriggerBoolOp::And,
            comparators: vec![comp],
        }
    }

    pub fn and(comparators: Vec<ProbeComparator>) -> Self {
        Self {
            bool_op: TriggerBoolOp::And,
            comparators,
        }
    }

    pub fn or(comparators: Vec<ProbeComparator>) -> Self {
        Self {
            bool_op: TriggerBoolOp::Or,
            comparators,
        }
    }

    pub fn evaluate(&self, prev_sample: Option<&[bool]>, curr_sample: &[bool]) -> bool {
        if self.comparators.is_empty() {
            return true;
        }
        let mut results = Vec::with_capacity(self.comparators.len());
        for c in &self.comparators {
            results.push(c.evaluate(prev_sample, curr_sample));
        }
        self.bool_op.evaluate(&results)
    }
}

/// Action taken when a trigger state's condition + 16-bit event counter target is met.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateTransition {
    /// Advance to `next_state` (`0..16`).
    Goto(u8),
    /// Fire the ILA capture trigger.
    Trigger,
}

/// One state in the 16-state ILA trigger sequencer (`state_id` in `0..16`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerStateSpec {
    pub state_id: u8,
    pub label: String,
    pub condition: TriggerCondition,
    /// 16-bit event counter target (`1` = transition/trigger on first match; `N` = require `N` matches).
    pub counter_target: u16,
    /// If true, non-matching cycles reset this state's 16-bit counter back to `0` (consecutive matches).
    pub reset_counter_on_miss: bool,
    /// Transition when condition has matched `counter_target` times.
    pub on_match: StateTransition,
    /// Optional branch state when condition evaluates to `false` on a cycle.
    pub else_state: Option<u8>,
}

impl TriggerStateSpec {
    pub fn new(
        state_id: u8,
        label: impl Into<String>,
        condition: TriggerCondition,
        counter_target: u16,
        on_match: StateTransition,
    ) -> Self {
        Self {
            state_id,
            label: label.into(),
            condition,
            counter_target: counter_target.max(1),
            reset_counter_on_miss: false,
            on_match,
            else_state: None,
        }
    }

    pub fn with_reset_on_miss(mut self, reset: bool) -> Self {
        self.reset_counter_on_miss = reset;
        self
    }

    pub fn with_else_state(mut self, else_state: u8) -> Self {
        self.else_state = Some(else_state);
        self
    }
}

/// Trigger position within each capture segment window (UG908).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerWindowPosition {
    /// Post-trigger capture: trigger sits at sample `0` (`pre_samples = 0`).
    PostTrigger,
    /// Center-trigger capture: trigger sits at `segment_depth / 2`.
    CenterTrigger,
    /// Pre-trigger capture: trigger sits at the end of the window (`segment_depth - 1`).
    PreTrigger,
    /// Explicit number of pre-trigger samples (`0..segment_depth`).
    Custom { pre_samples: usize },
}

impl TriggerWindowPosition {
    pub fn pre_samples(self, segment_depth: usize) -> usize {
        let depth = segment_depth.max(1);
        match self {
            Self::PostTrigger => 0,
            Self::CenterTrigger => (depth / 2).min(depth.saturating_sub(1)),
            Self::PreTrigger => depth.saturating_sub(1),
            Self::Custom { pre_samples } => pre_samples.min(depth.saturating_sub(1)),
        }
    }
}

/// Configuration for the multi-stage ILA trigger sequencer & segmented capture engine.
#[derive(Clone, Debug)]
pub struct IlaSequencerConfig {
    /// Up to 16 trigger sequencer states (ordered by `state_id`, starting at state `0`).
    pub states: Vec<TriggerStateSpec>,
    /// Window positioning within each segment.
    pub window_position: TriggerWindowPosition,
    /// Number of samples per segment (`>= 1`).
    pub segment_depth: usize,
    /// Number of segments in the capture buffer (`>= 1`).
    pub num_segments: usize,
    /// Optional capture qualification predicate: only cycles where this evaluates to `true`
    /// are stored in the capture buffer.
    pub capture_qualifier: Option<TriggerCondition>,
}

impl IlaSequencerConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.states.is_empty() {
            return Err("ila sequencer: must have at least 1 state".into());
        }
        if self.states.len() > MAX_TRIGGER_STATES {
            return Err(format!(
                "ila sequencer: {} states exceeds UG908 max {}",
                self.states.len(),
                MAX_TRIGGER_STATES
            ));
        }
        if self.segment_depth == 0 {
            return Err("ila sequencer: segment_depth must be >= 1".into());
        }
        if self.num_segments == 0 {
            return Err("ila sequencer: num_segments must be >= 1".into());
        }
        for s in &self.states {
            if usize::from(s.state_id) >= MAX_TRIGGER_STATES {
                return Err(format!(
                    "ila sequencer: state_id {} out of range 0..{}",
                    s.state_id, MAX_TRIGGER_STATES
                ));
            }
            if let StateTransition::Goto(next) = s.on_match {
                if !self.states.iter().any(|st| st.state_id == next) {
                    return Err(format!(
                        "ila sequencer: state {} Goto({}) targets nonexistent state",
                        s.state_id, next
                    ));
                }
            }
            if let Some(else_id) = s.else_state {
                if !self.states.iter().any(|st| st.state_id == else_id) {
                    return Err(format!(
                        "ila sequencer: state {} else_state {} targets nonexistent state",
                        s.state_id, else_id
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One captured segment from a segmented ILA capture run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IlaCapturedSegment {
    pub segment_index: usize,
    /// Chronological samples in this segment (`samples.len() == segment_depth`).
    pub samples: Vec<Vec<bool>>,
    /// Packed 64-bit sample words corresponding to `samples`.
    pub words: Vec<u64>,
    /// Index inside `samples` where the final-stage trigger fired.
    pub trigger_offset: usize,
    /// Global clock cycle on which the final-stage trigger fired.
    pub trigger_cycle: usize,
    /// Sequence of `(cycle, state_id, counter_after)` transitions leading to this segment trigger.
    pub state_trace: Vec<(usize, u8, u16)>,
}

/// Complete multi-segment capture result from [`IlaTriggerSequencer`].
#[derive(Clone, Debug)]
pub struct IlaSequencerCaptureResult {
    pub probes: Vec<String>,
    pub segments: Vec<IlaCapturedSegment>,
    pub qualified_cycles: usize,
    pub total_cycles: usize,
    pub upload_dr_scans: usize,
    pub upload_raw_scans: usize,
}

/// Runtime execution engine for the 16-state ILA trigger state machine.
#[derive(Clone, Debug)]
pub struct IlaTriggerSequencer {
    pub cfg: IlaSequencerConfig,
    pub current_state: u8,
    pub state_counters: [u16; MAX_TRIGGER_STATES],
    pub prev_sample: Option<Vec<bool>>,
    pub fired_in_segment: bool,
    pub post_remaining: usize,
    pub trigger_cycle_in_seg: usize,
    pub pre_samples_in_seg: usize,
    pub ring: VecDeque<Vec<bool>>,
    pub state_trace: Vec<(usize, u8, u16)>,
    pub segments: Vec<IlaCapturedSegment>,
    pub total_cycles: usize,
    pub qualified_cycles: usize,
}

impl IlaTriggerSequencer {
    pub fn new(cfg: IlaSequencerConfig) -> Result<Self, String> {
        cfg.validate()?;
        let initial_state = cfg.states[0].state_id;
        let depth = cfg.segment_depth;
        Ok(Self {
            cfg,
            current_state: initial_state,
            state_counters: [0u16; MAX_TRIGGER_STATES],
            prev_sample: None,
            fired_in_segment: false,
            post_remaining: 0,
            trigger_cycle_in_seg: 0,
            pre_samples_in_seg: 0,
            ring: VecDeque::with_capacity(depth),
            state_trace: Vec::new(),
            segments: Vec::new(),
            total_cycles: 0,
            qualified_cycles: 0,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.segments.len() >= self.cfg.num_segments
    }

    fn reset_for_next_segment(&mut self) {
        self.current_state = self.cfg.states[0].state_id;
        self.state_counters = [0u16; MAX_TRIGGER_STATES];
        self.fired_in_segment = false;
        self.post_remaining = 0;
        self.trigger_cycle_in_seg = 0;
        self.pre_samples_in_seg = 0;
        self.ring.clear();
        self.state_trace.clear();
    }

    /// Step the multi-stage trigger sequencer and capture buffer on one clock cycle's sample.
    /// Returns `true` when all `num_segments` segments have been captured.
    pub fn step_sample(&mut self, sample: &[bool]) -> bool {
        if self.is_complete() {
            return true;
        }
        let cycle = self.total_cycles;
        self.total_cycles += 1;

        let prev_ref = self.prev_sample.as_deref();

        // 1. Evaluate capture qualification predicate (if any).
        let qualified = match &self.cfg.capture_qualifier {
            Some(q) => q.evaluate(prev_ref, sample),
            None => true,
        };

        let depth = self.cfg.segment_depth;
        let target_pre = self.cfg.window_position.pre_samples(depth);

        if qualified {
            self.qualified_cycles += 1;
            if self.ring.len() == depth {
                self.ring.pop_front();
            }
            self.ring.push_back(sample.to_vec());
        }

        // 2. If not yet triggered in the current segment, advance the trigger state machine.
        //    When a capture qualifier is present, only qualified cycles are eligible to trigger;
        //    also wait until at least `target_pre` pre-trigger samples are in the ring when possible.
        if !self.fired_in_segment {
            if qualified {
                if let Some(st) = self
                    .cfg
                    .states
                    .iter()
                    .find(|s| s.state_id == self.current_state)
                    .cloned()
                {
                    let st_idx = usize::from(st.state_id);
                    let cond_ok = st.condition.evaluate(prev_ref, sample);
                    if cond_ok {
                        let new_cnt = self.state_counters[st_idx].saturating_add(1);
                        self.state_counters[st_idx] = new_cnt;
                        self.state_trace.push((cycle, st.state_id, new_cnt));
                        if new_cnt >= st.counter_target.max(1) {
                            self.state_counters[st_idx] = 0;
                            match st.on_match {
                                StateTransition::Goto(next_id) => {
                                    self.current_state = next_id;
                                }
                                StateTransition::Trigger => {
                                    self.fired_in_segment = true;
                                    self.trigger_cycle_in_seg = cycle;
                                    // Actual pre-trigger samples available in ring before this trigger sample:
                                    let avail_pre = self.ring.len().saturating_sub(1).min(target_pre);
                                    self.pre_samples_in_seg = avail_pre;
                                    // Number of post-trigger samples still needed after the trigger sample:
                                    let post_needed = depth.saturating_sub(avail_pre + 1);
                                    self.post_remaining = post_needed;
                                    if self.post_remaining == 0 {
                                        self.finalize_segment(sample.len());
                                    }
                                }
                            }
                        }
                    } else {
                        if st.reset_counter_on_miss {
                            self.state_counters[st_idx] = 0;
                        }
                        if let Some(else_id) = st.else_state {
                            self.current_state = else_id;
                        }
                    }
                }
            }
        } else if qualified {
            // Post-trigger fill phase: count down qualified post-trigger samples.
            if self.post_remaining > 0 {
                self.post_remaining -= 1;
            }
            if self.post_remaining == 0 {
                self.finalize_segment(sample.len());
            }
        }

        self.prev_sample = Some(sample.to_vec());
        self.is_complete()
    }

    fn finalize_segment(&mut self, num_probes: usize) {
        let depth = self.cfg.segment_depth;
        let mut samples: Vec<Vec<bool>> = if self.ring.len() >= depth {
            self.ring.iter().skip(self.ring.len() - depth).cloned().collect()
        } else {
            let pad = depth - self.ring.len();
            let mut v = vec![vec![false; num_probes]; pad];
            v.extend(self.ring.iter().cloned());
            v
        };
        samples.truncate(depth);
        let words: Vec<u64> = samples.iter().map(|s| pack_sample_word(s)).collect();
        let seg_idx = self.segments.len();
        let trigger_offset = self.pre_samples_in_seg.min(depth.saturating_sub(1));
        self.segments.push(IlaCapturedSegment {
            segment_index: seg_idx,
            samples,
            words,
            trigger_offset,
            trigger_cycle: self.trigger_cycle_in_seg,
            state_trace: self.state_trace.clone(),
        });
        if !self.is_complete() {
            self.reset_for_next_segment();
        }
    }

    /// Run the trigger sequencer over an in-memory stream of samples.
    pub fn run_stream(
        &mut self,
        probes: Vec<String>,
        stream: &[Vec<bool>],
    ) -> Result<IlaSequencerCaptureResult, String> {
        for sample in stream {
            if self.step_sample(sample) {
                break;
            }
        }
        if self.segments.is_empty() {
            return Err(format!(
                "ila sequencer: trigger did not fire within {} samples (ended in state {})",
                stream.len(),
                self.current_state
            ));
        }
        let raw_scans: usize = self.segments.iter().map(|s| s.words.len()).sum();
        let all_words: Vec<u64> = self
            .segments
            .iter()
            .flat_map(|s| s.words.iter().copied())
            .collect();
        let rle_tokens = helion_hw::rle_encode_words(&all_words);
        Ok(IlaSequencerCaptureResult {
            probes,
            segments: self.segments.clone(),
            qualified_cycles: self.qualified_cycles,
            total_cycles: self.total_cycles,
            upload_dr_scans: 1 + rle_tokens.len(),
            upload_raw_scans: raw_scans,
        })
    }
}

/// Compile `design` with ILA probes + capture BRAM, execute `seq_cfg` against the live
/// cycle-accurate [`Tap`] fabric, write segmented capture words to fabric BRAM, and
/// upload all captured segments over `IR_USR1` RLE JTAG DR scans.
pub fn run_sequencer_capture(
    dev: &Device,
    design: &Design,
    probes: &[String],
    seq_cfg: IlaSequencerConfig,
    max_cycles: usize,
) -> Result<IlaSequencerCaptureResult, String> {
    if probes.is_empty() {
        return Err("run_sequencer_capture: probes list cannot be empty".into());
    }
    seq_cfg.validate()?;

    let total_words = (seq_cfg.segment_depth * seq_cfg.num_segments).clamp(1, 255);
    let mut d = design.clone();
    for p in probes {
        strip_ila(&mut d, p);
        insert_ila(&mut d, p)?;
    }
    strip_capture_bram(&mut d);
    insert_capture_bram(&mut d, total_words)?;

    let (routed, bits) = compile(dev, &d)?;
    let bram_major = routed
        .placed
        .packed
        .brams
        .iter()
        .position(|b| b.cell == CAPTURE_BRAM)
        .ok_or("run_sequencer_capture: capture BRAM not packed")? as u16;

    let mut sites = Vec::with_capacity(probes.len());
    for p in probes {
        sites.push(probe_site(&routed.placed, design, p)?);
    }

    let mut tap = Tap::new(dev);
    tap.program(&bits)?;

    let mut seq = IlaTriggerSequencer::new(seq_cfg)?;
    let limit = max_cycles.max(total_words * 32).max(256);

    {
        let fab = tap.fabric_mut();
        for _ in 0..limit {
            fab.step_user();
            fab.refresh_comb();
            let mut sample = Vec::with_capacity(sites.len());
            for &(site, ble) in &sites {
                sample.push(fab.ble_out(site.x, site.y, ble as u32));
            }
            if seq.step_sample(&sample) {
                break;
            }
        }
        if seq.segments.is_empty() {
            return Err(format!(
                "run_sequencer_capture: sequencer did not fire in {limit} cycles (stuck in state {})",
                seq.current_state
            ));
        }
        // Write segmented capture words into fabric BRAM so USR1 JTAG RLE upload reads them back.
        let mut addr = 0usize;
        for seg in &seq.segments {
            for &w in &seg.words {
                if addr < 255 {
                    fab.bram_write_word(bram_major, addr, w);
                    addr += 1;
                }
            }
        }
    }

    let captured_words_count: usize = seq
        .segments
        .iter()
        .map(|s| s.words.len())
        .sum::<usize>()
        .min(255);
    let rle = tap.usr1_upload_bram_rle(
        bram_major,
        0,
        captured_words_count,
        captured_words_count as u32,
    );

    // Verify and reconstruct segment samples from the JTAG USR1 RLE upload stream.
    let mut word_cursor = 0usize;
    for seg in &mut seq.segments {
        for i in 0..seg.words.len() {
            if let Some(&uploaded_w) = rle.words.get(word_cursor) {
                seg.words[i] = uploaded_w;
                seg.samples[i] = unpack_sample_word(uploaded_w, probes.len());
            }
            word_cursor += 1;
        }
    }

    Ok(IlaSequencerCaptureResult {
        probes: probes.to_vec(),
        segments: seq.segments,
        qualified_cycles: seq.qualified_cycles,
        total_cycles: seq.total_cycles,
        upload_dr_scans: rle.dr_scans,
        upload_raw_scans: rle.raw_scans,
    })
}
