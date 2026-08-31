//! Original VHDL-2008 subset → Helion SV elaborator (not UNISIM, not string-match).
//!
//! entity / architecture / process / rising_edge / not / + / downto vectors.

use helion_ir::Design;
use std::path::Path;

fn vhdl_to_sv(source: &str) -> Result<String, String> {
    let t = tokenize(source)?;
    let mut p = P { t: &t, i: 0 };
    while p.eat("library") || p.eat("use") {
        while p.peek().is_some() && !p.eat(";") {
            p.bump();
        }
    }
    if !p.eat("entity") {
        return Err("vhdl: need entity".into());
    }
    let entity = p.ident()?;
    let _ = p.eat("is");
    let mut ports: Vec<(String, &'static str, usize)> = Vec::new();
    if p.eat("port") {
        if !p.eat("(") {
            return Err("port (".into());
        }
        loop {
            if p.eat(")") {
                break;
            }
            if p.peek().is_none() {
                return Err("unterminated port".into());
            }
            let name = p.ident()?;
            let _ = p.eat(":");
            let dir = if p.eat("in") {
                "input"
            } else if p.eat("out") {
                "output"
            } else if p.eat("inout") {
                "input"
            } else {
                return Err("port dir".into());
            };
            let w = parse_type(&mut p)?;
            ports.push((name, dir, w));
            let _ = p.eat(";");
        }
    }
    let _ = p.eat(";");
    let _ = p.eat("end");
    let _ = p.eat("entity");
    let _ = p.eat_ident();
    let _ = p.eat(";");
    if !p.eat("architecture") {
        return Err("vhdl: need architecture".into());
    }
    let _arch = p.ident()?;
    let _ = p.eat("of");
    let _ = p.ident();
    let _ = p.eat("is");
    let mut signals: Vec<(String, usize)> = Vec::new();
    while !p.eat("begin") {
        if p.peek().is_none() {
            return Err("architecture begin".into());
        }
        if p.eat("signal") {
            let n = p.ident()?;
            let _ = p.eat(":");
            let w = parse_type(&mut p)?;
            skip_init(&mut p);
            let _ = p.eat(";");
            signals.push((n, w));
        } else {
            p.bump();
        }
    }
    let mut nbas: Vec<String> = Vec::new();
    let mut assigns: Vec<String> = Vec::new();
    while !p.eat("end") {
        if p.peek().is_none() {
            return Err("unterminated architecture".into());
        }
        if p.eat("process") {
            let _ = p.eat("(");
            while !p.eat(")") && p.peek().is_some() {
                p.bump();
            }
            let _ = p.eat("begin");
            while !p.eat("end") {
                if p.peek().is_none() {
                    return Err("unterminated process".into());
                }
                if p.eat("if") {
                    let _ = p.eat("rising_edge");
                    let _ = p.eat("(");
                    let _ = p.ident();
                    let _ = p.eat(")");
                    let _ = p.eat("then");
                    while !p.eat("end") {
                        if p.peek().is_none() {
                            break;
                        }
                        if let Some(line) = parse_assign_sv(&mut p)? {
                            nbas.push(line);
                        }
                    }
                    let _ = p.eat("if");
                    let _ = p.eat(";");
                } else {
                    p.bump();
                }
            }
            let _ = p.eat("process");
            let _ = p.eat(";");
            continue;
        }
        if let Some(line) = parse_assign_sv(&mut p)? {
            assigns.push(line);
            continue;
        }
        p.bump();
    }
    // emit SV
    let mut sv = String::new();
    sv.push_str(&format!("module {entity}("));
    let plist: Vec<String> = ports
        .iter()
        .map(|(n, d, w)| {
            if *w == 1 {
                format!("{d} logic {n}")
            } else {
                format!("{d} logic [{}:0] {n}", w - 1)
            }
        })
        .collect();
    sv.push_str(&plist.join(", "));
    sv.push_str(");\n");
    for (n, w) in &signals {
        if *w == 1 {
            sv.push_str(&format!("  logic {n};\n"));
        } else {
            sv.push_str(&format!("  logic [{}:0] {n};\n", w - 1));
        }
    }
    if !nbas.is_empty() {
        sv.push_str("  always_ff @(posedge clk) begin\n");
        for l in &nbas {
            sv.push_str(&format!("    {l}\n"));
        }
        sv.push_str("  end\n");
    }
    for l in &assigns {
        sv.push_str(&format!("  assign {l}\n"));
    }
    sv.push_str("endmodule\n");
    Ok(sv)
}

fn parse_type(p: &mut P) -> Result<usize, String> {
    let _ = p.eat("std_logic");
    let _ = p.eat("std_logic_vector");
    let _ = p.eat("unsigned");
    let _ = p.eat("signed");
    if p.eat("(") {
        let msb = p.number().unwrap_or(0) as usize;
        let _ = p.eat("downto");
        let _ = p.eat("to");
        let lsb = p.number().unwrap_or(0) as usize;
        let _ = p.eat(")");
        return Ok(msb.max(lsb) - msb.min(lsb) + 1);
    }
    Ok(1)
}

fn skip_init(p: &mut P) {
    if p.eat(":=") {
        let mut depth = 0i32;
        while p.peek().is_some() {
            if depth == 0 && p.peek_sym(";") {
                break;
            }
            if p.eat("(") {
                depth += 1;
            } else if p.eat(")") {
                depth -= 1;
            } else {
                p.bump();
            }
        }
    }
}

fn parse_assign_sv(p: &mut P) -> Result<Option<String>, String> {
    let Some(Tok::Ident(_)) = p.peek() else {
        return Ok(None);
    };
    let start = p.i;
    let lhs = p.ident()?;
    let mut lhs_sv = lhs.clone();
    if p.eat("(") {
        if let Some(n) = p.number() {
            lhs_sv = format!("{lhs}[{n}]");
        }
        let _ = p.eat(")");
    }
    if !p.eat("<=") {
        p.i = start;
        return Ok(None);
    }
    let rhs = parse_vhdl_expr(p)?;
    let _ = p.eat(";");
    Ok(Some(format!("{lhs_sv} = {rhs};")))
}

fn parse_vhdl_expr(p: &mut P) -> Result<String, String> {
    if p.eat("not") {
        let x = parse_vhdl_expr(p)?;
        return Ok(format!("~{x}"));
    }
    if p.eat("'") {
        let b = match p.bump() {
            Some(Tok::Ident(s)) | Some(Tok::Kw(s)) => s.clone(),
            Some(Tok::Num(n)) => n.to_string(),
            Some(Tok::Sym(c)) => c.to_string(),
            _ => "0".into(),
        };
        let _ = p.eat("'");
        return Ok(if b.starts_with('1') { "1'b1".into() } else { "1'b0".into() });
    }
    let mut e = if let Some(Tok::Ident(_)) = p.peek() {
        let n = p.ident()?;
        if p.eat("(") {
            let idx = p.number().unwrap_or(0);
            let _ = p.eat(")");
            format!("{n}[{idx}]")
        } else {
            n
        }
    } else if let Some(n) = p.number() {
        n.to_string()
    } else if p.eat("(") {
        let inner = parse_vhdl_expr(p)?;
        let _ = p.eat(")");
        format!("({inner})")
    } else {
        return Err("vhdl expr".into());
    };
    if p.eat("+") {
        let r = parse_vhdl_expr(p)?;
        e = format!("{e} + {r}");
    }
    Ok(e)
}

#[derive(Clone, Debug)]
enum Tok {
    Ident(String),
    Kw(String),
    Num(u128),
    Sym(String),
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }
    fn peek_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if x == s)
    }
    fn bump(&mut self) -> Option<&'a Tok> {
        let t = self.t.get(self.i)?;
        self.i += 1;
        Some(t)
    }
    fn eat(&mut self, s: &str) -> bool {
        match self.peek() {
            Some(Tok::Kw(k)) if k.eq_ignore_ascii_case(s) => {
                self.i += 1;
                true
            }
            Some(Tok::Sym(k)) if k == s => {
                self.i += 1;
                true
            }
            Some(Tok::Ident(k)) if k.eq_ignore_ascii_case(s) && is_kw(s) => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Some(Tok::Ident(s)) => Ok(s.clone()),
            Some(Tok::Kw(s)) => Ok(s.clone()),
            other => Err(format!("ident {other:?}")),
        }
    }
    fn eat_ident(&mut self) -> bool {
        matches!(self.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))).then(|| self.bump()).is_some()
    }
    fn number(&mut self) -> Option<u128> {
        match self.peek() {
            Some(Tok::Num(n)) => {
                let v = *n;
                self.i += 1;
                Some(v)
            }
            _ => None,
        }
    }
}

