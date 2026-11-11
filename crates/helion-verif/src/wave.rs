//! Waveform display math and protocol decoders (UI-free).
//!
//! Everything here works on the uniform sample grid the viewers use
//! (zero-order hold, one `u64` per sample, `x`/`z` already mapped to 0):
//!
//! - **Display modes**: fixed-point `Qm.n` / `UQm.n` values
//!   ([`FixedPoint`]) and analog Y mapping ([`analog_y`]). Ported from
//!   `helion-gui/src/ide.rs` (`WaveformDisplayMode::sample_to_f64`,
//!   `format_sample`, `parse`, `compute_analog_samples_y`).
//! - **Virtual buses**: [`pack_bits`] packs 1-bit traces LSB-first, the
//!   `rebuild_virtual_buses` convention.
//! - **Decoders**: [`decode_spi`], [`decode_uart`], [`decode_axi_lite`], and
//!   [`decode_fsm`] turn the signals a user assigns to each role into
//!   labeled spans. The FSM decoder's unnamed-state wording (`INIT`, `INC`,
//!   `WRAP`, `MAX`, `STATE`) is the old `WaveTrace::decode_transactions`
//!   Counter/FSM rule; SPI/UART/AXI-Lite decode real multi-signal
//!   handshakes and frames instead of the old single-trace run labels.
//! - **FSM names**: [`fsm_name_groups`] scans HDL text for `localparam` /
//!   `parameter` groups and `enum` bodies so a state trace reads `IDLE`
//!   rather than `0x0`.

/// Mask for a `width`-bit value (1..=64).
pub fn mask(width: u8) -> u64 {
    let w = u32::from(width.clamp(1, 64));
    if w >= 64 { u64::MAX } else { (1u64 << w) - 1 }
}

/// Two's-complement sign extension of a `width`-bit sample.
pub fn sign_extend(raw: u64, width: u8) -> i64 {
    let w = width.clamp(1, 64);
    if w >= 64 {
        return raw as i64;
    }
    let m = mask(w);
    let v = raw & m;
    if v & (1u64 << (w - 1)) != 0 {
        (v | !m) as i64
    } else {
        v as i64
    }
}

/// Fixed-point interpretation of a bus: `frac_bits` fractional bits,
/// two's-complement when `signed` (`Q`), unsigned otherwise (`UQ`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedPoint {
    pub frac_bits: u8,
    pub signed: bool,
}

impl FixedPoint {
    /// Parse `Q4.4`, `Q.2`, `UQ8.8`, `q12`, `uq.3`: the number after the
    /// last `.` (or the only number) is the fractional bit count.
    pub fn parse(spec: &str) -> Option<Self> {
        let s = spec.trim().to_ascii_lowercase();
        let (signed, rest) = if let Some(r) = s.strip_prefix("uq") {
            (false, r)
        } else if let Some(r) = s.strip_prefix('q') {
            (true, r)
        } else {
            return None;
        };
        let frac = rest.rsplit('.').next().unwrap_or(rest);
        let frac_bits: u8 = frac.parse().ok()?;
        (frac_bits <= 63).then_some(Self { frac_bits, signed })
    }

    /// `Q.4` / `UQ.4` label (integer bits follow from the trace width).
    pub fn label(self) -> String {
        if self.signed {
            format!("Q.{}", self.frac_bits)
        } else {
            format!("UQ.{}", self.frac_bits)
        }
    }

    /// Full `Qm.n` label for a `width`-bit trace.
    pub fn label_for(self, width: u8) -> String {
        let int = i32::from(width) - i32::from(self.frac_bits) - i32::from(self.signed);
        let p = if self.signed { "Q" } else { "UQ" };
        format!("{p}{}.{}", int.max(0), self.frac_bits)
    }

    pub fn value(self, raw: u64, width: u8) -> f64 {
        let v = raw & mask(width);
        let base = if self.signed {
            sign_extend(v, width) as f64
        } else {
            v as f64
        };
        base / 2f64.powi(i32::from(self.frac_bits))
    }

