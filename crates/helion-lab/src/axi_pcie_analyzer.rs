//! Vivado Hardware Manager AXI4 Traffic Generator & PG101 Protocol Checker +
//! PCIe Gen1–Gen4 LTSSM Link-Training State-Machine Trace Analyzer.
//!
//! Provides:
//! 1. [`AxiTrafficGenerator`] & [`AxiPerfMonitor`]: synthetic burst read/write traffic generation
//!    (`Sequential`, `Strided`, `Random`, `AddressSweep`) with 4KB boundary clamping, real-time
//!    throughput (MB/s & bus utilization %), read/write latency percentiles (`min`, `avg`, `p50`,
//!    `p99`, `max` cycles), and outstanding transaction depth histograms.
//! 2. [`AxiProtocolChecker`]: ARM IHI 0022E / Vivado PG101 protocol violation detector
//!    (`AXI_ERRM_AWADDR_BOUNDARY`, `AXI_ERRM_WDATA_STABLE`, `AXI_ERRM_WDATA_NUM`,
//!    `AXI_ERRM_EXCL_LEN`, `AXI_ERRM_AWADDR_WRAP_ALIGN`, and `MAXWAITS` handshake deadlocks).
//! 3. [`PcieLtssmAnalyzer`]: PCIe Gen1 (`2.5 GT/s`) through Gen4 (`16.0 GT/s`) LTSSM state
//!    transition decoder, speed upgrade/downgrade counter, Gen3/Gen4 Phase 0..3 equalization
//!    analyzer, and link-training failure diagnostician.

use std::collections::BTreeMap;

/// Address generation pattern for [`AxiTrafficGenerator`] (PG125).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxiTrafficPattern {
    /// Contiguous sequential bursts advancing by `burst_bytes`.
    Sequential,
    /// Strided bursts advancing by `stride_bytes`.
    Strided { stride_bytes: u64 },
    /// Pseudo-random 4KB-safe burst addresses seeded by `seed`.
    Random { seed: u64 },
    /// Sweeps `[low_addr, high_addr)` in increments of `step_bytes`, wrapping around.
    AddressSweep {
        low_addr: u64,
        high_addr: u64,
        step_bytes: u64,
    },
}

/// AXI4 burst type encoding (`AWBURST` / `ARBURST`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxiBurstKind {
    Fixed = 0b00,
    Incr = 0b01,
    Wrap = 0b10,
}

/// Configuration for [`AxiTrafficGenerator`] & [`AxiPerfMonitor`].
#[derive(Clone, Debug)]
pub struct AxiTrafficGenConfig {
    pub base_addr: u64,
    pub high_addr: u64,
    /// Bus data width in bits (`32`, `64`, `128`, `256`, `512`).
    pub data_width_bits: u16,
    /// Burst length in beats (`1..=256`, where `AWLEN = burst_len - 1`).
    pub burst_len: u16,
    pub pattern: AxiTrafficPattern,
    /// Percentage of generated transactions that are reads (`0..=100`).
    pub read_ratio_pct: u8,
    /// Fabric AXI clock frequency in MHz (e.g. `250.0`).
    pub clock_freq_mhz: f64,
    /// Target maximum outstanding transactions (`1..=64`).
    pub max_outstanding: usize,
}

impl Default for AxiTrafficGenConfig {
    fn default() -> Self {
        Self {
            base_addr: 0x4000_0000,
            high_addr: 0x4010_0000,
            data_width_bits: 64,
            burst_len: 16,
            pattern: AxiTrafficPattern::Sequential,
            read_ratio_pct: 50,
            clock_freq_mhz: 250.0,
            max_outstanding: 8,
        }
    }
}

/// One generated or monitored AXI4 burst transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiBurstTxn {
    pub id: u32,
    pub is_write: bool,
    pub addr: u64,
    /// Number of data beats (`1..=256`).
    pub beats: u16,
    /// Bytes per beat (`data_width_bits / 8`).
    pub beat_bytes: u16,
    pub burst_kind: AxiBurstKind,
    pub exclusive: bool,
    pub issue_cycle: u64,
    pub complete_cycle: u64,
    pub outstanding_at_issue: usize,
}

