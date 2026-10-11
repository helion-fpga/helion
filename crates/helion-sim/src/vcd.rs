//! VCD (IEEE 1364 value change dump) writer for helion-sim.
//!
//! [`dump_vcd`] runs the event kernel for N clock cycles and records every
//! net with a resolvable driver (FF Q, LUT O) plus a synthetic `clk`,
//! emitting standard VCD text readable by GTKWave, Surfer, or
//! helion-gui's own parser (for VCD-driven power estimation).
//!
//! The kernel is behavioral (FF/LUT evaluation, no routing delays), so all
//! signal changes land on clock edges — there are no glitch transitions in
//! the dump. This is documented, not hidden: power analysis built on this
//! VCD measures architectural switching, not post-route glitch power.

use crate::Sim;
use helion_ir::Design;

/// Upper bound on dumped signals. Ibex-scale designs would otherwise produce
/// gigabyte VCDs from this behavioral kernel; extra nets are silently
/// dropped (nets are sorted, so the drop set is deterministic).
pub const MAX_VCD_SIGNALS: usize = 4096;

/// Base-94 VCD identifier: `!`..`~` printable range, multi-char for >94 ids.
fn vcd_id(mut n: usize) -> String {
    let mut chars = Vec::new();
    loop {
        chars.push((33 + (n % 94)) as u8);
        n /= 94;
        if n == 0 {
            break;
        }
    }
    chars.iter().rev().map(|&b| b as char).collect()
}

/// `$timescale` token for a period in picoseconds, e.g. `10ps`, `1ns`.
fn timescale_token(period_ps: u64) -> String {
    const UNITS: [(u64, &str); 5] = [
        (1_000_000_000_000, "s"),
        (1_000_000_000, "ms"),
        (1_000_000, "us"),
        (1_000, "ns"),
        (1, "ps"),
    ];
    for (factor, unit) in UNITS {
        if period_ps >= factor && period_ps % factor == 0 {
            return format!("{}{}", period_ps / factor, unit);
        }
    }
    format!("{period_ps}ps")
}