fn is_kw(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "entity"
            | "architecture"
            | "port"
            | "is"
            | "in"
            | "out"
            | "inout"
            | "end"
            | "of"
            | "begin"
            | "signal"
            | "process"
            | "if"
            | "then"
            | "not"
            | "downto"
            | "to"
            | "rising_edge"
            | "std_logic"
            | "std_logic_vector"
            | "unsigned"
            | "signed"
            | "others"
            | "library"
            | "use"
            | "all"
            | "package"
            | "constant"
            | "variable"
    )
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let mut s = String::new();
    for line in src.lines() {
        s.push_str(line.split("--").next().unwrap_or(""));
        s.push('\n');
    }
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '<' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym("<=".into()));
            i += 2;
            continue;
        }
        if c == ':' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym(":=".into()));
            i += 2;
            continue;
        }
        if c == '=' && chars.get(i + 1) == Some(&'>') {
            out.push(Tok::Sym("=>".into()));
            i += 2;
            continue;
        }
        if "();:+-',.=<>".contains(c) {
            out.push(Tok::Sym(c.to_string()));
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut n = String::new();
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                n.push(chars[i]);
                i += 1;
            }
            if is_kw(&n) {
                out.push(Tok::Kw(n.to_ascii_lowercase()));
            } else {
                out.push(Tok::Ident(n));
            }
            continue;
        }
        if c.is_ascii_digit() {
            let mut n = String::new();
            while i < chars.len() && chars[i].is_ascii_digit() {
                n.push(chars[i]);
                i += 1;
            }
            out.push(Tok::Num(n.parse().unwrap_or(0)));
            continue;
        }
        return Err(format!("vhdl char {c:?}"));
    }
    Ok(out)
}