impl AxiBurstTxn {
    pub fn total_bytes(&self) -> u64 {
        u64::from(self.beats) * u64::from(self.beat_bytes)
    }

    pub fn latency_cycles(&self) -> u64 {
        self.complete_cycle.saturating_sub(self.issue_cycle)
    }
}

/// Latency distribution summary (`min`, `avg`, `p50`, `p99`, `max` in clock cycles).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LatencyStats {
    pub count: usize,
    pub min_cycles: u64,
    pub avg_cycles: f64,
    pub p50_cycles: u64,
    pub p99_cycles: u64,
    pub max_cycles: u64,
}

impl LatencyStats {
    pub fn from_samples(mut latencies: Vec<u64>) -> Self {
        if latencies.is_empty() {
            return Self::default();
        }
        latencies.sort_unstable();
        let count = latencies.len();
        let min_cycles = latencies[0];
        let max_cycles = *latencies.last().unwrap();
        let sum: u64 = latencies.iter().sum();
        let avg_cycles = (sum as f64) / (count as f64);
        let p50_idx = ((count - 1) * 50) / 100;
        let p99_idx = ((count - 1) * 99) / 100;
        Self {
            count,
            min_cycles,
            avg_cycles,
            p50_cycles: latencies[p50_idx],
            p99_cycles: latencies[p99_idx],
            max_cycles,
        }
    }
}

/// Performance report computed by [`AxiPerfMonitor`].
#[derive(Clone, Debug, PartialEq)]
pub struct AxiPerfReport {
    pub total_cycles: u64,
    pub active_data_beats: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_throughput_mbps: f64,
    pub write_throughput_mbps: f64,
    pub total_throughput_mbps: f64,
    pub bus_utilization_pct: f64,
    pub read_latency: LatencyStats,
    pub write_latency: LatencyStats,
    /// Histogram mapping `outstanding_depth -> transaction_count`.
    pub outstanding_histogram: BTreeMap<usize, usize>,
    pub peak_outstanding: usize,
}

/// Synthetic AXI4 Burst Traffic Generator (PG125 parity).
#[derive(Clone, Debug)]
pub struct AxiTrafficGenerator {
    pub cfg: AxiTrafficGenConfig,
    pub cursor_addr: u64,
    pub rng_state: u64,
    pub next_id: u32,
}

impl AxiTrafficGenerator {
    pub fn new(cfg: AxiTrafficGenConfig) -> Self {
        let seed = match cfg.pattern {
            AxiTrafficPattern::Random { seed } if seed != 0 => seed,
            _ => 0x9E37_79B9_7F4A_7C15,
        };
        let start = match cfg.pattern {
            AxiTrafficPattern::AddressSweep { low_addr, .. } => low_addr,
            _ => cfg.base_addr,
        };
        Self {
            cfg,
            cursor_addr: start,
            rng_state: seed,
            next_id: 1,
        }
    }