    /// Readout text: up to 6 decimals, trailing zeros trimmed (`1.5`,
    /// `-0.25`, `3.0`).
    pub fn format(self, raw: u64, width: u8) -> String {
        let real = self.value(raw, width);
        if self.frac_bits == 0 {
            return format!("{real:.0}");
        }
        let prec = usize::from(self.frac_bits).clamp(1, 6);
        let s = format!("{real:.prec$}");
        let t = s.trim_end_matches('0');
        if t.ends_with('.') {
            format!("{t}0")
        } else {
            t.to_string()
        }
    }
}

/// Map sample values onto `[top, bottom]` (max value at `top`). A flat
/// trace sits on `bottom` when it is <= 0, mid-lane otherwise
/// (`compute_analog_samples_y`).
pub fn analog_y(values: &[f64], top: f32, bottom: f32) -> Vec<f32> {
    if values.is_empty() {
        return Vec::new();
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = hi - lo;
    let h = bottom - top;
    if span.abs() < 1e-12 {
        let y = if lo <= 0.0 { bottom } else { top + 0.5 * h };
        return vec![y; values.len()];
    }
    values
        .iter()
        .map(|&v| top + (1.0 - ((v - lo) / span).clamp(0.0, 1.0) as f32) * h)
        .collect()
}

/// Pack 1-bit member traces LSB-first (member 0 is bit 0). Members may
/// differ in length; the result spans the longest and holds short ones.
pub fn pack_bits(members: &[&[u64]]) -> Vec<u64> {
    let n = members.iter().map(|m| m.len()).max().unwrap_or(0);
    (0..n)
        .map(|i| {
            members.iter().enumerate().fold(0u64, |acc, (b, m)| {
                let v = m.get(i).or(m.last()).copied().unwrap_or(0) & 1;
                if b < 64 { acc | (v << b) } else { acc }
            })
        })
        .collect()
}

/// Semantic class of a decoded span (the viewer picks a theme color).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Read,
    Write,
    Data,
    State,
    Error,
}

/// One decoded transaction / frame / state on the grid: samples
/// `start..end` (end exclusive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedSpan {
    pub start: usize,
    pub end: usize,
    pub label: String,
    pub kind: SpanKind,
}

/// One decoder output row (`MOSI`, `MISO`, `Write`, `Read`, `RX`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedLane {
    pub name: String,
    pub spans: Vec<DecodedSpan>,
}

fn at(s: &[u64], i: usize) -> u64 {
    s.get(i).or(s.last()).copied().unwrap_or(0)
}

fn bit(s: &[u64], i: usize) -> bool {
    at(s, i) & 1 == 1
}

fn grid_len(sigs: &[Option<&[u64]>]) -> usize {
    sigs.iter().flatten().map(|s| s.len()).max().unwrap_or(0)
}

/// Sample indices where `s` rises (`rising`) or falls.
fn edges(s: &[u64], rising: bool) -> Vec<usize> {
    (1..s.len())
        .filter(|&i| {
            let (a, b) = (bit(s, i - 1), bit(s, i));
            if rising { !a && b } else { a && !b }
        })
        .collect()
}

/// Hex text sized to `bits` (`0xA5`, `0x0004`).
fn hex(v: u64, bits: u32) -> String {
    let nib = bits.div_ceil(4).max(1) as usize;
    format!("0x{v:0nib$X}")
}

/// SPI options: clock mode 0..=3 (CPOL/CPHA) and frame size in bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpiConfig {
    pub mode: u8,
    pub bits: u8,
    pub msb_first: bool,
}

impl Default for SpiConfig {
    fn default() -> Self {
        Self {
            mode: 0,
            bits: 8,
            msb_first: true,
        }
    }
}

