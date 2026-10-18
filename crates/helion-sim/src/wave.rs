//! Compact waveform delta streaming (FST-style binary archive + VCD interop),
//! fast O(log N) time slicing and glob search, and built-in bus protocol decoders
//! (AXI4-Lite, AXI4-Stream, UART, SPI, I2C, Wishbone).

use crate::fast::{CompiledSim, Logic4};
use helion_ir::Design;
use std::collections::{HashMap, VecDeque};

/// Compact waveform signal identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WaveSignalId(pub u32);

/// Metadata for a signal in a [`CompactWaveStream`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveSignalDef {
    pub id: WaveSignalId,
    pub name: String,
    pub width: u16,
}

/// One delta transition sample on a waveform signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaveSample {
    pub time_ps: u64,
    pub value: u64,
    pub xz_mask: u64,
}

impl WaveSample {
    pub const fn new_2state(time_ps: u64, value: u64) -> Self {
        Self {
            time_ps,
            value,
            xz_mask: 0,
        }
    }

    pub const fn new_4state(time_ps: u64, value: u64, xz_mask: u64) -> Self {
        Self {
            time_ps,
            value,
            xz_mask,
        }
    }

    #[inline]
    pub const fn is_known(self) -> bool {
        self.xz_mask == 0
    }
}

/// Time-window slice `[start_ps..=end_ps]` extracted from a [`CompactWaveStream`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveSlice {
    pub signal: WaveSignalId,
    pub start_ps: u64,
    pub end_ps: u64,
    pub initial_value: u64,
    pub initial_xz: u64,
    pub transitions: Vec<WaveSample>,
}

/// Compact delta-compressed waveform stream with O(log N) binary-searched time queries
/// and LEB128 FST-style binary serialization.
#[derive(Clone, Debug, Default)]
pub struct CompactWaveStream {
    pub timescale_ps: u64,
    pub duration_ps: u64,
    signals: Vec<WaveSignalDef>,
    name_to_id: HashMap<String, WaveSignalId>,
    traces: Vec<Vec<WaveSample>>,
}

fn write_varint(buf: &mut Vec<u8>, mut val: u64) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if val == 0 {
            break;
        }
    }
}

fn read_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if *pos >= bytes.len() {
            return Err("fst decode: unexpected EOF in varint".to_string());
        }
        let b = bytes[*pos];
        *pos += 1;
        result |= u64::from(b & 0x7F)
            .checked_shl(shift)
            .ok_or_else(|| "fst decode: varint overflow".to_string())?;
        if (b & 0x80) == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

/// Wildcard pattern matcher supporting `*` and `?` (case-insensitive).
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            pi += 1;
            star_ti = ti;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

impl CompactWaveStream {
    pub fn new(timescale_ps: u64) -> Self {
        Self {
            timescale_ps: timescale_ps.max(1),
            duration_ps: 0,
            signals: Vec::new(),
            name_to_id: HashMap::new(),
            traces: Vec::new(),
        }
    }

    /// Add or look up a signal definition with bit width `width`.
    pub fn add_signal(&mut self, name: impl Into<String>, width: u16) -> WaveSignalId {
        let name = name.into();
        if let Some(&id) = self.name_to_id.get(&name) {
            return id;
        }
        let id = WaveSignalId(self.signals.len() as u32);
        self.signals.push(WaveSignalDef {
            id,
            name: name.clone(),
            width: width.max(1),
        });
        self.name_to_id.insert(name, id);
        self.traces.push(Vec::new());
        id
    }

    /// All registered signal definitions.
    pub fn signals(&self) -> &[WaveSignalDef] {
        &self.signals
    }

    /// Look up a signal by exact name or hierarchical suffix (`top.sig` matches `sig`).
    pub fn find_signal(&self, name: &str) -> Option<WaveSignalId> {
        if let Some(&id) = self.name_to_id.get(name) {
            return Some(id);
        }
        let suffix_dot = format!(".{name}");
        let suffix_slash = format!("/{name}");
        self.signals
            .iter()
            .find(|s| {
                s.name.eq_ignore_ascii_case(name)
                    || s.name.ends_with(&suffix_dot)
                    || s.name.ends_with(&suffix_slash)
            })
            .map(|s| s.id)
    }

    /// Fast signal search: supports glob wildcards (`*`, `?`) or case-insensitive substring matching.
    pub fn search_signals(&self, query: &str) -> Vec<&WaveSignalDef> {
        let q = query.trim();
        if q.is_empty() || q == "*" {
            return self.signals.iter().collect();
        }
        let has_glob = q.contains('*') || q.contains('?');
        let q_lower = q.to_ascii_lowercase();
        self.signals
            .iter()
            .filter(|s| {
                if has_glob {
                    glob_match(q, &s.name)
                } else {
                    s.name.to_ascii_lowercase().contains(&q_lower)
                }
            })
            .collect()
    }

    /// Record a 2-state value change on `sig` at `time_ps`. Suppresses redundant consecutive values.
    #[inline]
    pub fn record_change(&mut self, sig: WaveSignalId, time_ps: u64, value: u64) {
        self.record_change_4state(sig, time_ps, value, 0);
    }