/// Run the event kernel and dump a VCD of every driven net.
///
/// * `cycles` — number of clock posedges to simulate.
/// * `timescale_ps` — full clock period in picoseconds; also the VCD
///   `$timescale` when it divides evenly, else the raw ps value.
///
/// The dump contains one `$var wire 1` per driven net (named by net), plus a
/// synthetic `clk` that rises on each posedge and falls mid-cycle. Time 0
/// carries `$dumpvars` initial values; only changed signals are emitted
/// afterwards.
pub fn dump_vcd(design: &Design, cycles: u32, timescale_ps: u64) -> String {
    let period = timescale_ps.max(1);
    let half = (period / 2).max(1);
    let mut sim = Sim::new(design);

    // Signal list: synthetic `clk`, board `led`, then driven nets sorted.
    // Nets literally named "clk"/"led" are excluded to avoid colliding with
    // the synthetic signals (their values would otherwise misalign).
    let mut names: Vec<String> = vec!["clk".to_string(), "led".to_string()];
    let sample = sim.sample_nets(design);
    for (name, _) in &sample {
        if name != "clk" && name != "led" {
            names.push(name.clone());
        }
    }
    names.truncate(MAX_VCD_SIGNALS);
    let ids: Vec<String> = (0..names.len()).map(vcd_id).collect();

    let mut out = String::new();
    out.push_str("$date\n    helion-sim\n$end\n");
    out.push_str("$version\n    helion-sim VCD dumper\n$end\n");
    out.push_str(&format!("$timescale {} $end\n", timescale_token(period)));
    out.push_str(&format!("$scope module {} $end\n", design.name));
    for (name, id) in names.iter().zip(ids.iter()) {
        out.push_str(&format!("$var wire 1 {id} {name} $end\n"));
    }
    out.push_str("$upscope $end\n$enddefinitions $end\n");

    // Current value per signal, parallel to `names`.
    let initial: std::collections::HashMap<&str, bool> =
        sample.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    let mut cur: Vec<bool> = names
        .iter()
        .map(|n| match n.as_str() {
            "clk" => false,
            "led" => sim.led,
            _ => initial.get(n.as_str()).copied().unwrap_or(false),
        })
        .collect();
    debug_assert_eq!(cur.len(), names.len());

    out.push_str("#0\n$dumpvars\n");
    for (v, id) in cur.iter().zip(ids.iter()) {
        out.push_str(&format!("{}{id}\n", u8::from(*v)));
    }
    out.push_str("$end\n");

    // Fresh value of a named signal after a posedge.
    let fresh_value = |sim: &Sim, fresh: &std::collections::HashMap<&str, bool>, name: &str| -> Option<bool> {
        match name {
            "clk" => None, // driven explicitly per edge below
            "led" => Some(sim.led),
            _ => fresh.get(name).copied(),
        }
    };

    for i in 0..cycles {
        let t_edge = (i as u64) * period;
        // Posedge: clk rises, kernel evaluates, nets settle.
        if !cur[0] {
            cur[0] = true;
            out.push_str(&format!("#{t_edge}\n1{}\n", ids[0]));
        } else {
            out.push_str(&format!("#{t_edge}\n"));
        }
        sim.step_posedge(period);
        let fresh_vec = sim.sample_nets(design);
        let fresh: std::collections::HashMap<&str, bool> =
            fresh_vec.iter().map(|(n, v)| (n.as_str(), *v)).collect();
        for (idx, name) in names.iter().enumerate().skip(1) {
            if let Some(v) = fresh_value(&sim, &fresh, name) {
                if cur[idx] != v {
                    cur[idx] = v;
                    out.push_str(&format!("#{t_edge}\n{}{}\n", u8::from(v), ids[idx]));
                }
            }
        }
        // Negedge: clk falls mid-cycle; kernel state does not change.
        let t_neg = t_edge + half;
        if cur[0] {
            cur[0] = false;
            out.push_str(&format!("#{t_neg}\n0{}\n", ids[0]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, Design, PortDir};

    fn counter_design() -> Design {
        let src = r#"
module counter (
    input  logic clk,
    output logic led
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign led = cnt[3];
endmodule
"#;
        helion_sv::synth_sv(src, "counter.sv").expect("synth counter")
    }

    #[test]
    fn dump_vcd_header_and_timescale() {
        let d = counter_design();
        let vcd = dump_vcd(&d, 4, 10);
        assert!(vcd.contains("$timescale 10ps $end"), "timescale");
        assert!(vcd.contains("$scope module counter $end"), "scope");
        assert!(vcd.contains("$enddefinitions $end"), "enddefinitions");
        assert!(vcd.contains("$dumpvars"), "dumpvars");
        assert!(vcd.contains("#0"), "t=0");
        // 4 cycles at 10ps: posedges at 0/10/20/30, negedges at 5/15/25/35.
        assert!(vcd.contains("#30"), "last posedge");
        assert!(vcd.contains("#35"), "last negedge");
        assert!(!vcd.contains("#40"), "no time past the run");
    }

    #[test]
    fn dump_vcd_clk_toggles_every_cycle() {
        let d = counter_design();
        let vcd = dump_vcd(&d, 4, 10);
        // Find the clk id from its $var line.
        let clk_line = vcd
            .lines()
            .find(|l| l.starts_with("$var wire 1 ") && l.ends_with(" clk $end"))
            .expect("clk var");
        let id = clk_line.split_whitespace().nth(3).expect("clk id");
        let rises = vcd.lines().filter(|l| *l == format!("1{id}")).count();
        let falls = vcd.lines().filter(|l| *l == format!("0{id}")).count();
        assert_eq!(rises, 4, "clk rises once per cycle");
        assert_eq!(falls, 4, "clk falls once per cycle");
    }

    #[test]
    fn dump_vcd_counter_bit0_toggles_every_cycle() {
        let d = counter_design();
        let vcd = dump_vcd(&d, 8, 10);
        // cnt[0]'s net toggles every posedge: 8 changes over 8 cycles.
        let changes = vcd
            .lines()
            .filter(|l| {
                let b = l.as_bytes();
                b.len() > 1 && (b[0] == b'0' || b[0] == b'1') && !l.starts_with("$")
            })
            .count();
        assert!(changes >= 8, "at least the cnt[0] toggles: {changes}");
        assert!(changes < 8 * 50, "no change explosion: {changes}");
    }

    #[test]
    fn dump_vcd_const_design_has_quiet_nets() {
        let mut z = Design::new("z");
        z.add_port("clk", PortDir::In);
        z.add_port("led", PortDir::Out);
        z.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        z.add_cell("u_ff", CellKind::Hff);
        z.add_cell("u_iob", CellKind::IobOut);
        z.connect("d", "u_lut", "O");
        z.connect("d", "u_ff", "D");
        z.connect("q", "u_ff", "Q");
        z.connect("q", "u_lut", "I0");
        z.connect("q", "u_iob", "I");
        let vcd = dump_vcd(&z, 4, 10);
        // Only clk changes: 4 rises + 4 falls = 8 change lines.
        let changes = vcd
            .lines()
            .filter(|l| {
                let b = l.as_bytes();
                b.len() > 1 && (b[0] == b'0' || b[0] == b'1') && !l.starts_with("$")
            })
            .count();
        assert_eq!(changes, 8, "const design: only clk moves: {changes}");
    }

    #[test]
    fn vcd_id_encoding_is_unique() {
        let ids: Vec<String> = (0..300).map(vcd_id).collect();
        let mut seen = std::collections::HashSet::new();
        for id in &ids {
            assert!(seen.insert(id.clone()), "duplicate id {id}");
            assert!(
                id.bytes().all(|b| (33..=126).contains(&b)),
                "printable id {id}"
            );
        }
    }
}