/// Decode SPI frames. Bits are sampled on the leading edge for modes 0/3
/// (rising SCLK) and the falling edge for modes 1/2. With `cs_n`, frames
/// only count while it is low and a deassert drops a partial frame.
/// Returns `MOSI` and (when `miso` is mapped) `MISO` lanes.
pub fn decode_spi(
    sclk: &[u64],
    mosi: Option<&[u64]>,
    miso: Option<&[u64]>,
    cs_n: Option<&[u64]>,
    cfg: SpiConfig,
) -> Vec<DecodedLane> {
    let bits = u32::from(cfg.bits.clamp(1, 64));
    let rising = matches!(cfg.mode, 0 | 3);
    let selected = |i: usize| cs_n.is_none_or(|c| !bit(c, i));
    let mut lanes: Vec<(String, Option<&[u64]>)> = Vec::new();
    if mosi.is_some() {
        lanes.push(("MOSI".into(), mosi));
    }
    if miso.is_some() {
        lanes.push(("MISO".into(), miso));
    }
    let mut out: Vec<DecodedLane> = lanes
        .iter()
        .map(|(n, _)| DecodedLane {
            name: n.clone(),
            spans: Vec::new(),
        })
        .collect();
    let mut acc = vec![0u64; lanes.len()];
    let mut count = 0u32;
    let mut first = 0usize;
    let mut last_edge = 0usize;
    let cs_rise: Vec<usize> = cs_n.map(|c| edges(c, true)).unwrap_or_default();
    let mut cs_iter = cs_rise.iter().peekable();
    for e in edges(sclk, rising) {
        // A CS deassert between edges drops the partial frame.
        while let Some(&&r) = cs_iter.peek() {
            if r <= e {
                if r > last_edge {
                    count = 0;
                }
                cs_iter.next();
            } else {
                break;
            }
        }
        if !selected(e) {
            count = 0;
            continue;
        }
        if count == 0 {
            first = e;
            acc.iter_mut().for_each(|a| *a = 0);
        }
        for (k, (_, sig)) in lanes.iter().enumerate() {
            let b = u64::from(sig.is_some_and(|s| bit(s, e)));
            if cfg.msb_first {
                acc[k] = (acc[k] << 1) | b;
            } else if count < 64 {
                acc[k] |= b << count;
            }
        }
        count += 1;
        last_edge = e;
        if count == bits {
            // The frame ends one bit-period after its last sampling edge.
            let period = if bits > 1 {
                (e - first) / (bits as usize - 1)
            } else {
                1
            };
            let end = (e + period.max(1)).min(grid_len(&[Some(sclk)]).max(e + 1));
            for (k, lane) in out.iter_mut().enumerate() {
                lane.spans.push(DecodedSpan {
                    start: first,
                    end,
                    label: hex(acc[k] & mask(cfg.bits), bits),
                    kind: SpanKind::Data,
                });
            }
            count = 0;
        }
    }
    out
}

/// Shortest constant run strictly inside the trace: the bit period of an
/// asynchronous serial line (auto-baud). `None` without two edges.
pub fn estimate_bit_period(line: &[u64]) -> Option<usize> {
    let mut changes: Vec<usize> = (1..line.len())
        .filter(|&i| bit(line, i) != bit(line, i - 1))
        .collect();
    if changes.len() < 2 {
        return None;
    }
    changes.dedup();
    changes.windows(2).map(|w| w[1] - w[0]).min()
}