    /// Record a 4-state value change on `sig` at `time_ps`. Suppresses redundant consecutive values.
    #[inline]
    pub fn record_change_4state(
        &mut self,
        sig: WaveSignalId,
        time_ps: u64,
        value: u64,
        xz_mask: u64,
    ) {
        self.duration_ps = self.duration_ps.max(time_ps);
        let Some(trace) = self.traces.get_mut(sig.0 as usize) else {
            return;
        };
        if let Some(last) = trace.last_mut() {
            if last.value == value && last.xz_mask == xz_mask {
                return;
            }
            if last.time_ps == time_ps {
                last.value = value;
                last.xz_mask = xz_mask;
                // If coalescing reverted to the previous sample's value, pop the redundant entry.
                if trace.len() >= 2 {
                    let prev = trace[trace.len() - 2];
                    if prev.value == value && prev.xz_mask == xz_mask {
                        trace.pop();
                    }
                }
                return;
            }
        }
        trace.push(WaveSample {
            time_ps,
            value,
            xz_mask,
        });
    }

    /// Raw delta transitions for `sig`.
    pub fn transitions(&self, sig: WaveSignalId) -> &[WaveSample] {
        self.traces
            .get(sig.0 as usize)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Total number of recorded transitions across all signals.
    pub fn total_transitions(&self) -> usize {
        self.traces.iter().map(Vec::len).sum()
    }

    /// Query 4-state `(value, xz_mask)` of `sig` at `time_ps` in O(log N) via binary search.
    pub fn value_4state_at(&self, sig: WaveSignalId, time_ps: u64) -> Option<(u64, u64)> {
        let trace = self.traces.get(sig.0 as usize)?;
        if trace.is_empty() {
            return None;
        }
        let idx = trace.partition_point(|s| s.time_ps <= time_ps);
        if idx == 0 {
            Some((0, 0))
        } else {
            let s = trace[idx - 1];
            Some((s.value, s.xz_mask))
        }
    }

    /// Query 2-state value of `sig` at `time_ps` in O(log N) via binary search.
    #[inline]
    pub fn value_at(&self, sig: WaveSignalId, time_ps: u64) -> Option<u64> {
        self.value_4state_at(sig, time_ps).map(|(v, _)| v)
    }

    /// Reconstruct a multi-bit bus value at `time_ps` from LSB-ordered bit signals `bit_sigs`
    /// (or a single wide bus signal if `bit_sigs.len() == 1`).
    pub fn bus_value_at(&self, bit_sigs: &[WaveSignalId], time_ps: u64) -> u64 {
        if bit_sigs.len() == 1 {
            let sig = bit_sigs[0];
            if let Some(def) = self.signals.get(sig.0 as usize) {
                if def.width > 1 {
                    return self.value_at(sig, time_ps).unwrap_or(0);
                }
            }
        }
        let mut word = 0u64;
        for (i, &sig) in bit_sigs.iter().enumerate() {
            if i >= 64 {
                break;
            }
            if (self.value_at(sig, time_ps).unwrap_or(0) & 1) != 0 {
                word |= 1u64 << i;
            }
        }
        word
    }

    /// Extract a time slice `[start_ps..=end_ps]` for `sig` in O(log N + K) time.
    pub fn slice(&self, sig: WaveSignalId, start_ps: u64, end_ps: u64) -> Option<WaveSlice> {
        let trace = self.traces.get(sig.0 as usize)?;
        let (initial_value, initial_xz) = self.value_4state_at(sig, start_ps).unwrap_or((0, 0));
        let lo = trace.partition_point(|s| s.time_ps <= start_ps);
        let hi = trace.partition_point(|s| s.time_ps <= end_ps);
        let transitions = if lo < hi {
            trace[lo..hi].to_vec()
        } else {
            Vec::new()
        };
        Some(WaveSlice {
            signal: sig,
            start_ps,
            end_ps,
            initial_value,
            initial_xz,
            transitions,
        })
    }

    /// Find the next transition timestamp strictly after `after_ps` (optionally filtered to
    /// `rising = Some(true)` for `0 -> 1` or `Some(false)` for `1 -> 0`).
    pub fn next_edge(
        &self,
        sig: WaveSignalId,
        after_ps: u64,
        rising: Option<bool>,
    ) -> Option<u64> {
        let trace = self.traces.get(sig.0 as usize)?;
        let start_idx = trace.partition_point(|s| s.time_ps <= after_ps);
        for i in start_idx..trace.len() {
            let cur = trace[i];
            match rising {
                None => return Some(cur.time_ps),
                Some(want_rise) => {
                    let prev_val = if i > 0 { trace[i - 1].value & 1 } else { 0 };
                    let cur_val = cur.value & 1;
                    if want_rise && prev_val == 0 && cur_val == 1 {
                        return Some(cur.time_ps);
                    }
                    if !want_rise && prev_val == 1 && cur_val == 0 {
                        return Some(cur.time_ps);
                    }
                }
            }
        }
        None
    }

    /// Return all rising-edge (`0 -> 1`) timestamps of a clock signal `clk_sig`.
    pub fn clock_posedges(&self, clk_sig: WaveSignalId) -> Vec<u64> {
        let Some(trace) = self.traces.get(clk_sig.0 as usize) else {
            return Vec::new();
        };
        let mut edges = Vec::new();
        let mut prev = 0u64;
        for (i, sample) in trace.iter().enumerate() {
            let cur = sample.value & 1;
            if i > 0 && prev == 0 && cur == 1 {
                edges.push(sample.time_ps);
            }
            prev = cur;
        }
        edges
    }

    /// Run [`CompiledSim`] for `cycles` clock cycles of `period_ps` and stream delta transitions
    /// directly into a [`CompactWaveStream`].
    pub fn capture_compiled(design: &Design, cycles: u32, period_ps: u64) -> Self {
        let period = period_ps.max(1);
        let half = (period / 2).max(1);
        let mut sim = CompiledSim::new(design);
        let mut stream = Self::new(1);

        let clk_id = stream.add_signal("clk", 1);
        let led_id = stream.add_signal("led", 1);

        let mut net_to_sig: Vec<(u32, WaveSignalId)> = Vec::new();
        let mut sorted_nets: Vec<(String, u32)> = sim
            .schedule
            .net_ids
            .iter()
            .filter(|(n, id)| {
                **id >= 2 && *n != "clk" && *n != "led" && *n != "0" && *n != "1"
            })
            .map(|(n, id)| (n.clone(), *id))
            .collect();
        sorted_nets.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, net_id) in sorted_nets {
            let sig = stream.add_signal(name, 1);
            net_to_sig.push((net_id, sig));
        }

        // Initial t=0 dumpvars
        stream.record_change(clk_id, 0, 0);
        stream.record_change(led_id, 0, u64::from(sim.led));
        for &(net_id, sig) in &net_to_sig {
            let v = sim.net_logic4(net_id);
            let (val, xz) = v.to_planes();
            stream.record_change_4state(sig, 0, u64::from(val), u64::from(xz));
        }

        for i in 0..cycles {
            let t_pos = u64::from(i) * period;
            stream.record_change(clk_id, t_pos, 1);
            sim.step_2state(period);
            stream.record_change(led_id, t_pos, u64::from(sim.led));
            for &(net_id, sig) in &net_to_sig {
                stream.record_change(sig, t_pos, u64::from(sim.net_bit(net_id)));
            }
            let t_neg = t_pos + half;
            stream.record_change(clk_id, t_neg, 0);
        }

        stream
    }