    fn next_rand(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Generate `count` synthetic AXI4 burst transactions and simulate their pipeline execution.
    pub fn generate_bursts(&mut self, count: usize) -> Vec<AxiBurstTxn> {
        let beat_bytes = (self.cfg.data_width_bits.max(32) / 8) as u64;
        let beats = self.cfg.burst_len.clamp(1, 256);
        let burst_bytes = u64::from(beats) * beat_bytes;
        let span = self
            .cfg
            .high_addr
            .saturating_sub(self.cfg.base_addr)
            .max(4096);

        let mut txns = Vec::with_capacity(count);
        let mut cur_cycle = 0u64;
        let max_out = self.cfg.max_outstanding.max(1);

        for idx in 0..count {
            let is_write = ((idx * 100) / count.max(1)) as u8 >= self.cfg.read_ratio_pct;
            let raw_addr = match self.cfg.pattern {
                AxiTrafficPattern::Sequential => {
                    let a = self.cursor_addr;
                    self.cursor_addr = self.cursor_addr.wrapping_add(burst_bytes);
                    if self.cursor_addr >= self.cfg.high_addr {
                        self.cursor_addr = self.cfg.base_addr;
                    }
                    a
                }
                AxiTrafficPattern::Strided { stride_bytes } => {
                    let a = self.cursor_addr;
                    self.cursor_addr = self.cursor_addr.wrapping_add(stride_bytes.max(burst_bytes));
                    if self.cursor_addr >= self.cfg.high_addr {
                        self.cursor_addr = self.cfg.base_addr;
                    }
                    a
                }
                AxiTrafficPattern::Random { .. } => {
                    let r = self.next_rand() % span;
                    self.cfg.base_addr + (r & !(beat_bytes - 1))
                }
                AxiTrafficPattern::AddressSweep {
                    low_addr,
                    high_addr,
                    step_bytes,
                } => {
                    let a = self.cursor_addr;
                    let next = self.cursor_addr.wrapping_add(step_bytes.max(burst_bytes));
                    self.cursor_addr = if next >= high_addr { low_addr } else { next };
                    a
                }
            };

            // Align address to beat size and prevent crossing a 4KB (0x1000) boundary.
            let mut aligned = raw_addr & !(beat_bytes - 1);
            let offset_in_4k = aligned & 0xFFF;
            if offset_in_4k + burst_bytes > 4096 {
                aligned = (aligned & !0xFFF) + (4096u64.saturating_sub(burst_bytes));
                aligned &= !(beat_bytes - 1);
            }

            let out_depth = (idx % max_out) + 1;
            let base_lat = if is_write { 12u64 } else { 18u64 };
            let queue_delay = (out_depth as u64) * 3 + ((idx as u64 * 7) % 11);
            let issue_cycle = cur_cycle;
            let complete_cycle = issue_cycle + base_lat + u64::from(beats) + queue_delay;
            cur_cycle += (u64::from(beats)).div_ceil(2).max(1);

            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            txns.push(AxiBurstTxn {
                id,
                is_write,
                addr: aligned,
                beats,
                beat_bytes: beat_bytes as u16,
                burst_kind: AxiBurstKind::Incr,
                exclusive: false,
                issue_cycle,
                complete_cycle,
                outstanding_at_issue: out_depth,
            });
        }
        txns
    }
}

/// AXI4 Performance Monitor computing throughput, utilization, latency percentiles, and outstanding depth.
#[derive(Clone, Debug, Default)]
pub struct AxiPerfMonitor {
    pub txns: Vec<AxiBurstTxn>,
}

impl AxiPerfMonitor {
    pub fn new() -> Self {
        Self { txns: Vec::new() }
    }

    pub fn record_txn(&mut self, txn: AxiBurstTxn) {
        self.txns.push(txn);
    }

    pub fn record_batch(&mut self, batch: impl IntoIterator<Item = AxiBurstTxn>) {
        self.txns.extend(batch);
    }