/// Decode an 8N1 UART line (idle high, LSB first). `bit_period` is in
/// samples; `None` estimates it from the line ([`estimate_bit_period`]).
/// A low stop bit is a framing error.
pub fn decode_uart(line: &[u64], bit_period: Option<usize>) -> DecodedLane {
    let mut lane = DecodedLane {
        name: "UART".into(),
        spans: Vec::new(),
    };
    let Some(p) = bit_period.or_else(|| estimate_bit_period(line)) else {
        return lane;
    };
    if p == 0 {
        return lane;
    }
    let n = line.len();
    let pf = p as f64;
    let mut i = 1;
    while i < n {
        if bit(line, i - 1) && !bit(line, i) {
            let s = i;
            let sample = |k: f64| (s as f64 + pf * k).floor() as usize;
            if sample(9.5) >= n {
                break;
            }
            if bit(line, sample(0.5)) {
                // Glitch, not a start bit.
                i += 1;
                continue;
            }
            let mut v = 0u64;
            for b in 0..8 {
                v |= u64::from(bit(line, sample(1.5 + b as f64))) << b;
            }
            let stop_ok = bit(line, sample(9.5));
            let end = sample(10.0).min(n);
            let ch = char::from_u32(v as u32)
                .filter(|c| c.is_ascii_graphic() || *c == ' ')
                .map(|c| format!(" '{c}'"))
                .unwrap_or_default();
            lane.spans.push(DecodedSpan {
                start: s,
                end,
                label: if stop_ok {
                    format!("0x{v:02X}{ch}")
                } else {
                    format!("0x{v:02X} FRAME ERR")
                },
                kind: if stop_ok {
                    SpanKind::Data
                } else {
                    SpanKind::Error
                },
            });
            i = sample(9.5).max(i + 1);
        } else {
            i += 1;
        }
    }
    lane
}

/// AXI4-Lite channel signals. Each channel decodes when its valid, ready,
/// and payload are all mapped; responses are optional.
#[derive(Clone, Copy, Debug, Default)]
pub struct AxiLiteSignals<'a> {
    pub aclk: Option<&'a [u64]>,
    pub awvalid: Option<&'a [u64]>,
    pub awready: Option<&'a [u64]>,
    pub awaddr: Option<&'a [u64]>,
    pub wvalid: Option<&'a [u64]>,
    pub wready: Option<&'a [u64]>,
    pub wdata: Option<&'a [u64]>,
    pub bvalid: Option<&'a [u64]>,
    pub bready: Option<&'a [u64]>,
    pub bresp: Option<&'a [u64]>,
    pub arvalid: Option<&'a [u64]>,
    pub arready: Option<&'a [u64]>,
    pub araddr: Option<&'a [u64]>,
    pub rvalid: Option<&'a [u64]>,
    pub rready: Option<&'a [u64]>,
    pub rdata: Option<&'a [u64]>,
    pub rresp: Option<&'a [u64]>,
}

/// Handshake sample indices (`valid && ready`): on rising `aclk` edges
/// when the clock is mapped, else the first sample of each high run.
fn handshakes(valid: &[u64], ready: &[u64], aclk: Option<&[u64]>, n: usize) -> Vec<usize> {
    let hs = |i: usize| bit(valid, i) && bit(ready, i);
    match aclk {
        Some(c) => edges(c, true).into_iter().filter(|&i| hs(i)).collect(),
        None => (0..n)
            .filter(|&i| hs(i) && (i == 0 || !hs(i - 1)))
            .collect(),
    }
}

fn resp_text(r: u64) -> &'static str {
    match r & 3 {
        0 => "OKAY",
        1 => "EXOKAY",
        2 => "SLVERR",
        _ => "DECERR",
    }
}