    /// Encode this waveform stream into a compact FST-style binary archive (`HFST\x01`).
    /// Uses LEB128 varint delta timestamps per signal block for 5x-20x compression over VCD.
    pub fn to_fst_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"HFST\x01");
        write_varint(&mut out, self.timescale_ps);
        write_varint(&mut out, self.duration_ps);
        write_varint(&mut out, self.signals.len() as u64);

        for sig in &self.signals {
            let name_bytes = sig.name.as_bytes();
            write_varint(&mut out, name_bytes.len() as u64);
            out.extend_from_slice(name_bytes);
            write_varint(&mut out, u64::from(sig.width));
        }

        for trace in &self.traces {
            write_varint(&mut out, trace.len() as u64);
            let mut prev_t = 0u64;
            for sample in trace {
                let dt = sample.time_ps.saturating_sub(prev_t);
                prev_t = sample.time_ps;
                write_varint(&mut out, dt);
                write_varint(&mut out, sample.value);
                write_varint(&mut out, sample.xz_mask);
            }
        }
        out
    }

    /// Decode a [`CompactWaveStream`] from compact FST-style binary bytes produced by [`Self::to_fst_bytes`].
    pub fn from_fst_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 5 || &bytes[0..5] != b"HFST\x01" {
            return Err("fst decode: invalid HFST magic header".to_string());
        }
        let mut pos = 5;
        let timescale_ps = read_varint(bytes, &mut pos)?;
        let duration_ps = read_varint(bytes, &mut pos)?;
        let num_sigs = read_varint(bytes, &mut pos)? as usize;

        let mut stream = Self::new(timescale_ps);
        stream.duration_ps = duration_ps;

        for _ in 0..num_sigs {
            let name_len = read_varint(bytes, &mut pos)? as usize;
            if pos + name_len > bytes.len() {
                return Err("fst decode: truncated signal name".to_string());
            }
            let name = String::from_utf8_lossy(&bytes[pos..pos + name_len]).to_string();
            pos += name_len;
            let width = read_varint(bytes, &mut pos)? as u16;
            stream.add_signal(name, width);
        }

        for sig_idx in 0..num_sigs {
            let n_samples = read_varint(bytes, &mut pos)? as usize;
            let mut t = 0u64;
            let mut trace = Vec::with_capacity(n_samples);
            for _ in 0..n_samples {
                let dt = read_varint(bytes, &mut pos)?;
                let value = read_varint(bytes, &mut pos)?;
                let xz_mask = read_varint(bytes, &mut pos)?;
                t = t.saturating_add(dt);
                trace.push(WaveSample {
                    time_ps: t,
                    value,
                    xz_mask,
                });
            }
            stream.traces[sig_idx] = trace;
        }

        Ok(stream)
    }

    /// Export this [`CompactWaveStream`] as standard IEEE 1364 VCD text.
    pub fn to_vcd(&self, module_name: &str) -> String {
        let mut out = String::new();
        out.push_str("$date\n    helion-sim\n$end\n");
        out.push_str("$version\n    helion-sim CompactWaveStream\n$end\n");
        out.push_str(&format!("$timescale {}ps $end\n", self.timescale_ps));
        out.push_str(&format!("$scope module {module_name} $end\n"));

        let ids: Vec<String> = (0..self.signals.len()).map(crate::vcd::vcd_id_code).collect();
        for (sig, id) in self.signals.iter().zip(ids.iter()) {
            out.push_str(&format!("$var wire {} {id} {} $end\n", sig.width, sig.name));
        }
        out.push_str("$upscope $end\n$enddefinitions $end\n");

        // Gather all events sorted by timestamp.
        let mut events_by_time: std::collections::BTreeMap<u64, Vec<(usize, WaveSample)>> =
            std::collections::BTreeMap::new();
        for (sig_idx, trace) in self.traces.iter().enumerate() {
            for &sample in trace {
                events_by_time
                    .entry(sample.time_ps)
                    .or_default()
                    .push((sig_idx, sample));
            }
        }

        let fmt_change = |width: u16, id: &str, s: WaveSample| -> String {
            if width <= 1 {
                let ch = Logic4::from_planes((s.value & 1) != 0, (s.xz_mask & 1) != 0).to_char();
                format!("{ch}{id}\n")
            } else {
                let mut bits = String::with_capacity(width as usize);
                for b in (0..width).rev() {
                    let v = ((s.value >> b) & 1) != 0;
                    let xz = ((s.xz_mask >> b) & 1) != 0;
                    bits.push(Logic4::from_planes(v, xz).to_char());
                }
                format!("b{bits} {id}\n")
            }
        };

        out.push_str("#0\n$dumpvars\n");
        if let Some(t0) = events_by_time.remove(&0) {
            for (sig_idx, sample) in t0 {
                out.push_str(&fmt_change(
                    self.signals[sig_idx].width,
                    &ids[sig_idx],
                    sample,
                ));
            }
        }
        out.push_str("$end\n");

        for (t, evs) in events_by_time {
            out.push_str(&format!("#{t}\n"));
            for (sig_idx, sample) in evs {
                out.push_str(&fmt_change(
                    self.signals[sig_idx].width,
                    &ids[sig_idx],
                    sample,
                ));
            }
        }
        out
    }
}