    pub fn analyze(&self, clock_freq_mhz: f64, data_width_bits: u16) -> AxiPerfReport {
        if self.txns.is_empty() {
            return AxiPerfReport {
                total_cycles: 0,
                active_data_beats: 0,
                read_bytes: 0,
                write_bytes: 0,
                read_throughput_mbps: 0.0,
                write_throughput_mbps: 0.0,
                total_throughput_mbps: 0.0,
                bus_utilization_pct: 0.0,
                read_latency: LatencyStats::default(),
                write_latency: LatencyStats::default(),
                outstanding_histogram: BTreeMap::new(),
                peak_outstanding: 0,
            };
        }

        let min_issue = self.txns.iter().map(|t| t.issue_cycle).min().unwrap_or(0);
        let max_complete = self
            .txns
            .iter()
            .map(|t| t.complete_cycle)
            .max()
            .unwrap_or(1);
        let total_cycles = max_complete.saturating_sub(min_issue).max(1);

        let mut read_bytes = 0u64;
        let mut write_bytes = 0u64;
        let mut active_data_beats = 0u64;
        let mut rd_lats = Vec::new();
        let mut wr_lats = Vec::new();
        let mut outstanding_histogram = BTreeMap::new();
        let mut peak_outstanding = 0usize;

        for t in &self.txns {
            active_data_beats += u64::from(t.beats);
            if t.is_write {
                write_bytes += t.total_bytes();
                wr_lats.push(t.latency_cycles());
            } else {
                read_bytes += t.total_bytes();
                rd_lats.push(t.latency_cycles());
            }
            *outstanding_histogram
                .entry(t.outstanding_at_issue)
                .or_insert(0) += 1;
            peak_outstanding = peak_outstanding.max(t.outstanding_at_issue);
        }

        let freq_hz = clock_freq_mhz.max(1.0) * 1e6;
        let duration_sec = (total_cycles as f64) / freq_hz;
        let read_throughput_mbps = (read_bytes as f64) / duration_sec / 1e6;
        let write_throughput_mbps = (write_bytes as f64) / duration_sec / 1e6;
        let total_throughput_mbps = read_throughput_mbps + write_throughput_mbps;

        let _ = data_width_bits;
        let bus_utilization_pct =
            (((active_data_beats as f64) / (total_cycles as f64)) * 100.0).clamp(0.0, 100.0);

        AxiPerfReport {
            total_cycles,
            active_data_beats,
            read_bytes,
            write_bytes,
            read_throughput_mbps,
            write_throughput_mbps,
            total_throughput_mbps,
            bus_utilization_pct,
            read_latency: LatencyStats::from_samples(rd_lats),
            write_latency: LatencyStats::from_samples(wr_lats),
            outstanding_histogram,
            peak_outstanding,
        }
    }
}

/// ARM IHI 0022E / Vivado PG101 AXI4 Protocol Violation classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxiViolationKind {
    /// `AXI_ERRM_AWADDR_BOUNDARY` / `AXI_ERRM_ARADDR_BOUNDARY`: burst crosses a 4KB (`0x1000`) boundary.
    AwaddrBoundary4K,
    /// `AXI_ERRM_WDATA_STABLE`: `WDATA`/`WSTRB`/`WLAST` changed while `WVALID && !WREADY`.
    WdataUnstable,
    /// `AXI_ERRM_WDATA_NUM`: number of write beats before `WLAST` does not match `AWLEN + 1`.
    WdataNumMismatch,
    /// `AXI_ERRM_EXCL_LEN` / `AXI_ERRM_EXCL_ALIGN`: invalid exclusive access length or alignment.
    ExclAccessInvalid,
    /// `AXI_ERRM_AWADDR_WRAP_ALIGN`: `WRAP` burst start address not aligned or length not in `{2,4,8,16}`.
    UnalignedWrapBurst,
    /// `AXI_RECS_MAXWAITS`: `VALID && !READY` stalled for `>= max_waits_cycles` consecutive cycles.
    MaxWaitsDeadlock,
}

impl AxiViolationKind {
    pub fn pg101_code(self) -> &'static str {
        match self {
            Self::AwaddrBoundary4K => "AXI_ERRM_AWADDR_BOUNDARY",
            Self::WdataUnstable => "AXI_ERRM_WDATA_STABLE",
            Self::WdataNumMismatch => "AXI_ERRM_WDATA_NUM",
            Self::ExclAccessInvalid => "AXI_ERRM_EXCL_ACCESS",
            Self::UnalignedWrapBurst => "AXI_ERRM_AWADDR_WRAP_ALIGN",
            Self::MaxWaitsDeadlock => "AXI_RECS_MAXWAITS_DEADLOCK",
        }
    }
}

/// Recorded PG101 AXI4 protocol violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiProtocolViolation {
    pub kind: AxiViolationKind,
    pub code: &'static str,
    pub cycle: u64,
    pub detail: String,
}

