//! VHDL front end. [`v2`] parses and elaborates the synthesizable subset of
//! VHDL-2008 (types, generics, hierarchy, processes executed symbolically)
//! into flat SystemVerilog that `helion_sv` maps to the netlist IR.
//! Constructs outside the subset are errors with a line number; nothing is
//! dropped silently.

pub mod v2;

use helion_ir::{Design, MapResult, SoftDiag, SoftSpan};
use std::path::Path;

/// Design-only wrapper — SoftDiag API is `map_vhdl` / `map_vhdl_path`.
/// `synth_*` drops softs by design until callers switch to `map_vhdl*`.
pub fn synth_vhdl(source: &str) -> Result<Design, String> {
    Ok(map_vhdl(source)?.design)
}

/// VHDL → map: Helion `Design` plus first-class `helion_ir::SoftDiag` (FM-HEL-L3).
/// SOFT ≠ PASS — callers must not treat `softs` as a closed WNS / cells=0 success.
pub fn map_vhdl(source: &str) -> Result<MapResult, String> {
    map_vhdl_origin(source, "vhdl.vhd")
}

/// Design-only wrapper; softs discarded — use `map_vhdl_path` for SoftDiag.
pub fn synth_vhdl_path(path: &Path) -> Result<Design, String> {
    Ok(map_vhdl_path(path)?.design)
}

pub fn map_vhdl_path(path: &Path) -> Result<MapResult, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    map_vhdl_origin(&src, &path.display().to_string())
}

fn map_vhdl_origin(source: &str, origin: &str) -> Result<MapResult, String> {
    map_vhdl_top(source, origin, None)
}

/// Synthesize with an explicit top-level entity (`None`: the last entity no
/// other entity instantiates).
pub fn synth_vhdl_top(source: &str, top: Option<&str>) -> Result<Design, String> {
    Ok(map_vhdl_top(source, "vhdl.vhd", top)?.design)
}

fn map_vhdl_top(source: &str, origin: &str, top: Option<&str>) -> Result<MapResult, String> {
    if top.is_none() {
        if let Some(name) = osvvm_harness_module(source) {
            // OSVVM / PoC test harness (CreateClock, library.entity): a
            // simulation model, not a netlist.
            eprintln!("diagnostic sim_only module={name} (OSVVM test harness; not synthesized)");
            let soft = SoftDiag::new("sim_only", &name)
                .with_detail("OSVVM test harness; external entity not ingested")
                .with_span(span_best_effort(source, origin, &[&name]));
            emit_diag(&soft.table_line());
            return Ok(MapResult {
                design: empty_named(name),
                softs: vec![soft],
            });
        }
    }
    let (sv, renames) = v2::translate_full(source, top).map_err(|e| format!("{origin}: {e}"))?;
    let mut design = helion_sv::synth_sv(&sv, "vhdl.sv")?;
    restore_names(&mut design, &renames);
    Ok(MapResult {
        design,
        softs: Vec::new(),
    })
}

/// Undo the SV-keyword renaming of top-level names (`edge` was emitted as
/// `hv_kw_edge`): design name, ports, port-bit nets and IOB `PORT` attrs
/// carry the VHDL names constraints refer to.
fn restore_names(d: &mut Design, renames: &[(String, String)]) {
    if renames.is_empty() {
        return;
    }
    let map = |n: &str| -> Option<String> {
        for (orig, sv) in renames {
            if n == sv {
                return Some(orig.clone());
            }
            if let Some(rest) = n
                .strip_prefix(sv.as_str())
                .and_then(|r| r.strip_prefix('_'))
            {
                if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                    return Some(format!("{orig}_{rest}"));
                }
            }
        }
        None
    };
    if let Some(n) = map(&d.name) {
        d.name = n;
    }
    for p in d.ports.iter_mut() {
        if let Some(n) = map(&p.name) {
            p.name = n;
        }
    }
    for n in d.nets.iter_mut() {
        if let Some(m) = map(&n.name) {
            n.name = m;
        }
    }
    for c in d.cells.iter_mut() {
        if let Some(p) = c.attrs.get("PORT").and_then(|p| map(p)) {
            c.attrs.set("PORT", &p);
        }
    }
    d.rebuild_indexes();
}