// ============================================================================
// Built-in Waveform Protocol Decoders
// ============================================================================

/// High-level protocol transaction reconstructed from waveform traces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolTransaction {
    AxiLite(AxiLiteTransaction),
    AxiStream(AxiStreamPacket),
    Uart(UartFrame),
    Spi(SpiTransaction),
    I2c(I2cTransaction),
    Wishbone(WishboneTransaction),
}

// --- 1. AXI4-Lite Protocol Decoder ---

/// Signal binding for an AXI4-Lite bus interface.
#[derive(Clone, Debug)]
pub struct AxiLiteSignals {
    pub aclk: WaveSignalId,
    pub aresetn: Option<WaveSignalId>,
    pub awvalid: WaveSignalId,
    pub awready: WaveSignalId,
    pub awaddr: WaveSignalId,
    pub wvalid: WaveSignalId,
    pub wready: WaveSignalId,
    pub wdata: WaveSignalId,
    pub wstrb: Option<WaveSignalId>,
    pub bvalid: WaveSignalId,
    pub bready: WaveSignalId,
    pub bresp: Option<WaveSignalId>,
    pub arvalid: WaveSignalId,
    pub arready: WaveSignalId,
    pub araddr: WaveSignalId,
    pub rvalid: WaveSignalId,
    pub rready: WaveSignalId,
    pub rdata: WaveSignalId,
    pub rresp: Option<WaveSignalId>,
}

/// Direction of an AXI4-Lite bus transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxiLiteTxKind {
    Read,
    Write,
}

/// Reconstructed AXI4-Lite read or write transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiLiteTransaction {
    pub kind: AxiLiteTxKind,
    pub start_ps: u64,
    pub end_ps: u64,
    pub addr: u64,
    pub data: u64,
    pub strb: u64,
    /// 2-bit AXI response (`00` = OKAY, `01` = EXOKAY, `10` = SLVERR, `11` = DECERR).
    pub resp: u8,
}