/// Cycle-level write-data channel sample for [`AxiProtocolChecker`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxiWriteChannelSample {
    pub cycle: u64,
    pub wvalid: bool,
    pub wready: bool,
    pub wdata: u64,
    pub wstrb: u8,
    pub wlast: bool,
}

/// Vivado PG101 AXI4 Protocol Checker.
#[derive(Clone, Debug)]
pub struct AxiProtocolChecker {
    pub max_waits_cycles: u32,
    pub violations: Vec<AxiProtocolViolation>,
    prev_stalled_w: Option<AxiWriteChannelSample>,
    consecutive_stall_cycles: u32,
}

impl Default for AxiProtocolChecker {
    fn default() -> Self {
        Self::new(64)
    }
}

impl AxiProtocolChecker {
    pub fn new(max_waits_cycles: u32) -> Self {
        Self {
            max_waits_cycles: max_waits_cycles.max(1),
            violations: Vec::new(),
            prev_stalled_w: None,
            consecutive_stall_cycles: 0,
        }
    }

    /// Check address phase rules (`AXI_ERRM_AWADDR_BOUNDARY`, `AXI_ERRM_EXCL_ACCESS`, `AXI_ERRM_AWADDR_WRAP_ALIGN`).
    pub fn check_address_phase(&mut self, txn: &AxiBurstTxn) {
        let total_bytes = txn.total_bytes();
        let end_addr = txn.addr.saturating_add(total_bytes.saturating_sub(1));

        // 1. 4KB boundary check (Fixed bursts do not increment address).
        if txn.burst_kind != AxiBurstKind::Fixed && (txn.addr >> 12) != (end_addr >> 12) {
            let kind = AxiViolationKind::AwaddrBoundary4K;
            self.violations.push(AxiProtocolViolation {
                kind,
                code: kind.pg101_code(),
                cycle: txn.issue_cycle,
                detail: format!(
                    "Burst ID={} at {:#010x} ({} beats x {}B) crosses 4KB boundary into {:#010x}",
                    txn.id, txn.addr, txn.beats, txn.beat_bytes, end_addr
                ),
            });
        }

        // 2. Exclusive access rules: max 16 beats, total bytes power-of-two <= 128, aligned to total_bytes.
        if txn.exclusive {
            let valid_excl = txn.beats <= 16
                && total_bytes <= 128
                && total_bytes.is_power_of_two()
                && (txn.addr % total_bytes == 0);
            if !valid_excl {
                let kind = AxiViolationKind::ExclAccessInvalid;
                self.violations.push(AxiProtocolViolation {
                    kind,
                    code: kind.pg101_code(),
                    cycle: txn.issue_cycle,
                    detail: format!(
                        "Exclusive burst ID={} at {:#010x} (beats={}, bytes={total_bytes}) violates ARM IHI 0022E exclusive alignment/length",
                        txn.id, txn.addr, txn.beats
                    ),
                });
            }
        }

        // 3. WRAP burst alignment & length in {2, 4, 8, 16}.
        if txn.burst_kind == AxiBurstKind::Wrap {
            let aligned = txn.addr % u64::from(txn.beat_bytes.max(1)) == 0;
            let legal_len = matches!(txn.beats, 2 | 4 | 8 | 16);
            if !aligned || !legal_len {
                let kind = AxiViolationKind::UnalignedWrapBurst;
                self.violations.push(AxiProtocolViolation {
                    kind,
                    code: kind.pg101_code(),
                    cycle: txn.issue_cycle,
                    detail: format!(
                        "WRAP burst ID={} at {:#010x} has beats={} (must be aligned and length in {{2,4,8,16}})",
                        txn.id, txn.addr, txn.beats
                    ),
                });
            }
        }
    }