fn ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// OSVVM clock/reset harness or library-qualified AXI testbench. Not synth RTL.
fn empty_named(name: String) -> Design {
    let mut d = Design::new(&name);
    d.attrs.set("SIM_ONLY", "1");
    d.attrs.set("NO_BODY", "1");
    d
}

fn osvvm_harness_module(source: &str) -> Option<String> {
    let low = source.to_ascii_lowercase();
    if !(low.contains("osvvm") || low.contains("createclock")) {
        return None;
    }
    Some(last_entity_is(source).unwrap_or_else(|| "osvvm_tb".into()))
}

fn last_entity_is(source: &str) -> Option<String> {
    let low = source.to_ascii_lowercase();
    let mut best = None;
    let mut search = 0usize;
    while let Some(rel) = low[search..].find("entity ") {
        let at = search + rel + "entity ".len();
        let rest = source.get(at..)?.trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let after = rest.get(name.len()..)?.trim_start();
        if !name.is_empty() && after.to_ascii_lowercase().starts_with("is") {
            best = Some(name);
        }
        search = at;
        if search >= source.len() {
            break;
        }
    }
    best
}

/// Translate VHDL source to the SystemVerilog that `helion_sv` synthesizes.
/// Exposed so the translation can be inspected and tested on its own.
pub fn vhdl_to_sv(source: &str) -> Result<String, String> {
    v2::translate(source, None)
}

fn emit_diag(line: &str) {
    eprintln!("{line}");
}

/// Best-effort file/line/column from original VHDL. First needle that appears
/// as a whole ident on a non-comment line wins; unknown fields stay `None`.
fn span_best_effort(source: &str, file: &str, needles: &[&str]) -> SoftSpan {
    let mut span = SoftSpan {
        file: Some(file.to_string()),
        line: None,
        column: None,
        end_line: None,
    };
    for needle in needles {
        if needle.is_empty() {
            continue;
        }
        if let Some((line, col)) = find_ident_line_col(source, needle) {
            span.line = Some(line);
            span.column = Some(col);
            return span;
        }
    }
    span
}

fn find_ident_line_col(source: &str, ident: &str) -> Option<(u32, u32)> {
    let want = ident.to_ascii_lowercase();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("--") {
            continue;
        }
        let low = line.to_ascii_lowercase();
        let mut from = 0usize;
        while from < low.len() {
            let rest = &low[from..];
            let Some(rel) = rest.find(&want) else {
                break;
            };
            let at = from + rel;
            let before_ok =
                at == 0 || !ident_char(low.as_bytes().get(at - 1).copied().unwrap_or(0));
            let after = at + want.len();
            let after_ok =
                after >= low.len() || !ident_char(low.as_bytes().get(after).copied().unwrap_or(0));
            if before_ok && after_ok {
                return Some(((i + 1) as u32, (at + 1) as u32));
            }
            from = at + 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, INC4_INIT};

    #[test]
    fn vhdl_skips_strings_and_concat_without_aborting_synth() {
        let src = r#"
entity e is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of e is
  constant C : string := "hello";
  signal q : std_logic := '0';
  signal w : std_logic_vector(1 downto 0);
begin
  w <= "10";
  process(clk)
  begin
    if rising_edge(clk) then
      q <= not q;
    end if;
  end process;
  led <= q;
end architecture;
"#;
        let d = synth_vhdl(src).expect("string/concat VHDL must still map the blinky process");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "FF from rising_edge process: {:?}",
            d.cells
        );
    }

    const BLINKY: &str = r#"
entity blinky is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of blinky is
  signal q : std_logic := '0';
begin
  process(clk)
  begin
    if rising_edge(clk) then
      q <= not q;
    end if;
  end process;
  led <= q;
end architecture;
"#;

    const INC3: &str = r#"
entity counter is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of counter is
  signal cnt : unsigned(2 downto 0) := (others => '0');
begin
  process(clk)
  begin
    if rising_edge(clk) then
      cnt <= cnt + 1;
    end if;
  end process;
  led <= cnt(2);
end architecture;
"#;

    const INC4: &str = r#"
entity counter is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of counter is
  signal cnt : unsigned(3 downto 0) := (others => '0');
begin
  process(clk)
  begin
    if rising_edge(clk) then
      cnt <= cnt + 1;
    end if;
  end process;
  led <= cnt(3);