/// Decode AXI4-Lite read and write transactions from a [`CompactWaveStream`].
pub fn decode_axi_lite(
    stream: &CompactWaveStream,
    sigs: &AxiLiteSignals,
) -> Vec<AxiLiteTransaction> {
    let posedges = stream.clock_posedges(sigs.aclk);
    let mut aw_queue: VecDeque<(u64, u64)> = VecDeque::new();
    let mut w_queue: VecDeque<(u64, u64, u64)> = VecDeque::new();
    let mut b_pending: VecDeque<(u64, u64, u64, u64)> = VecDeque::new();
    let mut ar_queue: VecDeque<(u64, u64)> = VecDeque::new();
    let mut out = Vec::new();

    for t_edge in posedges {
        // Sample pre-edge values at `t_edge.saturating_sub(1)` (or `t_edge` if driven at edge).
        let sample_t = t_edge;
        if let Some(rst_n) = sigs.aresetn {
            if (stream.value_at(rst_n, sample_t).unwrap_or(1) & 1) == 0 {
                aw_queue.clear();
                w_queue.clear();
                b_pending.clear();
                ar_queue.clear();
                continue;
            }
        }

        // Write Address (AW) channel handshake
        if (stream.value_at(sigs.awvalid, sample_t).unwrap_or(0) & 1) != 0
            && (stream.value_at(sigs.awready, sample_t).unwrap_or(0) & 1) != 0
        {
            let addr = stream.value_at(sigs.awaddr, sample_t).unwrap_or(0);
            aw_queue.push_back((t_edge, addr));
        }

        // Write Data (W) channel handshake
        if (stream.value_at(sigs.wvalid, sample_t).unwrap_or(0) & 1) != 0
            && (stream.value_at(sigs.wready, sample_t).unwrap_or(0) & 1) != 0
        {
            let data = stream.value_at(sigs.wdata, sample_t).unwrap_or(0);
            let strb = sigs
                .wstrb
                .and_then(|s| stream.value_at(s, sample_t))
                .unwrap_or(0xF);
            w_queue.push_back((t_edge, data, strb));
        }

        // Pair completed AW + W into pending B response queue
        while !aw_queue.is_empty() && !w_queue.is_empty() {
            let (t_aw, addr) = aw_queue.pop_front().unwrap();
            let (t_w, data, strb) = w_queue.pop_front().unwrap();
            b_pending.push_back((t_aw.min(t_w), addr, data, strb));
        }

        // Write Response (B) channel handshake
        if (stream.value_at(sigs.bvalid, sample_t).unwrap_or(0) & 1) != 0
            && (stream.value_at(sigs.bready, sample_t).unwrap_or(0) & 1) != 0
        {
            if let Some((start_ps, addr, data, strb)) = b_pending.pop_front() {
                let resp = sigs
                    .bresp
                    .and_then(|s| stream.value_at(s, sample_t))
                    .unwrap_or(0) as u8
                    & 0b11;
                out.push(AxiLiteTransaction {
                    kind: AxiLiteTxKind::Write,
                    start_ps,
                    end_ps: t_edge,
                    addr,
                    data,
                    strb,
                    resp,
                });
            }
        }

        // Read Address (AR) channel handshake
        if (stream.value_at(sigs.arvalid, sample_t).unwrap_or(0) & 1) != 0
            && (stream.value_at(sigs.arready, sample_t).unwrap_or(0) & 1) != 0
        {
            let addr = stream.value_at(sigs.araddr, sample_t).unwrap_or(0);
            ar_queue.push_back((t_edge, addr));
        }

        // Read Data (R) channel handshake
        if (stream.value_at(sigs.rvalid, sample_t).unwrap_or(0) & 1) != 0
            && (stream.value_at(sigs.rready, sample_t).unwrap_or(0) & 1) != 0
        {
            if let Some((start_ps, addr)) = ar_queue.pop_front() {
                let data = stream.value_at(sigs.rdata, sample_t).unwrap_or(0);
                let resp = sigs
                    .rresp
                    .and_then(|s| stream.value_at(s, sample_t))
                    .unwrap_or(0) as u8
                    & 0b11;
                out.push(AxiLiteTransaction {
                    kind: AxiLiteTxKind::Read,
                    start_ps,
                    end_ps: t_edge,
                    addr,
                    data,
                    strb: 0,
                    resp,
                });
            }
        }
    }

    out
}

// --- 2. AXI4-Stream Protocol Decoder ---

/// Signal binding for an AXI4-Stream interface.
#[derive(Clone, Debug)]
pub struct AxiStreamSignals {
    pub aclk: WaveSignalId,
    pub tvalid: WaveSignalId,
    pub tready: WaveSignalId,
    pub tdata: WaveSignalId,
    pub tlast: Option<WaveSignalId>,
    pub tkeep: Option<WaveSignalId>,
    pub tuser: Option<WaveSignalId>,
}

/// Reconstructed AXI4-Stream packet (one or more beats terminated by `TLAST`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiStreamPacket {
    pub start_ps: u64,
    pub end_ps: u64,
    pub beats: Vec<u64>,
    pub keep: Vec<u64>,
    pub user: Vec<u64>,
    pub terminated_by_tlast: bool,
}

/// Decode AXI4-Stream beat transfers and packets from a [`CompactWaveStream`].
pub fn decode_axi_stream(
    stream: &CompactWaveStream,
    sigs: &AxiStreamSignals,
) -> Vec<AxiStreamPacket> {
    let posedges = stream.clock_posedges(sigs.aclk);
    let mut packets = Vec::new();
    let mut cur_start: Option<u64> = None;
    let mut cur_end = 0u64;
    let mut beats = Vec::new();
    let mut keep = Vec::new();
    let mut user = Vec::new();

    for t_edge in posedges {
        let valid = (stream.value_at(sigs.tvalid, t_edge).unwrap_or(0) & 1) != 0;
        let ready = (stream.value_at(sigs.tready, t_edge).unwrap_or(0) & 1) != 0;
        if !valid || !ready {
            continue;
        }
        let k = sigs
            .tkeep
            .and_then(|s| stream.value_at(s, t_edge))
            .unwrap_or(!0);
        let u = sigs
            .tuser
            .and_then(|s| stream.value_at(s, t_edge))
            .unwrap_or(0);
        let d = stream.value_at(sigs.tdata, t_edge).unwrap_or(0);
        let last = sigs
            .tlast
            .map(|s| (stream.value_at(s, t_edge).unwrap_or(0) & 1) != 0)
            .unwrap_or(true);

        if cur_start.is_none() {
            cur_start = Some(t_edge);
        }
        cur_end = t_edge;
        beats.push(d);
        keep.push(k);
        user.push(u);

        if last {
            packets.push(AxiStreamPacket {
                start_ps: cur_start.take().unwrap_or(t_edge),
                end_ps: cur_end,
                beats: std::mem::take(&mut beats),
                keep: std::mem::take(&mut keep),
                user: std::mem::take(&mut user),
                terminated_by_tlast: sigs.tlast.is_some(),
            });
        }
    }

    if !beats.is_empty() {
        packets.push(AxiStreamPacket {
            start_ps: cur_start.unwrap_or(0),
            end_ps: cur_end,
            beats,
            keep,
            user,
            terminated_by_tlast: false,
        });
    }

    packets
}