    /// Verify write data burst beat count (`AXI_ERRM_WDATA_NUM`), payload stability under backpressure
    /// (`AXI_ERRM_WDATA_STABLE`), and handshake deadlock (`AXI_RECS_MAXWAITS_DEADLOCK`).
    pub fn check_write_stream(
        &mut self,
        expected_beats: u16,
        samples: &[AxiWriteChannelSample],
    ) {
        let mut accepted_beats = 0u16;
        for &s in samples {
            // Check payload stability when previous cycle had WVALID=1 && WREADY=0.
            if let Some(prev) = self.prev_stalled_w {
                if !s.wvalid || s.wdata != prev.wdata || s.wstrb != prev.wstrb || s.wlast != prev.wlast {
                    let kind = AxiViolationKind::WdataUnstable;
                    self.violations.push(AxiProtocolViolation {
                        kind,
                        code: kind.pg101_code(),
                        cycle: s.cycle,
                        detail: format!(
                            "Write channel changed while stalled (WVALID=1, WREADY=0): prev=({:#x}, strb={:#x}, last={}) curr=({:#x}, strb={:#x}, last={})",
                            prev.wdata, prev.wstrb, prev.wlast, s.wdata, s.wstrb, s.wlast
                        ),
                    });
                }
            }

            if s.wvalid && !s.wready {
                self.prev_stalled_w = Some(s);
                self.consecutive_stall_cycles += 1;
                if self.consecutive_stall_cycles == self.max_waits_cycles {
                    let kind = AxiViolationKind::MaxWaitsDeadlock;
                    self.violations.push(AxiProtocolViolation {
                        kind,
                        code: kind.pg101_code(),
                        cycle: s.cycle,
                        detail: format!(
                            "WVALID held without WREADY for {} consecutive cycles (MAXWAITS deadlock)",
                            self.consecutive_stall_cycles
                        ),
                    });
                }
            } else {
                self.prev_stalled_w = None;
                self.consecutive_stall_cycles = 0;
            }

            if s.wvalid && s.wready {
                accepted_beats += 1;
                if s.wlast {
                    if accepted_beats != expected_beats {
                        let kind = AxiViolationKind::WdataNumMismatch;
                        self.violations.push(AxiProtocolViolation {
                            kind,
                            code: kind.pg101_code(),
                            cycle: s.cycle,
                            detail: format!(
                                "WLAST asserted on beat {accepted_beats}, but AWLEN+1 expected {expected_beats} beats"
                            ),
                        });
                    }
                    accepted_beats = 0;
                } else if accepted_beats == expected_beats {
                    let kind = AxiViolationKind::WdataNumMismatch;
                    self.violations.push(AxiProtocolViolation {
                        kind,
                        code: kind.pg101_code(),
                        cycle: s.cycle,
                        detail: format!(
                            "Final beat {expected_beats} completed without WLAST asserted"
                        ),
                    });
                }
            }
        }
    }
}

/// PCIe Link Speed generation (`Gen1` = 2.5 GT/s .. `Gen4` = 16.0 GT/s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PcieGenSpeed {
    Gen1 = 1,
    Gen2 = 2,
    Gen3 = 3,
    Gen4 = 4,
}

impl PcieGenSpeed {
    pub fn gt_per_sec(self) -> f64 {
        match self {
            Self::Gen1 => 2.5,
            Self::Gen2 => 5.0,
            Self::Gen3 => 8.0,
            Self::Gen4 => 16.0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gen1 => "Gen1 (2.5 GT/s)",
            Self::Gen2 => "Gen2 (5.0 GT/s)",
            Self::Gen3 => "Gen3 (8.0 GT/s)",
            Self::Gen4 => "Gen4 (16.0 GT/s)",
        }
    }
}

/// PCIe Base Specification Link Training and Status State Machine (LTSSM) state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcieLtssmState {
    DetectQuiet,
    DetectActive,
    PollingActive,
    PollingConfiguration,
    PollingCompliance,
    ConfigurationLinkwidthStart,
    ConfigurationLinkwidthAccept,
    ConfigurationLanenumWait,
    ConfigurationLanenumAccept,
    ConfigurationComplete,
    ConfigurationIdle,
    L0,
    L0s,
    L1,
    L2,
    RecoveryRcvrLock,
    RecoverySpeed,
    RecoveryEqualizationPhase0,
    RecoveryEqualizationPhase1,
    RecoveryEqualizationPhase2,
    RecoveryEqualizationPhase3,
    RecoveryRcvrCfg,
    RecoveryIdle,
    HotReset,
    Disabled,
    Loopback,
}