/// Decode AXI4-Lite writes (AW + W [+ B]) and reads (AR [+ R]) into
/// `Write` / `Read` lanes. Each transaction spans from its address
/// handshake to its response handshake (or the data/address handshake when
/// the response channel is unmapped). A non-OKAY response is an error span.
pub fn decode_axi_lite(sig: &AxiLiteSignals<'_>) -> Vec<DecodedLane> {
    let n = grid_len(&[
        sig.aclk,
        sig.awvalid,
        sig.wvalid,
        sig.arvalid,
        sig.rvalid,
        sig.bvalid,
    ]);
    let mut out = Vec::new();
    if let (Some(awv), Some(awr), Some(awa), Some(wv), Some(wr), Some(wd)) = (
        sig.awvalid,
        sig.awready,
        sig.awaddr,
        sig.wvalid,
        sig.wready,
        sig.wdata,
    ) {
        let aw = handshakes(awv, awr, sig.aclk, n);
        let w = handshakes(wv, wr, sig.aclk, n);
        let b = match (sig.bvalid, sig.bready) {
            (Some(bv), Some(br)) => Some(handshakes(bv, br, sig.aclk, n)),
            _ => None,
        };
        let mut spans = Vec::new();
        let mut bi = 0usize;
        for (&a, &d) in aw.iter().zip(w.iter()) {
            let start = a.min(d);
            let mut end = a.max(d);
            let mut resp = None;
            if let Some(b) = &b {
                while bi < b.len() && b[bi] < end {
                    bi += 1;
                }
                // The first unconsumed B handshake at or after the data
                // phase answers this write (AXI responses are in order).
                if let Some(&r) = b.get(bi) {
                    end = r;
                    resp = Some(sig.bresp.map(|s| at(s, r)).unwrap_or(0));
                    bi += 1;
                }
            }
            let mut label = format!("WR {} ← {}", hex(at(awa, a), 16), hex(at(wd, d), 32));
            let mut kind = SpanKind::Write;
            if let Some(r) = resp {
                label.push(' ');
                label.push_str(resp_text(r));
                if r & 2 != 0 {
                    kind = SpanKind::Error;
                }
            }
            spans.push(DecodedSpan {
                start,
                end: (end + 1).min(n.max(end + 1)),
                label,
                kind,
            });
        }
        out.push(DecodedLane {
            name: "Write".into(),
            spans,
        });
    }
    if let (Some(arv), Some(arr), Some(ara)) = (sig.arvalid, sig.arready, sig.araddr) {
        let ar = handshakes(arv, arr, sig.aclk, n);
        let r = match (sig.rvalid, sig.rready) {
            (Some(rv), Some(rr)) => Some(handshakes(rv, rr, sig.aclk, n)),
            _ => None,
        };
        let mut spans = Vec::new();
        let mut ri = 0usize;
        for &a in &ar {
            let mut end = a;
            let mut label = format!("RD {}", hex(at(ara, a), 16));
            let mut kind = SpanKind::Read;
            if let Some(r) = &r {
                while ri < r.len() && r[ri] < a {
                    ri += 1;
                }
                if let Some(&h) = r.get(ri) {
                    end = h;
                    if let Some(d) = sig.rdata {
                        label.push_str(&format!(" → {}", hex(at(d, h), 32)));
                    }
                    if let Some(rr) = sig.rresp {
                        let v = at(rr, h);
                        label.push(' ');
                        label.push_str(resp_text(v));
                        if v & 2 != 0 {
                            kind = SpanKind::Error;
                        }
                    }
                    ri += 1;
                }
            }
            spans.push(DecodedSpan {
                start: a,
                end: end + 1,
                label,
                kind,
            });
        }
        out.push(DecodedLane {
            name: "Read".into(),
            spans,
        });
    }
    out
}

/// Decode a state register into one span per held value. `names` maps
/// encodings to state names; unnamed values use the old Counter/FSM
/// wording (`INIT`, `INC`, `WRAP`, `MAX`, `STATE`).
pub fn decode_fsm(state: &[u64], width: u8, names: &[(u64, String)]) -> DecodedLane {
    let m = mask(width);
    let mut spans = Vec::new();
    let mut prev: Option<u64> = None;
    let mut start = 0usize;
    while start < state.len() {
        let v = state[start] & m;
        let mut end = start + 1;
        while end < state.len() && state[end] & m == v {
            end += 1;
        }
        let label = match names.iter().find(|(e, _)| *e == v) {
            Some((_, n)) => n.clone(),
            None if v == 0 && prev.is_none() => "INIT 0x0".into(),
            None if v == 0 && prev.is_some_and(|p| p > 0) => "WRAP 0x0".into(),
            None if width > 1 && v == m => format!("MAX 0x{v:X}"),
            None if prev.is_some_and(|p| v == p.wrapping_add(1)) => format!("INC 0x{v:X}"),
            None => format!("STATE 0x{v:X}"),
        };
        spans.push(DecodedSpan {
            start,
            end,
            label,
            kind: SpanKind::State,
        });
        prev = Some(v);
        start = end;
    }
    DecodedLane {
        name: "State".into(),
        spans,
    }
}