// --- 3. UART Protocol Decoder ---

/// UART parity mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartParity {
    None,
    Even,
    Odd,
}

/// Configuration for decoding an asynchronous UART serial line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UartConfig {
    pub line: WaveSignalId,
    /// Duration of one bit cell in picoseconds (`1e12 / baud_rate`).
    pub bit_period_ps: u64,
    /// Number of data bits per frame (typically `8`).
    pub data_bits: u8,
    pub parity: UartParity,
    pub stop_bits: u8,
}

impl UartConfig {
    pub const fn new_8n1(line: WaveSignalId, bit_period_ps: u64) -> Self {
        Self {
            line,
            bit_period_ps,
            data_bits: 8,
            parity: UartParity::None,
            stop_bits: 1,
        }
    }
}

/// Reconstructed UART serial frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UartFrame {
    pub start_ps: u64,
    pub end_ps: u64,
    pub data: u16,
    pub parity_error: bool,
    pub framing_error: bool,
}

/// Decode UART serial frames from a [`CompactWaveStream`].
pub fn decode_uart(stream: &CompactWaveStream, cfg: &UartConfig) -> Vec<UartFrame> {
    let bit_ps = cfg.bit_period_ps.max(2);
    let half_ps = bit_ps / 2;
    let trace = stream.transitions(cfg.line);
    let mut frames = Vec::new();
    let mut cursor_ps = 0u64;

    for i in 0..trace.len() {
        let sample = trace[i];
        if sample.time_ps < cursor_ps {
            continue;
        }
        // Detect falling edge (`1 -> 0`) marking the start bit.
        let prev_val = if i > 0 { trace[i - 1].value & 1 } else { 1 };
        let cur_val = sample.value & 1;
        if !(prev_val == 1 && cur_val == 0) {
            continue;
        }

        let t_start = sample.time_ps;
        // Verify start bit is still low at mid-bit.
        if (stream.value_at(cfg.line, t_start + half_ps).unwrap_or(1) & 1) != 0 {
            continue;
        }

        let mut data = 0u16;
        let data_bits = cfg.data_bits.clamp(5, 9);
        for b in 0..data_bits {
            let t_sample = t_start + half_ps + u64::from(b + 1) * bit_ps;
            let bit = (stream.value_at(cfg.line, t_sample).unwrap_or(0) & 1) as u16;
            data |= bit << b;
        }

        let mut bit_offset = u64::from(data_bits) + 1;
        let mut parity_error = false;
        if cfg.parity != UartParity::None {
            let t_par = t_start + half_ps + bit_offset * bit_ps;
            let p_bit = (stream.value_at(cfg.line, t_par).unwrap_or(0) & 1) as u32;
            let ones = data.count_ones() + p_bit;
            match cfg.parity {
                UartParity::Even => {
                    if ones % 2 != 0 {
                        parity_error = true;
                    }
                }
                UartParity::Odd => {
                    if ones % 2 == 0 {
                        parity_error = true;
                    }
                }
                UartParity::None => {}
            }
            bit_offset += 1;
        }

        let stop_bits = cfg.stop_bits.max(1);
        let mut framing_error = false;
        for s in 0..stop_bits {
            let t_stop = t_start + half_ps + (bit_offset + u64::from(s)) * bit_ps;
            if (stream.value_at(cfg.line, t_stop).unwrap_or(0) & 1) == 0 {
                framing_error = true;
            }
        }
        let t_end = t_start + (bit_offset + u64::from(stop_bits)) * bit_ps;
        frames.push(UartFrame {
            start_ps: t_start,
            end_ps: t_end,
            data,
            parity_error,
            framing_error,
        });
        cursor_ps = t_end.saturating_sub(half_ps / 2);
    }

    frames
}

// --- 4. SPI Protocol Decoder ---

/// Configuration for decoding a 4-wire SPI bus (`SCLK`, `CS_N`, `MOSI`, optional `MISO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpiConfig {
    pub sclk: WaveSignalId,
    pub cs_n: WaveSignalId,
    pub mosi: WaveSignalId,
    pub miso: Option<WaveSignalId>,
    pub cpol: bool,
    pub cpha: bool,
    pub bits_per_word: u8,
    pub msb_first: bool,
}

impl SpiConfig {
    /// Standard SPI Mode 0 (`CPOL=0, CPHA=0`), 8-bit MSB-first.
    pub const fn mode0_8bit(
        sclk: WaveSignalId,
        cs_n: WaveSignalId,
        mosi: WaveSignalId,
        miso: Option<WaveSignalId>,
    ) -> Self {
        Self {
            sclk,
            cs_n,
            mosi,
            miso,
            cpol: false,
            cpha: false,
            bits_per_word: 8,
            msb_first: true,
        }
    }
}