impl PcieLtssmState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DetectQuiet => "Detect.Quiet",
            Self::DetectActive => "Detect.Active",
            Self::PollingActive => "Polling.Active",
            Self::PollingConfiguration => "Polling.Configuration",
            Self::PollingCompliance => "Polling.Compliance",
            Self::ConfigurationLinkwidthStart => "Configuration.Linkwidth.Start",
            Self::ConfigurationLinkwidthAccept => "Configuration.Linkwidth.Accept",
            Self::ConfigurationLanenumWait => "Configuration.Lanenum.Wait",
            Self::ConfigurationLanenumAccept => "Configuration.Lanenum.Accept",
            Self::ConfigurationComplete => "Configuration.Complete",
            Self::ConfigurationIdle => "Configuration.Idle",
            Self::L0 => "L0",
            Self::L0s => "L0s",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::RecoveryRcvrLock => "Recovery.RcvrLock",
            Self::RecoverySpeed => "Recovery.Speed",
            Self::RecoveryEqualizationPhase0 => "Recovery.Equalization.Phase0",
            Self::RecoveryEqualizationPhase1 => "Recovery.Equalization.Phase1",
            Self::RecoveryEqualizationPhase2 => "Recovery.Equalization.Phase2",
            Self::RecoveryEqualizationPhase3 => "Recovery.Equalization.Phase3",
            Self::RecoveryRcvrCfg => "Recovery.RcvrCfg",
            Self::RecoveryIdle => "Recovery.Idle",
            Self::HotReset => "HotReset",
            Self::Disabled => "Disabled",
            Self::Loopback => "Loopback",
        }
    }
}

/// One captured PIPE / LTSSM hardware trace sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcieTraceSample {
    pub timestamp_ns: u64,
    pub state: PcieLtssmState,
    pub speed: PcieGenSpeed,
    pub link_width: u8,
    /// Active Gen3/Gen4 TX equalization preset (`0..=10`, or `0xFF` if invalid/unset).
    pub tx_eq_preset: u8,
    /// Receiver eye Figure of Merit (FOM in mV*UI; `< 20` indicates marginal equalization).
    pub rx_eq_fom: u16,
}

/// Diagnosed PCIe link-training or equalization anomaly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PcieLinkIssue {
    /// `Polling.Active` timed out into `Polling.Compliance`.
    PollingEnteredCompliance { timestamp_ns: u64 },
    /// Gen3/Gen4 equalization phase failed and caused a link speed fallback.
    EqualizationFallbackDowngrade {
        timestamp_ns: u64,
        from_speed: PcieGenSpeed,
        to_speed: PcieGenSpeed,
    },
    /// Invalid TX equalization preset (`> 10`) or poor RX FOM (`< 20`) during Phase 2/3.
    EqualizationPresetFailure {
        timestamp_ns: u64,
        phase: PcieLtssmState,
        preset: u8,
        fom: u16,
    },
    /// Excessive `L0 -> Recovery.RcvrLock` retrain events indicating signal integrity loss.
    ExcessiveRecoveryRetrains { count: usize },
}

/// Summary report produced by [`PcieLtssmAnalyzer`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcieLtssmReport {
    pub transitions: Vec<(u64, PcieLtssmState, PcieLtssmState)>,
    pub reached_l0: bool,
    pub max_speed: PcieGenSpeed,
    pub final_speed: PcieGenSpeed,
    pub final_link_width: u8,
    pub speed_upgrades: usize,
    pub speed_downgrades: usize,
    pub eq_phases_visited: Vec<u8>,
    pub recovery_retrain_count: usize,
    pub issues: Vec<PcieLinkIssue>,
}