/// Parse a Verilog/VHDL-ish integer literal: `5`, `4'd5`, `2'b10`,
/// `8'hA5`, `'h3`, `32'sd7` (underscores ignored).
pub fn parse_int_literal(text: &str) -> Option<u64> {
    let t: String = text.trim().chars().filter(|c| *c != '_').collect();
    if let Some(q) = t.find('\'') {
        let rest = t[q + 1..].trim_start_matches(['s', 'S']);
        let mut ch = rest.chars();
        let base = match ch.next()?.to_ascii_lowercase() {
            'd' => 10,
            'h' => 16,
            'b' => 2,
            'o' => 8,
            _ => return None,
        };
        return u64::from_str_radix(ch.as_str(), base).ok();
    }
    t.parse().ok()
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_' || x == '$')
}

/// State-name groups found in HDL text: each `localparam`/`parameter`
/// statement with constant integer values, and each `enum { ... }` body
/// (implicit encodings count up from the previous one). Comments are
/// stripped first.
pub fn fsm_name_groups(src: &str) -> Vec<Vec<(u64, String)>> {
    let mut clean = String::with_capacity(src.len());
    let mut in_block = false;
    for line in src.lines() {
        let mut l = line;
        let mut keep = String::new();
        loop {
            if in_block {
                match l.find("*/") {
                    Some(e) => {
                        l = &l[e + 2..];
                        in_block = false;
                    }
                    None => break,
                }
            } else {
                let lc = l.find("//");
                let bc = l.find("/*");
                match (lc, bc) {
                    (Some(a), Some(b)) if a < b => {
                        keep.push_str(&l[..a]);
                        break;
                    }
                    (Some(a), None) => {
                        keep.push_str(&l[..a]);
                        break;
                    }
                    (_, Some(b)) => {
                        keep.push_str(&l[..b]);
                        l = &l[b + 2..];
                        in_block = true;
                    }
                    (None, None) => {
                        keep.push_str(l);
                        break;
                    }
                }
            }
        }
        clean.push_str(&keep);
        clean.push('\n');
    }
    let mut groups = Vec::new();
    for stmt in clean.split(';') {
        let st = stmt.trim();
        if let Some(open) = st.find("enum") {
            if let (Some(a), Some(b)) = (st[open..].find('{'), st[open..].rfind('}')) {
                let body = &st[open + a + 1..open + b];
                let mut next = 0u64;
                let mut g = Vec::new();
                for item in body.split(',') {
                    let (name, val) = match item.split_once('=') {
                        Some((n, v)) => (n.trim(), parse_int_literal(v)),
                        None => (item.trim(), Some(next)),
                    };
                    if let (true, Some(v)) = (is_ident(name), val) {
                        g.push((v, name.to_string()));
                        next = v.wrapping_add(1);
                    }
                }
                if !g.is_empty() {
                    groups.push(g);
                }
                continue;
            }
        }
        let words: Vec<&str> = st.split_whitespace().collect();
        let Some(kw) = words
            .iter()
            .position(|w| *w == "localparam" || *w == "parameter")
        else {
            continue;
        };
        let rest = words[kw + 1..].join(" ");
        // Drop an optional `[3:0]`-style range / type before the first name.
        let body = match rest.rfind(']') {
            Some(i) if rest[..i].find('=').is_none() => rest[i + 1..].to_string(),
            _ => rest,
        };
        let mut g = Vec::new();
        for item in body.split(',') {
            let Some((n, v)) = item.split_once('=') else {
                continue;
            };
            let name = n.split_whitespace().last().unwrap_or("");
            if let (true, Some(v)) = (is_ident(name), parse_int_literal(v)) {
                g.push((v, name.to_string()));
            }
        }
        if !g.is_empty() {
            groups.push(g);
        }
    }
    groups
}