/// Reconstructed SPI frame while `CS_N == 0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpiTransaction {
    pub start_ps: u64,
    pub end_ps: u64,
    pub mosi_words: Vec<u64>,
    pub miso_words: Vec<u64>,
}

/// Decode SPI transactions from a [`CompactWaveStream`].
pub fn decode_spi(stream: &CompactWaveStream, cfg: &SpiConfig) -> Vec<SpiTransaction> {
    let sample_on_rising = !(cfg.cpol ^ cfg.cpha);
    let sclk_trace = stream.transitions(cfg.sclk);
    let word_bits = cfg.bits_per_word.clamp(1, 64);

    let mut out = Vec::new();
    let mut active_start: Option<u64> = None;
    let mut last_sample_ps = 0u64;
    let mut bit_count = 0u8;
    let mut cur_mosi = 0u64;
    let mut cur_miso = 0u64;
    let mut mosi_words = Vec::new();
    let mut miso_words = Vec::new();

    for i in 1..sclk_trace.len() {
        let t = sclk_trace[i].time_ps;
        let cs_low = (stream.value_at(cfg.cs_n, t).unwrap_or(1) & 1) == 0;
        if !cs_low {
            if !mosi_words.is_empty() {
                out.push(SpiTransaction {
                    start_ps: active_start.take().unwrap_or(t),
                    end_ps: last_sample_ps,
                    mosi_words: std::mem::take(&mut mosi_words),
                    miso_words: std::mem::take(&mut miso_words),
                });
            }
            active_start = None;
            bit_count = 0;
            cur_mosi = 0;
            cur_miso = 0;
            continue;
        }

        let prev_clk = (sclk_trace[i - 1].value & 1) != 0;
        let cur_clk = (sclk_trace[i].value & 1) != 0;
        let is_rising = !prev_clk && cur_clk;
        let is_falling = prev_clk && !cur_clk;
        if (sample_on_rising && !is_rising) || (!sample_on_rising && !is_falling) {
            continue;
        }

        if active_start.is_none() {
            active_start = Some(t);
        }
        last_sample_ps = t;

        let mosi_bit = stream.value_at(cfg.mosi, t).unwrap_or(0) & 1;
        let miso_bit = cfg
            .miso
            .and_then(|m| stream.value_at(m, t))
            .unwrap_or(0)
            & 1;

        if cfg.msb_first {
            cur_mosi = (cur_mosi << 1) | mosi_bit;
            cur_miso = (cur_miso << 1) | miso_bit;
        } else {
            cur_mosi |= mosi_bit << bit_count;
            cur_miso |= miso_bit << bit_count;
        }
        bit_count += 1;

        if bit_count == word_bits {
            mosi_words.push(cur_mosi);
            miso_words.push(cur_miso);
            bit_count = 0;
            cur_mosi = 0;
            cur_miso = 0;
        }
    }

    if !mosi_words.is_empty() {
        out.push(SpiTransaction {
            start_ps: active_start.unwrap_or(0),
            end_ps: last_sample_ps,
            mosi_words,
            miso_words,
        });
    }

    out
}

// --- 5. I2C Protocol Decoder ---

/// Signal binding for a 2-wire I2C bus (`SCL`, `SDA`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I2cSignals {
    pub scl: WaveSignalId,
    pub sda: WaveSignalId,
}

/// One byte + ACK/NACK transfer inside an [`I2cTransaction`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I2cByteTransfer {
    pub byte: u8,
    pub ack: bool,
}

/// Reconstructed I2C bus transaction delimited by START / Repeated START / STOP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct I2cTransaction {
    pub start_ps: u64,
    pub end_ps: u64,
    /// 7-bit I2C target address.
    pub address: u8,
    /// `true` for Read (`R/W = 1`), `false` for Write (`R/W = 0`).
    pub is_read: bool,
    /// `true` if the target pulled `SDA` low (`0`) on the 9th clock pulse.
    pub address_ack: bool,
    pub data_bytes: Vec<I2cByteTransfer>,
}