pub fn synth_vhdl(source: &str) -> Result<Design, String> {
    let sv = vhdl_to_sv(source)?;
    helion_sv::synth_sv(&sv, "vhdl.sv")
}

pub fn synth_vhdl_path(path: &Path) -> Result<Design, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    synth_vhdl(&src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, INC4_INIT};

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
        match d.cell("u_lut").or_else(|| d.cells.iter().find(|c| matches!(c.kind, CellKind::Lut6 { .. }))).unwrap().kind {
            CellKind::Lut6 { init } => assert_eq!(init, 0x5555_5555_5555_5555),
            _ => panic!("lut"),
        }
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
        assert_eq!(d3.lut_inits().len(), 3, "3-bit VHDL incrementer must be 3 LUTs {:?}", d3.lut_inits());
        assert_eq!(d4.lut_inits(), INC4_INIT.to_vec(), "4-bit must match gold incrementer");
        assert_ne!(d3.lut_inits().len(), d4.lut_inits().len());
    }

    #[test]
    fn vhdl_to_sv_is_not_a_noop_copy() {
        let sv = vhdl_to_sv(INC4).unwrap();
        assert!(sv.contains("module counter"), "{sv}");
        assert!(sv.contains("always_ff"), "{sv}");
        assert!(sv.contains("cnt + 1") || sv.contains("cnt+1"), "{sv}");
        assert!(!sv.contains("entity"), "must lower VHDL, not echo it: {sv}");
        assert!(!sv.contains("rising_edge"), "{sv}");
        let sv3 = vhdl_to_sv(INC3).unwrap();
        assert!(sv3.contains("[2:0]"), "3-bit vector in SV {sv3}");
        assert!(sv.contains("[3:0]"), "4-bit vector in SV {sv}");
        assert_ne!(sv3, sv);
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
        // LED = cnt[2]; after k steps cnt=k; LED=1 for k=4,5,6,7
        assert!(w3[0..3].iter().all(|b| !b), "3-bit 1..3 LED=0 {w3:?}");
        assert!(w3[3..7].iter().all(|b| *b), "3-bit 4..7 LED=1 {w3:?}");
        assert!(!w3[7], "3-bit wrap {w3:?}");
    }
}
