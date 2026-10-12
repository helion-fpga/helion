//! Exact binary codec for [`Constraints`] and its sub-structs.
//!
//! Used by design checkpoints (`.hck2`) so timing constraints round-trip
//! bit-for-bit: `decode(encode(c)) == c` for every field. Layout is a flat
//! sequence of little-endian scalars, length-prefixed strings, and
//! length-prefixed lists, in struct declaration order. There is no version
//! negotiation inside this blob; the checkpoint envelope carries the format
//! version. Decoding is total: truncated or malformed input returns `Err`,
//! never panics.

use super::{
    BusSkew, CaseAnalysis, Clock, ClockGroups, ClockLatency, ClockSense, ClockUncertainty,
    Constraints, DataCheck, DisableTiming, InputJitter, MaxDelay, MaxTimeBorrow, MinDelay,
    MulticyclePath, OperatingConditions, PathGroup, TimingDerate,
};
use std::collections::BTreeMap;

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }
    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        self.u32(b.len() as u32);
        self.buf.extend_from_slice(b);
    }
    fn opt_str(&mut self, s: &Option<String>) {
        match s {
            Some(v) => {
                self.u8(1);
                self.str(v);
            }
            None => self.u8(0),
        }
    }
    fn str_list(&mut self, v: &[String]) {
        self.u32(v.len() as u32);
        for s in v {
            self.str(s);
        }
    }
    fn str_i64_map(&mut self, m: &BTreeMap<String, i64>) {
        self.u32(m.len() as u32);
        for (k, v) in m {
            self.str(k);
            self.i64(*v);
        }
    }
    fn str_str_map(&mut self, m: &BTreeMap<String, String>) {
        self.u32(m.len() as u32);
        for (k, v) in m {
            self.str(k);
            self.str(v);
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or("constraints: length overflow")?;
        if end > self.buf.len() {
            return Err(format!(
                "constraints: truncated (need {n} bytes at offset {})",
                self.pos
            ));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(self.u64()? as i64)
    }
    fn bool(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(format!("constraints: bad bool {v}")),
        }
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        if n > 1 << 24 {
            return Err("constraints: string too long".into());
        }
        let b = self.take(n)?;
        std::str::from_utf8(b)
            .map(|s| s.to_string())
            .map_err(|e| format!("constraints: bad utf8: {e}"))
    }
    fn opt_str(&mut self) -> Result<Option<String>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.str()?)),
            v => Err(format!("constraints: bad option tag {v}")),
        }
    }
    fn str_list(&mut self) -> Result<Vec<String>, String> {
        let n = self.u32()? as usize;
        if n > 1 << 20 {
            return Err("constraints: list too long".into());
        }
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            v.push(self.str()?);
        }
        Ok(v)
    }
    fn str_i64_map(&mut self) -> Result<BTreeMap<String, i64>, String> {
        let n = self.u32()? as usize;
        if n > 1 << 20 {
            return Err("constraints: map too long".into());
        }
        let mut m = BTreeMap::new();
        for _ in 0..n {
            let k = self.str()?;
            let v = self.i64()?;
            m.insert(k, v);
        }
        Ok(m)
    }
    fn str_str_map(&mut self) -> Result<BTreeMap<String, String>, String> {
        let n = self.u32()? as usize;
        if n > 1 << 20 {
            return Err("constraints: map too long".into());
        }
        let mut m = BTreeMap::new();
        for _ in 0..n {
            let k = self.str()?;
            let v = self.str()?;
            m.insert(k, v);
        }
        Ok(m)
    }
    fn end(&self) -> Result<(), String> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(format!(
                "constraints: {} trailing bytes",
                self.buf.len() - self.pos
            ))
        }
    }
}

fn enc_clock(w: &mut Writer, c: &Clock) {
    w.str(&c.name);
    w.u64(c.period_ps);
    w.str(&c.source);
    w.bool(c.generated);
    w.opt_str(&c.master);
    w.u32(c.divide_by);
    w.u32(c.multiply_by);
    w.bool(c.invert);
    w.u32(c.edges.len() as u32);
    for e in &c.edges {
        w.u32(*e);
    }
}