/// The name group that names the most of `observed` state values (ties
/// keep the earliest group). `None` when no group names any value.
pub fn best_name_group(
    groups: &[Vec<(u64, String)>],
    observed: &[u64],
) -> Option<Vec<(u64, String)>> {
    let mut best: Option<(usize, &Vec<(u64, String)>)> = None;
    for g in groups {
        let hits = observed
            .iter()
            .filter(|v| g.iter().any(|(e, _)| e == *v))
            .count();
        if hits > 0 && best.is_none_or(|(h, _)| hits > h) {
            best = Some((hits, g));
        }
    }
    best.map(|(_, g)| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_formats_like_the_old_viewer() {
        let q = FixedPoint::parse("Q.2").unwrap();
        assert_eq!(q.format(0b0110, 4), "1.5");
        assert_eq!(q.format(0b1110, 4), "-0.5");
        let uq = FixedPoint::parse("uq2.2").unwrap();
        assert_eq!(uq.format(0b1110, 4), "3.5");
        assert_eq!(FixedPoint::parse("Q4.4").unwrap().label_for(8), "Q3.4");
        assert_eq!(FixedPoint::parse("Q0").unwrap().format(5, 4), "5");
        assert!(FixedPoint::parse("hex").is_none());
    }

    #[test]
    fn analog_y_spans_the_lane() {
        let y = analog_y(&[0.0, 5.0, 10.0], 0.0, 100.0);
        assert_eq!(y, vec![100.0, 50.0, 0.0]);
        assert_eq!(analog_y(&[3.0, 3.0], 0.0, 10.0), vec![5.0, 5.0]);
        assert_eq!(analog_y(&[0.0], 0.0, 10.0), vec![10.0]);
    }

    #[test]
    fn pack_bits_is_lsb_first() {
        let a = [1u64, 0, 1];
        let b = [0u64, 1, 1];
        assert_eq!(pack_bits(&[&a, &b]), vec![1, 2, 3]);
    }

    /// SPI mode 0, 8 bits MSB first: drive `byte` on MOSI around rising
    /// edges of a 2-sample clock with CS low.
    fn spi_frame(byte: u8) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
        let (mut clk, mut mosi, mut cs) = (vec![0, 0], vec![0, 0], vec![1, 0]);
        for b in (0..8).rev() {
            let v = u64::from((byte >> b) & 1);
            clk.extend([0, 1]);
            mosi.extend([v, v]);
            cs.extend([0, 0]);
        }
        clk.extend([0, 0]);
        mosi.extend([0, 0]);
        cs.extend([0, 1]);
        (clk, mosi, cs)
    }

    #[test]
    fn spi_decodes_a_mode0_byte() {
        let (clk, mosi, cs) = spi_frame(0xA5);
        let lanes = decode_spi(&clk, Some(&mosi), None, Some(&cs), SpiConfig::default());
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].spans.len(), 1);
        assert_eq!(lanes[0].spans[0].label, "0xA5");
    }

    #[test]
    fn spi_cs_deassert_drops_a_partial_frame() {
        let (mut clk, mut mosi, mut cs) = spi_frame(0xFF);
        // Truncate after 4 bits then deassert CS.
        clk.truncate(10);
        mosi.truncate(10);
        cs.truncate(10);
        cs.extend([1, 1]);
        clk.extend([0, 0]);
        mosi.extend([0, 0]);
        let (c2, m2, s2) = spi_frame(0x3C);
        clk.extend(c2);
        mosi.extend(m2);
        cs.extend(s2);
        let lanes = decode_spi(&clk, Some(&mosi), None, Some(&cs), SpiConfig::default());
        let labels: Vec<_> = lanes[0].spans.iter().map(|s| s.label.clone()).collect();
        assert_eq!(labels, vec!["0x3C"]);
    }

    fn uart_line(bytes: &[u8], p: usize) -> Vec<u64> {
        let mut l = vec![1u64; p * 2];
        for &b in bytes {
            l.extend(std::iter::repeat_n(0, p));
            for i in 0..8 {
                l.extend(std::iter::repeat_n(u64::from((b >> i) & 1), p));
            }
            l.extend(std::iter::repeat_n(1, p * 2));
        }
        l
    }

    #[test]
    fn uart_decodes_with_auto_baud() {
        let line = uart_line(b"Hi", 4);
        assert_eq!(estimate_bit_period(&line), Some(4));
        let lane = decode_uart(&line, None);
        let labels: Vec<_> = lane.spans.iter().map(|s| s.label.clone()).collect();
        assert_eq!(labels, vec!["0x48 'H'", "0x69 'i'"]);
        assert_eq!(lane.spans[0].end - lane.spans[0].start, 40);
    }

    #[test]
    fn uart_flags_a_framing_error() {
        let mut line = uart_line(b"A", 2);
        // Force the stop bit low.
        let s = 4 + 2 * 9;
        line[s] = 0;
        line[s + 1] = 0;
        let lane = decode_uart(&line, Some(2));
        assert_eq!(lane.spans[0].kind, SpanKind::Error);
    }

    #[test]
    fn axi_lite_pairs_write_and_read_handshakes() {
        // 12 samples, no clock: AW+W at 2, B at 4; AR at 7, R at 9.
        let z = vec![0u64; 12];
        let pulse = |i: usize| {
            let mut v = z.clone();
            v[i] = 1;
            v
        };
        let awv = pulse(2);
        let w = pulse(2);
        let b = pulse(4);
        let ar = pulse(7);
        let r = pulse(9);
        let one = vec![1u64; 12];
        let addr = vec![0x10u64; 12];
        let data = vec![0xBEEFu64; 12];
        let sig = AxiLiteSignals {
            awvalid: Some(&awv),
            awready: Some(&one),
            awaddr: Some(&addr),
            wvalid: Some(&w),
            wready: Some(&one),
            wdata: Some(&data),
            bvalid: Some(&b),
            bready: Some(&one),
            bresp: Some(&z),
            arvalid: Some(&ar),
            arready: Some(&one),
            araddr: Some(&addr),
            rvalid: Some(&r),
            rready: Some(&one),
            rdata: Some(&data),
            ..Default::default()
        };
        let lanes = decode_axi_lite(&sig);
        assert_eq!(lanes.len(), 2);
        let wr = &lanes[0].spans[0];
        assert_eq!((wr.start, wr.end), (2, 5));
        assert_eq!(wr.label, "WR 0x0010 ← 0x0000BEEF OKAY");
        let rd = &lanes[1].spans[0];
        assert_eq!((rd.start, rd.end), (7, 10));
        assert_eq!(rd.label, "RD 0x0010 → 0x0000BEEF");
    }

    #[test]
    fn fsm_uses_names_then_old_counter_wording() {
        let st = [0u64, 0, 1, 1, 2, 0];
        let lane = decode_fsm(&st, 2, &[(0, "IDLE".into()), (1, "RUN".into())]);
        let labels: Vec<_> = lane.spans.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["IDLE", "RUN", "INC 0x2", "IDLE"]);
        let raw = decode_fsm(&[0, 1, 3, 0], 2, &[]);
        let labels: Vec<_> = raw.spans.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["INIT 0x0", "INC 0x1", "MAX 0x3", "WRAP 0x0"]);
    }

    #[test]
    fn fsm_names_come_from_localparams_and_enums() {
        let src = "module m;\n// localparam BOGUS = 9;\n\
            localparam [1:0] S_IDLE = 2'd0, S_RUN = 2'd1, S_DONE = 2'b10;\n\
            parameter WIDTH = 8;\n\
            typedef enum logic [1:0] {A, B = 2, C} st_t;\nendmodule\n";
        let groups = fsm_name_groups(src);
        assert_eq!(groups.len(), 3, "{groups:?}");
        assert_eq!(groups[0][2], (2, "S_DONE".to_string()));
        assert_eq!(
            groups[2],
            vec![(0, "A".into()), (2, "B".into()), (3, "C".into())]
        );
        let best = best_name_group(&groups, &[0, 1, 2]).unwrap();
        assert_eq!(best[1].1, "S_RUN");
        assert!(best_name_group(&groups, &[77]).is_none());
    }
}