/// PCIe Gen1–Gen4 LTSSM Link-Training State-Machine Trace Analyzer.
#[derive(Clone, Debug, Default)]
pub struct PcieLtssmAnalyzer {
    pub samples: Vec<PcieTraceSample>,
}

impl PcieLtssmAnalyzer {
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    pub fn push_sample(&mut self, sample: PcieTraceSample) {
        self.samples.push(sample);
    }

    pub fn analyze(&self) -> PcieLtssmReport {
        if self.samples.is_empty() {
            return PcieLtssmReport {
                transitions: Vec::new(),
                reached_l0: false,
                max_speed: PcieGenSpeed::Gen1,
                final_speed: PcieGenSpeed::Gen1,
                final_link_width: 0,
                speed_upgrades: 0,
                speed_downgrades: 0,
                eq_phases_visited: Vec::new(),
                recovery_retrain_count: 0,
                issues: Vec::new(),
            };
        }

        let mut transitions = Vec::new();
        let mut reached_l0 = false;
        let mut max_speed = self.samples[0].speed;
        let mut speed_upgrades = 0usize;
        let mut speed_downgrades = 0usize;
        let mut eq_phases_visited = Vec::new();
        let mut recovery_retrain_count = 0usize;
        let mut issues = Vec::new();

        for (idx, s) in self.samples.iter().enumerate() {
            if s.state == PcieLtssmState::L0 {
                reached_l0 = true;
            }
            if s.speed > max_speed {
                max_speed = s.speed;
            }

            let eq_phase_num = match s.state {
                PcieLtssmState::RecoveryEqualizationPhase0 => Some(0u8),
                PcieLtssmState::RecoveryEqualizationPhase1 => Some(1u8),
                PcieLtssmState::RecoveryEqualizationPhase2 => Some(2u8),
                PcieLtssmState::RecoveryEqualizationPhase3 => Some(3u8),
                _ => None,
            };
            if let Some(ph) = eq_phase_num {
                if !eq_phases_visited.contains(&ph) {
                    eq_phases_visited.push(ph);
                }
                if (ph == 2 || ph == 3) && (s.tx_eq_preset > 10 || s.rx_eq_fom < 20) {
                    issues.push(PcieLinkIssue::EqualizationPresetFailure {
                        timestamp_ns: s.timestamp_ns,
                        phase: s.state,
                        preset: s.tx_eq_preset,
                        fom: s.rx_eq_fom,
                    });
                }
            }

            if s.state == PcieLtssmState::PollingCompliance {
                issues.push(PcieLinkIssue::PollingEnteredCompliance {
                    timestamp_ns: s.timestamp_ns,
                });
            }

            if idx > 0 {
                let prev = self.samples[idx - 1];
                if prev.state != s.state {
                    transitions.push((s.timestamp_ns, prev.state, s.state));
                    if prev.state == PcieLtssmState::L0
                        && s.state == PcieLtssmState::RecoveryRcvrLock
                    {
                        recovery_retrain_count += 1;
                    }
                }
                if s.speed > prev.speed {
                    speed_upgrades += 1;
                } else if s.speed < prev.speed {
                    speed_downgrades += 1;
                    issues.push(PcieLinkIssue::EqualizationFallbackDowngrade {
                        timestamp_ns: s.timestamp_ns,
                        from_speed: prev.speed,
                        to_speed: s.speed,
                    });
                }
            }
        }

        if recovery_retrain_count >= 3 {
            issues.push(PcieLinkIssue::ExcessiveRecoveryRetrains {
                count: recovery_retrain_count,
            });
        }

        let last = self.samples.last().unwrap();
        PcieLtssmReport {
            transitions,
            reached_l0,
            max_speed,
            final_speed: last.speed,
            final_link_width: last.link_width,
            speed_upgrades,
            speed_downgrades,
            eq_phases_visited,
            recovery_retrain_count,
            issues,
        }
    }
}