fn dec_clock(r: &mut Reader) -> Result<Clock, String> {
    let name = r.str()?;
    let period_ps = r.u64()?;
    let source = r.str()?;
    let generated = r.bool()?;
    let master = r.opt_str()?;
    let divide_by = r.u32()?;
    let multiply_by = r.u32()?;
    let invert = r.bool()?;
    let n = r.u32()? as usize;
    if n > 1 << 16 {
        return Err("constraints: clock edges too many".into());
    }
    let mut edges = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        edges.push(r.u32()?);
    }
    Ok(Clock {
        name,
        period_ps,
        source,
        generated,
        master,
        divide_by,
        multiply_by,
        invert,
        edges,
    })
}

impl Constraints {
    /// Exact binary encoding; [`Constraints::decode`] inverts it.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(self.clocks.len() as u32);
        for c in &self.clocks {
            enc_clock(&mut w, c);
        }
        w.str_i64_map(&self.input_delay_ps);
        w.str_i64_map(&self.output_delay_ps);
        w.str_list(&self.false_paths);
        w.u32(self.multicycle_paths.len() as u32);
        for m in &self.multicycle_paths {
            w.str(&m.from);
            w.str(&m.to);
            w.u32(m.setup_mult);
            w.u32(m.hold_mult);
            w.bool(m.start);
            w.bool(m.end);
        }
        w.u32(self.max_delays.len() as u32);
        for m in &self.max_delays {
            w.str(&m.from);
            w.str(&m.to);
            w.i64(m.delay_ps);
            w.bool(m.datapath_only);
        }
        w.u32(self.min_delays.len() as u32);
        for m in &self.min_delays {
            w.str(&m.from);
            w.str(&m.to);
            w.i64(m.delay_ps);
            w.bool(m.datapath_only);
        }
        w.u32(self.clock_groups.len() as u32);
        for g in &self.clock_groups {
            w.bool(g.asynchronous);
            w.bool(g.exclusive);
            w.u32(g.groups.len() as u32);
            for grp in &g.groups {
                w.str_list(grp);
            }
        }
        w.u32(self.clock_uncertainties.len() as u32);
        for u in &self.clock_uncertainties {
            w.str(&u.from);
            w.str(&u.to);
            w.i64(u.setup_ps);
            w.i64(u.hold_ps);
        }
        w.u32(self.clock_latencies.len() as u32);
        for l in &self.clock_latencies {
            w.str(&l.clock);
            w.i64(l.late_ps);
            w.i64(l.early_ps);
            w.bool(l.source);
        }
        w.u32(self.disable_timings.len() as u32);
        for d in &self.disable_timings {
            w.str(&d.from);
            w.str(&d.to);
            w.str(&d.object);
        }
        w.u32(self.case_analyses.len() as u32);
        for c in &self.case_analyses {
            w.str(&c.value);
            w.str(&c.object);
        }
        w.str_list(&self.propagated_clocks);
        w.u32(self.clock_senses.len() as u32);
        for s in &self.clock_senses {
            w.str(&s.object);
            w.str(&s.sense);
        }
        w.u32(self.input_jitters.len() as u32);
        for j in &self.input_jitters {
            w.str(&j.clock);
            w.i64(j.jitter_ps);
        }
        w.i64(self.system_jitter_ps);
        w.u32(self.timing_derates.len() as u32);
        for t in &self.timing_derates {
            w.i64(t.early_milli);
            w.i64(t.late_milli);
            w.bool(t.cell);
            w.bool(t.net);
            w.bool(t.clock);
            w.bool(t.data);
        }
        w.i64(self.operating_conditions.voltage_mv);
        w.i64(self.operating_conditions.temperature_c);
        w.bool(self.operating_conditions.voltage_set);
        w.bool(self.operating_conditions.temperature_set);
        w.u32(self.bus_skews.len() as u32);
        for b in &self.bus_skews {
            w.str(&b.from);
            w.str(&b.to);
            w.i64(b.skew_ps);
            w.bool(b.setup);
            w.bool(b.hold);
        }
        w.u32(self.path_groups.len() as u32);
        for p in &self.path_groups {
            w.str(&p.name);
            w.str(&p.from);
            w.str(&p.to);
            w.i64(p.weight_milli);
            w.i64(p.critical_range_ps);
        }
        w.u32(self.max_time_borrows.len() as u32);
        for b in &self.max_time_borrows {
            w.str(&b.object);
            w.i64(b.borrow_ps);
        }
        w.u32(self.data_checks.len() as u32);
        for d in &self.data_checks {
            w.str(&d.from);
            w.str(&d.to);
            w.i64(d.setup_ps);
            w.i64(d.hold_ps);
            w.str(&d.clock);
        }
        w.str_str_map(&self.package_pins);
        w.str_str_map(&self.iostandards);
        w.str_str_map(&self.drives);
        w.str_str_map(&self.slews);
        w.str_str_map(&self.pulltypes);
        w.str_str_map(&self.diff_terms);
        w.str_str_map(&self.in_terms);
        w.buf
    }

    /// Inverse of [`Constraints::encode`]. Rejects truncated/malformed input.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(bytes);
        let n = r.u32()? as usize;
        if n > 1 << 16 {
            return Err("constraints: too many clocks".into());
        }
        let mut clocks = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            clocks.push(dec_clock(&mut r)?);
        }
        let input_delay_ps = r.str_i64_map()?;
        let output_delay_ps = r.str_i64_map()?;
        let false_paths = r.str_list()?;
        let nm = r.u32()? as usize;
        let mut multicycle_paths = Vec::with_capacity(nm.min(64));
        for _ in 0..nm {
            multicycle_paths.push(MulticyclePath {
                from: r.str()?,
                to: r.str()?,
                setup_mult: r.u32()?,
                hold_mult: r.u32()?,
                start: r.bool()?,
                end: r.bool()?,
            });
        }
        let nmx = r.u32()? as usize;
        let mut max_delays = Vec::with_capacity(nmx.min(64));
        for _ in 0..nmx {
            max_delays.push(MaxDelay {
                from: r.str()?,
                to: r.str()?,
                delay_ps: r.i64()?,
                datapath_only: r.bool()?,
            });
        }
        let nmn = r.u32()? as usize;
        let mut min_delays = Vec::with_capacity(nmn.min(64));
        for _ in 0..nmn {
            min_delays.push(MinDelay {
                from: r.str()?,
                to: r.str()?,
                delay_ps: r.i64()?,
                datapath_only: r.bool()?,
            });
        }
        let ng = r.u32()? as usize;
        let mut clock_groups = Vec::with_capacity(ng.min(16));
        for _ in 0..ng {
            let asynchronous = r.bool()?;
            let exclusive = r.bool()?;
            let gn = r.u32()? as usize;
            let mut groups = Vec::with_capacity(gn.min(16));
            for _ in 0..gn {
                groups.push(r.str_list()?);
            }
            clock_groups.push(ClockGroups {
                asynchronous,
                exclusive,
                groups,
            });
        }
        let nu = r.u32()? as usize;
        let mut clock_uncertainties = Vec::with_capacity(nu.min(64));
        for _ in 0..nu {
            clock_uncertainties.push(ClockUncertainty {
                from: r.str()?,
                to: r.str()?,
                setup_ps: r.i64()?,
                hold_ps: r.i64()?,
            });
        }
        let nl = r.u32()? as usize;
        let mut clock_latencies = Vec::with_capacity(nl.min(64));
        for _ in 0..nl {
            clock_latencies.push(ClockLatency {
                clock: r.str()?,
                late_ps: r.i64()?,
                early_ps: r.i64()?,
                source: r.bool()?,
            });
        }
        let nd = r.u32()? as usize;
        let mut disable_timings = Vec::with_capacity(nd.min(64));
        for _ in 0..nd {
            disable_timings.push(DisableTiming {
                from: r.str()?,
                to: r.str()?,
                object: r.str()?,
            });
        }
        let nc = r.u32()? as usize;
        let mut case_analyses = Vec::with_capacity(nc.min(64));
        for _ in 0..nc {
            case_analyses.push(CaseAnalysis {
                value: r.str()?,
                object: r.str()?,
            });
        }
        let propagated_clocks = r.str_list()?;
        let ns = r.u32()? as usize;
        let mut clock_senses = Vec::with_capacity(ns.min(64));
        for _ in 0..ns {
            clock_senses.push(ClockSense {
                object: r.str()?,
                sense: r.str()?,
            });
        }
        let nj = r.u32()? as usize;
        let mut input_jitters = Vec::with_capacity(nj.min(64));
        for _ in 0..nj {
            input_jitters.push(InputJitter {
                clock: r.str()?,
                jitter_ps: r.i64()?,
            });
        }
        let system_jitter_ps = r.i64()?;
        let nt = r.u32()? as usize;
        let mut timing_derates = Vec::with_capacity(nt.min(16));
        for _ in 0..nt {
            timing_derates.push(TimingDerate {
                early_milli: r.i64()?,
                late_milli: r.i64()?,
                cell: r.bool()?,
                net: r.bool()?,
                clock: r.bool()?,
                data: r.bool()?,
            });
        }
        let operating_conditions = OperatingConditions {
            voltage_mv: r.i64()?,
            temperature_c: r.i64()?,
            voltage_set: r.bool()?,
            temperature_set: r.bool()?,
        };
        let nb = r.u32()? as usize;
        let mut bus_skews = Vec::with_capacity(nb.min(64));
        for _ in 0..nb {
            bus_skews.push(BusSkew {
                from: r.str()?,
                to: r.str()?,
                skew_ps: r.i64()?,
                setup: r.bool()?,
                hold: r.bool()?,
            });
        }
        let np = r.u32()? as usize;
        let mut path_groups = Vec::with_capacity(np.min(64));
        for _ in 0..np {
            path_groups.push(PathGroup {
                name: r.str()?,
                from: r.str()?,
                to: r.str()?,
                weight_milli: r.i64()?,
                critical_range_ps: r.i64()?,
            });
        }
        let nmb = r.u32()? as usize;
        let mut max_time_borrows = Vec::with_capacity(nmb.min(64));
        for _ in 0..nmb {
            max_time_borrows.push(MaxTimeBorrow {
                object: r.str()?,
                borrow_ps: r.i64()?,
            });
        }
        let ndc = r.u32()? as usize;
        let mut data_checks = Vec::with_capacity(ndc.min(64));
        for _ in 0..ndc {
            data_checks.push(DataCheck {
                from: r.str()?,
                to: r.str()?,
                setup_ps: r.i64()?,
                hold_ps: r.i64()?,
                clock: r.str()?,
            });
        }
        let package_pins = r.str_str_map()?;
        let iostandards = r.str_str_map()?;
        let drives = r.str_str_map()?;
        let slews = r.str_str_map()?;
        let pulltypes = r.str_str_map()?;
        let diff_terms = r.str_str_map()?;
        let in_terms = r.str_str_map()?;
        r.end()?;
        Ok(Constraints {
            clocks,
            input_delay_ps,
            output_delay_ps,
            false_paths,
            multicycle_paths,
            max_delays,
            min_delays,
            clock_groups,
            clock_uncertainties,
            clock_latencies,
            disable_timings,
            case_analyses,
            propagated_clocks,
            clock_senses,
            input_jitters,
            system_jitter_ps,
            timing_derates,
            operating_conditions,
            bus_skews,
            path_groups,
            max_time_borrows,
            data_checks,
            package_pins,
            iostandards,
            drives,
            slews,
            pulltypes,
            diff_terms,
            in_terms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_constraints() -> Constraints {
        let mut c = Constraints::default();
        c.clocks.push(Clock {
            name: "clk".into(),
            period_ps: 10_000,
            source: "clk".into(),
            generated: false,
            master: None,
            divide_by: 1,
            multiply_by: 1,
            invert: false,
            edges: vec![],
        });
        c.clocks.push(Clock {
            name: "clk_div2".into(),
            period_ps: 20_000,
            source: "div_q".into(),
            generated: true,
            master: Some("clk".into()),
            divide_by: 2,
            multiply_by: 1,
            invert: true,
            edges: vec![1, 2, 3],
        });
        c.input_delay_ps.insert("din".into(), 500);
        c.output_delay_ps.insert("led".into(), -200);
        c.false_paths.push("-from [get_clocks a] -to [get_clocks b]".into());
        c.multicycle_paths.push(MulticyclePath {
            from: "a".into(),
            to: "b".into(),
            setup_mult: 2,
            hold_mult: 1,
            start: true,
            end: false,
        });
        c.max_delays.push(MaxDelay {
            from: "x".into(),
            to: "y".into(),
            delay_ps: 8000,
            datapath_only: true,
        });
        c.min_delays.push(MinDelay {
            from: "x".into(),
            to: "y".into(),
            delay_ps: 100,
            datapath_only: false,
        });
        c.clock_groups.push(ClockGroups {
            asynchronous: true,
            exclusive: false,
            groups: vec![vec!["a".into()], vec!["b".into(), "c".into()]],
        });
        c.clock_uncertainties.push(ClockUncertainty {
            from: "a".into(),
            to: "b".into(),
            setup_ps: 50,
            hold_ps: 25,
        });
        c.clock_latencies.push(ClockLatency {
            clock: "clk".into(),
            late_ps: 100,
            early_ps: 80,
            source: true,
        });
        c.disable_timings.push(DisableTiming {
            from: "u".into(),
            to: "v".into(),
            object: "cell0".into(),
        });
        c.case_analyses.push(CaseAnalysis {
            value: "0".into(),
            object: "sel".into(),
        });
        c.propagated_clocks.push("clk".into());
        c.clock_senses.push(ClockSense {
            object: "pin0".into(),
            sense: "positive".into(),
        });
        c.input_jitters.push(InputJitter {
            clock: "clk".into(),
            jitter_ps: 120,
        });
        c.system_jitter_ps = 60;
        c.timing_derates.push(TimingDerate {
            early_milli: 900,
            late_milli: 1100,
            cell: true,
            net: false,
            clock: true,
            data: false,
        });
        c.operating_conditions = OperatingConditions {
            voltage_mv: 950,
            temperature_c: 85,
            voltage_set: true,
            temperature_set: true,
        };
        c.bus_skews.push(BusSkew {
            from: "d[0]".into(),
            to: "d[7]".into(),
            skew_ps: 300,
            setup: true,
            hold: true,
        });
        c.path_groups.push(PathGroup {
            name: "g1".into(),
            from: "a".into(),
            to: "b".into(),
            weight_milli: 1000,
            critical_range_ps: 500,
        });
        c.max_time_borrows.push(MaxTimeBorrow {
            object: "latch0".into(),
            borrow_ps: 400,
        });
        c.data_checks.push(DataCheck {
            from: "d0".into(),
            to: "d1".into(),
            setup_ps: 200,
            hold_ps: 100,
            clock: "clk".into(),
        });
        c.package_pins.insert("led".into(), "IOB_X5Y0".into());
        c.iostandards.insert("led".into(), "LVCMOS33".into());
        c.drives.insert("led".into(), "8".into());
        c.slews.insert("led".into(), "SLOW".into());
        c.pulltypes.insert("btn".into(), "PULLUP".into());
        c.diff_terms.insert("p".into(), "TRUE".into());
        c.in_terms.insert("n".into(), "TRUE".into());
        c
    }

    #[test]
    fn constraints_round_trip_exact() {
        let c = full_constraints();
        let bytes = c.encode();
        let back = Constraints::decode(&bytes).expect("decode");
        assert_eq!(c, back);
    }

    #[test]
    fn constraints_round_trip_empty() {
        let c = Constraints::default();
        let back = Constraints::decode(&c.encode()).expect("decode");
        assert_eq!(c, back);
    }

    #[test]
    fn constraints_decode_rejects_truncation() {
        let c = full_constraints();
        let mut bytes = c.encode();
        bytes.truncate(bytes.len() / 2);
        assert!(Constraints::decode(&bytes).is_err());
        assert!(Constraints::decode(&[]).is_err());
        assert!(Constraints::decode(b"garbage garbage").is_err());
    }
}