/// Decode I2C bus transactions from a [`CompactWaveStream`].
pub fn decode_i2c(stream: &CompactWaveStream, sigs: &I2cSignals) -> Vec<I2cTransaction> {
    #[derive(Clone, Copy)]
    enum I2cEv {
        Start(u64),
        Stop(u64),
        SclRise(u64),
    }

    let mut events: Vec<I2cEv> = Vec::new();
    let sda_trace = stream.transitions(sigs.sda);
    for i in 1..sda_trace.len() {
        let t = sda_trace[i].time_ps;
        // Check if SCL is stably high just before and at this SDA transition.
        let scl_before = stream.value_at(sigs.scl, t.saturating_sub(1)).unwrap_or(0) & 1;
        let scl_at = stream.value_at(sigs.scl, t).unwrap_or(0) & 1;
        if scl_before == 1 && scl_at == 1 {
            let prev_sda = sda_trace[i - 1].value & 1;
            let cur_sda = sda_trace[i].value & 1;
            if prev_sda == 1 && cur_sda == 0 {
                events.push(I2cEv::Start(t));
            } else if prev_sda == 0 && cur_sda == 1 {
                events.push(I2cEv::Stop(t));
            }
        }
    }

    let scl_trace = stream.transitions(sigs.scl);
    for i in 1..scl_trace.len() {
        let prev_scl = scl_trace[i - 1].value & 1;
        let cur_scl = scl_trace[i].value & 1;
        if prev_scl == 0 && cur_scl == 1 {
            events.push(I2cEv::SclRise(scl_trace[i].time_ps));
        }
    }

    events.sort_by_key(|ev| match *ev {
        I2cEv::Start(t) | I2cEv::Stop(t) | I2cEv::SclRise(t) => t,
    });

    let mut out = Vec::new();
    let mut active_start: Option<u64> = None;
    let mut bits: Vec<u8> = Vec::new();
    let mut last_t = 0u64;

    let finalize_tx = |start_ps: u64, end_ps: u64, bits: &[u8], out: &mut Vec<I2cTransaction>| {
        if bits.len() < 9 {
            return;
        }
        let mut addr_byte = 0u8;
        for b in 0..8 {
            addr_byte = (addr_byte << 1) | (bits[b] & 1);
        }
        let address = addr_byte >> 1;
        let is_read = (addr_byte & 1) != 0;
        let address_ack = bits[8] == 0; // SDA == 0 is ACK

        let mut data_bytes = Vec::new();
        let mut offset = 9;
        while offset + 9 <= bits.len() {
            let mut byte = 0u8;
            for b in 0..8 {
                byte = (byte << 1) | (bits[offset + b] & 1);
            }
            let ack = bits[offset + 8] == 0;
            data_bytes.push(I2cByteTransfer { byte, ack });
            offset += 9;
        }
        out.push(I2cTransaction {
            start_ps,
            end_ps,
            address,
            is_read,
            address_ack,
            data_bytes,
        });
    };

    for ev in events {
        match ev {
            I2cEv::Start(t) => {
                if let Some(st) = active_start.take() {
                    finalize_tx(st, t, &bits, &mut out);
                }
                active_start = Some(t);
                bits.clear();
                last_t = t;
            }
            I2cEv::Stop(t) => {
                if let Some(st) = active_start.take() {
                    finalize_tx(st, t, &bits, &mut out);
                }
                bits.clear();
            }
            I2cEv::SclRise(t) => {
                if active_start.is_some() {
                    let sda_bit = (stream.value_at(sigs.sda, t).unwrap_or(1) & 1) as u8;
                    bits.push(sda_bit);
                    last_t = t;
                }
            }
        }
    }

    if let Some(st) = active_start {
        finalize_tx(st, last_t, &bits, &mut out);
    }

    out
}

// --- 6. Wishbone Bus Protocol Decoder ---

/// Signal binding for a Wishbone B3/B4 bus interface.
#[derive(Clone, Debug)]
pub struct WishboneSignals {
    pub clk: WaveSignalId,
    pub cyc: WaveSignalId,
    pub stb: WaveSignalId,
    pub we: WaveSignalId,
    pub adr: WaveSignalId,
    pub dat_w: WaveSignalId,
    pub dat_r: WaveSignalId,
    pub ack: WaveSignalId,
    pub err: Option<WaveSignalId>,
    pub sel: Option<WaveSignalId>,
}

/// Reconstructed Wishbone bus transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WishboneTransaction {
    pub start_ps: u64,
    pub end_ps: u64,
    pub is_write: bool,
    pub addr: u64,
    pub data: u64,
    pub sel: u64,
    pub error: bool,
}

/// Decode Wishbone read and write transactions from a [`CompactWaveStream`].
pub fn decode_wishbone(
    stream: &CompactWaveStream,
    sigs: &WishboneSignals,
) -> Vec<WishboneTransaction> {
    let posedges = stream.clock_posedges(sigs.clk);
    let mut out = Vec::new();
    let mut cycle_start: Option<u64> = None;

    for t_edge in posedges {
        let cyc = (stream.value_at(sigs.cyc, t_edge).unwrap_or(0) & 1) != 0;
        let stb = (stream.value_at(sigs.stb, t_edge).unwrap_or(0) & 1) != 0;
        if !cyc || !stb {
            cycle_start = None;
            continue;
        }
        if cycle_start.is_none() {
            cycle_start = Some(t_edge);
        }
        let ack = (stream.value_at(sigs.ack, t_edge).unwrap_or(0) & 1) != 0;
        let err = sigs
            .err
            .map(|e| (stream.value_at(e, t_edge).unwrap_or(0) & 1) != 0)
            .unwrap_or(false);
        if ack || err {
            let is_write = (stream.value_at(sigs.we, t_edge).unwrap_or(0) & 1) != 0;
            let addr = stream.value_at(sigs.adr, t_edge).unwrap_or(0);
            let data = if is_write {
                stream.value_at(sigs.dat_w, t_edge).unwrap_or(0)
            } else {
                stream.value_at(sigs.dat_r, t_edge).unwrap_or(0)
            };
            let sel = sigs
                .sel
                .and_then(|s| stream.value_at(s, t_edge))
                .unwrap_or(0xF);
            out.push(WishboneTransaction {
                start_ps: cycle_start.take().unwrap_or(t_edge),
                end_ps: t_edge,
                is_write,
                addr,
                data,
                sel,
                error: err,
            });
        }
    }

    out
}