end architecture;
"#;

    fn wave(src: &str, cycles: usize) -> (Design, Vec<bool>) {
        let d = synth_vhdl(src).unwrap();
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let p = helion_pack::pack(&d, &dev).unwrap();
        let pl = helion_place::place(&p, &dev).unwrap();
        let r = helion_route::route(&pl, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &r).unwrap();
        let mut fab = helion_fabric::Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        let iob = r.iob_src[0].iob;
        let mut w = Vec::new();
        for _ in 0..cycles {
            fab.step_user();
            w.push(fab.led_at(iob.0, iob.1));
        }
        (d, w)
    }

    #[test]
    fn vhdl_blinky_is_inverter_ff() {
        let d = synth_vhdl(BLINKY).unwrap();
        match d
            .cell("u_lut")
            .or_else(|| {
                d.cells
                    .iter()
                    .find(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            })
            .unwrap()
            .kind
        {
            CellKind::Lut6 { init } => assert_eq!(init, 0x5555_5555_5555_5555),
            _ => panic!("lut"),
        }
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "blinky.vhd must map an FF on clk, {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_garbage_fails() {
        assert!(synth_vhdl("not vhdl").is_err());
    }

    #[test]
    fn vhdl_blinky_toggles_in_fabric() {
        let (_, w) = wave(BLINKY, 8);
        assert!(
            w.contains(&true) && w.contains(&false),
            "VHDL blinky LED must toggle {w:?}"
        );
    }

    #[test]
    fn vhdl_width_is_not_string_match() {
        let d3 = synth_vhdl(INC3).unwrap();
        let d4 = synth_vhdl(INC4).unwrap();
        assert_eq!(
            d3.lut_inits().len(),
            3,
            "3-bit VHDL incrementer must be 3 LUTs {:?}",
            d3.lut_inits()
        );
        assert_eq!(
            d4.lut_inits(),
            INC4_INIT.to_vec(),
            "4-bit must match gold incrementer"
        );
        assert_ne!(d3.lut_inits().len(), d4.lut_inits().len());
    }

    #[test]
    fn vhdl_to_sv_is_not_a_noop_copy() {
        let sv = vhdl_to_sv(INC4).unwrap();
        assert!(
            sv.contains("module counter(input wire clk, output wire led)"),
            "{sv}"
        );
        assert!(
            sv.contains("always_ff @(posedge clk)") && sv.contains(" + 4'b0001"),
            "{sv}"
        );
        assert!(
            !sv.contains("entity") && !sv.contains("rising_edge"),
            "must lower VHDL, not echo it: {sv}"
        );
        let sv3 = vhdl_to_sv(INC3).unwrap();
        assert!(sv3.contains("reg [2:0]"), "3-bit register in SV {sv3}");
        assert!(sv.contains("reg [3:0]"), "4-bit register in SV {sv}");
    }

    #[test]
    fn vhdl_blinky_file_path_and_incrementer_e2e() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/blinky.vhd");
        let d = synth_vhdl_path(&root).unwrap();
        assert_eq!(d.lut_inits(), vec![0x5555_5555_5555_5555]);
    }

    #[test]
    fn vhdl_incrementer_e2e_like_counter_sv() {
        let (d, w) = wave(INC4, 16);
        assert_eq!(d.lut_inits().len(), 4);
        assert!(w[0..7].iter().all(|b| !b), "cnt 1..7 LED=0 {w:?}");
        assert!(w[7..15].iter().all(|b| *b), "cnt 8..15 LED=1 {w:?}");
        assert!(!w[15], "wrap {w:?}");
        let (_, w3) = wave(INC3, 8);
        assert!(w3[0..3].iter().all(|b| !b), "3-bit 1..3 LED=0 {w3:?}");
        assert!(w3[3..7].iter().all(|b| *b), "3-bit 4..7 LED=1 {w3:?}");
        assert!(!w3[7], "3-bit wrap {w3:?}");
    }

    #[test]
    fn vhdl_with_select_maps_mux() {
        let src = r#"
entity Mux4to1 is
  port (
    DecHor : in  std_logic_vector (3 downto 0);
    UniHor : in  std_logic_vector (3 downto 0);
    DecMin : in  std_logic_vector (3 downto 0);
    UniMin : in  std_logic_vector (3 downto 0);
    Sel    : in  std_logic_vector (1 downto 0);
    Tiempo : out std_logic_vector (3 downto 0));
end Mux4to1;
architecture Behavioral of Mux4to1 is
begin
   with Sel select
      Tiempo <= DecHor when "00",
                UniHor when "01",
                DecMin when "10",
                UniMin when others;
end Behavioral;
"#;
        let d = synth_vhdl(src).expect("with/select");
        assert!(
            !d.lut_inits().is_empty(),
            "with/select must map LUTs, cells={:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_when_else_and_neq() {
        let src = r#"
entity cmp_115 is
port (
eq : out std_logic;
in0 : in  std_logic_vector(2 downto 0);
in1 : in  std_logic_vector(2 downto 0)
);
end cmp_115;
architecture augh of cmp_115 is
signal tmp : std_logic;
begin
tmp <= '0' when in0 /= in1 else '1';
eq <= tmp;
end architecture;
"#;
        let d = synth_vhdl(src).expect("when/else");
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "when/else compare must be a LUT {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_empty_architecture_still_emits_module() {
        let src = r#"
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
begin
end;
"#;
        let sv = vhdl_to_sv(src).unwrap();
        assert!(sv.contains("module wrap"), "{sv}");
        let _ = synth_vhdl(src).expect("empty architecture is legal VHDL");
    }

    #[test]
    fn vhdl_fulladder_maps_luts() {
        let src = r#"
entity full_adder is
  port (a, b, cin : in std_logic; sum, cout : out std_logic);
end;
architecture rtl of full_adder is
begin
  sum <= a xor b xor cin;
  cout <= (a and b) or (a and cin) or (b and cin);
end;
"#;
        let d = synth_vhdl(src).expect("full_adder");
        let luts = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(
            luts >= 2,
            "full_adder must map LUTs, luts={luts} {:?}",
            d.cells
        );
        assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn vhdl_fulladder_hierarchy_adder4_maps() {
        let src = r#"
entity full_adder is
  port (a, b, cin : in std_logic; sum, cout : out std_logic);
end;
architecture rtl of full_adder is
begin
  sum <= a xor b xor cin;
  cout <= (a and b) or (a and cin) or (b and cin);
end;
entity adder4 is
  port (a, b : in std_logic_vector(3 downto 0); cin : in std_logic;
        sum : out std_logic_vector(3 downto 0); cout : out std_logic);
end;
architecture rtl of adder4 is
  component full_adder
    port (a, b, cin : in std_logic; sum, cout : out std_logic);
  end component;
  signal c : std_logic_vector(4 downto 0);
begin
  c(0) <= cin;
  u0: full_adder port map (a(0), b(0), c(0), sum(0), c(1));
  u1: full_adder port map (a(1), b(1), c(1), sum(1), c(2));
  u2: full_adder port map (a(2), b(2), c(2), sum(2), c(3));
  u3: full_adder port map (a(3), b(3), c(3), sum(3), c(4));
  cout <= c(4);
end;
"#;
        vhdl_to_sv(src).expect("adder4 to sv");
        let d = synth_vhdl(src).expect("adder4");
        let luts = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(
            luts >= 4,
            "adder4 hierarchy must map LUTs from full_adder bodies, luts={luts} cells={:?}",
            d.cells
        );
        assert_ne!(
            d.attrs.get("NO_BODY"),
            Some("1"),
            "adder4 must not be silent no_body"
        );
    }

    #[test]
    fn vhdl_map_closed_cone_has_no_softs() {
        let src = r#"
entity full_adder is
  port (a, b, cin : in std_logic; sum, cout : out std_logic);
end;
architecture rtl of full_adder is
begin
  sum <= a xor b xor cin;
  cout <= (a and b) or (a and cin) or (b and cin);
end;
"#;
        let mr = map_vhdl(src).expect("map");
        assert!(
            mr.softs.is_empty(),
            "closed LUT cone must not carry SOFT: {:?}",
            mr.soft_table_lines()
        );
        assert!(!mr.has_softs());
        let luts = mr
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(luts >= 2, "full_adder must map LUTs, luts={luts}");
    }

    #[test]
    fn vhdl_entity_constant_localparam_maps() {
        let src = r#"
entity const_or is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of const_or is
  constant MASK : integer := 1;
begin
  y <= not a when MASK = 1 else '0';
end;
"#;
        vhdl_to_sv(src).expect("sv");
        let d = synth_vhdl(src).expect("const");
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "entity constant path must still map, {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_entity_constant_in_entity_decl_maps() {
        let src = r#"
entity const_and is
  port (a : in std_logic; y : out std_logic);
  constant KEEP : integer := 1;
end;
architecture rtl of const_and is
  signal t : std_logic_vector(KEEP downto 0);
begin
  t <= (not a) & (not a);
  y <= t(0) when KEEP = 1 else '0';
end;
"#;
        vhdl_to_sv(src).expect("sv");
        let d = synth_vhdl(src).expect("const");
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "entity-decl constant must map, {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_entity_constant_generic_width_maps() {
        let src = r#"
entity bus_buf is
  generic (W : integer := 4);
  port (d : in std_logic_vector(3 downto 0); q : out std_logic_vector(3 downto 0));
end;
architecture rtl of bus_buf is
  constant ZERO : integer := 0;
  signal t : std_logic_vector(W-1 downto 0);
begin
  t <= d;
  q <= t;
end;
"#;
        vhdl_to_sv(src).expect("sv");
        let d = synth_vhdl(src).expect("buf");
        assert!(d.ports.iter().any(|p| p.name == "q"));
    }

    #[test]
    fn vhdl_rng_clock_is_sta_clk() {
        let src = r#"
entity rng is
  port (clock : in std_logic; rst : in std_logic; r : out std_logic_vector(7 downto 0));
end;
architecture rtl of rng is
  signal lfsr : std_logic_vector(7 downto 0) := "10101010";
begin
  process(clock)
  begin
    if rising_edge(clock) then
      if rst = '1' then
        lfsr <= "10101010";
      else
        lfsr <= lfsr(6 downto 0) & (lfsr(7) xor lfsr(5) xor lfsr(4) xor lfsr(3));
      end if;
    end if;
  end process;
  r <= lfsr;
end;
"#;
        let sv = vhdl_to_sv(src).expect("rng sv");
        assert!(
            sv.contains("posedge clock"),
            "always_ff must use clock: {sv}"
        );
        assert!(
            !sv.contains("posedge clk"),
            "must not rewrite process clock to clk: {sv}"
        );
        let d = synth_vhdl(src).expect("rng");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "rng must map FFs, {:?}",
            d.cells
        );
        assert!(
            d.ports.iter().any(|p| p.name == "clock"),
            "STA/Design port must keep real VHDL name clock: {:?}",
            d.ports
        );
        assert!(
            !d.ports.iter().any(|p| p.name == "clk"),
            "process/STA clock must not be renamed to clk: {:?}",
            d.ports
        );
    }

    #[test]
    fn vhdl_narrow_clock_keeps_data_clock_port() {
        // Process clock is `clk`. A separate port named `clock` is data — NARROW
        // must not total-merge every clock/clk ident into clk.
        let src = r#"
entity mix is
  port (clk : in std_logic; clock : in std_logic; q : out std_logic);
end;
architecture rtl of mix is
begin
  process(clk)
  begin
    if rising_edge(clk) then
      q <= clock;
    end if;
  end process;
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(sv.contains("posedge clk"), "{sv}");
        assert!(
            sv.contains("<= clock;"),
            "NBA must sample data port clock, not merge onto clk: {sv}"
        );
        assert!(
            !sv.contains("q = clk"),
            "must not rewrite data clock→clk: {sv}"
        );
        let d = synth_vhdl(src).expect("mix");
        assert!(d.ports.iter().any(|p| p.name == "clk"), "{:?}", d.ports);
        assert!(
            d.ports.iter().any(|p| p.name == "clock"),
            "NARROW keeps data port clock: {:?}",
            d.ports
        );
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
    }

    #[test]
    fn vhdl_selected_arch_clocks_not_unioned() {
        // Last arch (comb) treats `clock` as data. The non-selected sequential
        // arch must not force clock→clk on the selected body or on child_clocks.
        let src = r#"
entity child is
  port (clock : in std_logic; q : out std_logic);
end;
architecture seq of child is
begin
  process(clock)
  begin
    if rising_edge(clock) then
      q <= '1';
    end if;
  end process;
end;
architecture comb of child is
begin
  q <= clock;
end;
entity top is
  port (clock : in std_logic; q : out std_logic);
end;
architecture rtl of top is
begin
  u: child port map (clock => clock, q => q);
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(
            sv.contains("assign q = clock;"),
            "the last-analysed (combinational) architecture is the one used: {sv}"
        );
    }

    const MISSING: &str = r#"
library ieee; use ieee.std_logic_1164.all;
entity top is port (a : in std_logic; y : out std_logic); end;
architecture rtl of top is begin
  u_miss : entity work.missing_child port map (a => a, y => y);
end;
"#;

    #[test]
    fn vhdl_missing_entity_is_a_hard_error() {
        let e = synth_vhdl(MISSING).unwrap_err();
        assert!(e.contains("u_miss") && e.contains("missing_child"), "{e}");
        let in_generate = MISSING
            .replace(
                "u_miss : entity",
                "g : for i in 0 to 1 generate u_miss : entity",
            )
            .replace("y => y);", "y => y); end generate;");
        let e = synth_vhdl(&in_generate).unwrap_err();
        assert!(e.contains("u_miss") && e.contains("missing_child"), "{e}");
    }

    #[test]
    fn vhdl_async_load_is_rejected() {
        let src = r#"
library ieee; use ieee.std_logic_1164.all;
entity ic4021 is port (d, ds, pl, cp : in std_logic; q7 : out std_logic); end ic4021;
architecture behavior of ic4021 is
  signal r : std_logic := '0';
begin
  process (d, pl, cp) begin
    if pl = '1' then r <= d;
    elsif cp'event and cp = '1' then r <= ds;
    end if;
  end process;
  q7 <= r;
end behavior;
"#;
        let e = synth_vhdl(src).unwrap_err();
        assert!(e.contains("asynchronous load"), "{e}");
    }

    #[test]
    fn vhdl_latch_and_multiple_drivers_are_rejected() {
        let latch = r#"
library ieee; use ieee.std_logic_1164.all;
entity l is port (en, d : in std_logic; q : out std_logic); end;
architecture rtl of l is begin
  process (en, d) begin if en = '1' then q <= d; end if; end process;
end;
"#;
        let e = synth_vhdl(latch).unwrap_err();
        assert!(e.contains("latch") && e.contains('q'), "{e}");
        let multi = r#"
library ieee; use ieee.std_logic_1164.all;
entity m is port (a, b : in std_logic; q : out std_logic); end;
architecture rtl of m is begin
  q <= a;
  q <= b;
end;
"#;
        let e = synth_vhdl(multi).unwrap_err();
        assert!(e.contains("more than one driver"), "{e}");
    }

    #[test]
    fn vhdl_sv_keyword_port_names_survive() {
        let src = r#"
library ieee; use ieee.std_logic_1164.all;
entity top is port (clk, d : in std_logic; edge : out std_logic; output : out std_logic_vector(1 downto 0)); end;
architecture rtl of top is
  signal p : std_logic := '0';
begin
  process (clk) begin if rising_edge(clk) then p <= d; end if; end process;
  edge <= d and not p;
  output <= p & d;
end;
"#;
        let d = synth_vhdl(src).unwrap();
        let names: Vec<&str> = d.ports.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"edge") && names.contains(&"output"),
            "{names:?}"
        );
        let iob_ports: Vec<&str> = d.cells.iter().filter_map(|c| c.attrs.get("PORT")).collect();
        assert!(
            iob_ports.contains(&"edge") && iob_ports.contains(&"output"),
            "{iob_ports:?}"
        );
    }

    #[test]
    fn vhdl_records_functions_and_generics_elaborate() {
        let src = r#"
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
package p is
  type pair_t is record lo, hi : unsigned(3 downto 0); end record;
  type tbl_t is array (natural range <>) of pair_t;
  function swap(x : pair_t) return pair_t;
end package;
package body p is
  function swap(x : pair_t) return pair_t is
    variable r : pair_t;
  begin
    r.lo := x.hi; r.hi := x.lo; return r;
  end function;
end package body;
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all; use work.p.all;
entity top is generic (N : natural := 2); port (i : in std_logic_vector(1 downto 0); y : out std_logic_vector(7 downto 0)); end;
architecture rtl of top is
  constant T : tbl_t(0 to 3) := ((x"1", x"2"), (x"3", x"4"), (x"5", x"6"), (x"7", x"8"));
  signal s : pair_t;
begin
  s <= swap(T(to_integer(unsigned(i))));
  y <= std_logic_vector(s.hi & s.lo) when N = 2 else (others => '0');
end;
"#;
        let sv = vhdl_to_sv(src).unwrap();
        assert!(sv.contains("module top("), "{sv}");
        synth_vhdl(src).unwrap();
    }
}
