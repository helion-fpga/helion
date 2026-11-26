//! Guards for the VHDL ROM gaps found on a two-layer image classifier
//! (HELION_GAPS G96, G97, G99): a ROM filled by an impure function reading
//! a file with `std.textio`, at the sizes that design uses, translates fast
//! into a SystemVerilog memory holding the file's contents.

use helion_zb_front::vhdl_to_sv;

const ROM: &str = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use std.textio.all;
use ieee.std_logic_textio.all;

entity rom_sync is
  generic (FILE_NAME : string; DEPTH : positive; WIDTH : positive);
  port (clk : in std_logic; addr : in unsigned;
        q : out std_logic_vector(WIDTH-1 downto 0) := (others => '0'));
end entity;

architecture rtl of rom_sync is
  type mem_t is array (0 to DEPTH-1) of std_logic_vector(WIDTH-1 downto 0);
  impure function load return mem_t is
    file f     : text open read_mode is FILE_NAME;
    variable l : line;
    variable b : bit_vector(WIDTH-1 downto 0);
    variable m : mem_t;
  begin
    for a in mem_t'range loop
      assert not endfile(f) report FILE_NAME & " is short" severity failure;
      readline(f, l);
      if WIDTH mod 4 = 0 then
        hread(l, m(a));
      else
        read(l, b);
        m(a) := to_stdlogicvector(b);
      end if;
    end loop;
    return m;
  end function;
  signal mem : mem_t := load;
begin
  process (clk) begin
    if rising_edge(clk) then
      if to_integer(addr) < DEPTH then q <= mem(to_integer(addr)); end if;
    end if;
  end process;
end architecture;
"#;

fn top(file: &str, depth: usize, width: usize, abits: usize) -> String {
    format!(
        "{ROM}
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
entity t is
  port (clk : in std_logic; a : in unsigned({}-1 downto 0); q : out std_logic_vector({width}-1 downto 0));
end entity;
architecture rtl of t is
begin
  u : entity work.rom_sync generic map (FILE_NAME => \"{file}\", DEPTH => {depth}, WIDTH => {width})
    port map (clk => clk, addr => a, q => q);
end architecture;
",
        abits
    )
}

/// The entries of the one `reg ... hv_mN [0:D-1] = '{...}` memory.
fn rom_entries(sv: &str) -> Vec<u64> {
    let l = sv.lines().find(|l| l.trim_start().starts_with("reg") && l.contains("hv_m")).expect("a ROM memory");
    let body = &l[l.find("'{").unwrap() + 2..l.rfind('}').unwrap()];
    body.split(", ")
        .map(|x| {
            let (_, v) = x.split_once('\'').unwrap();
            let radix = if v.starts_with('h') { 16 } else { 2 };
            u64::from_str_radix(&v[1..], radix).unwrap()
        })
        .collect()
}

fn write(tag: &str, text: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("helion_vhdl_rom_{tag}_{}.mem", std::process::id()));
    std::fs::write(&p, text).unwrap();
    p
}

/// G96 + G97: 50,176 x 8 by `hread`, a 784 x 64 weight layer's shape.
#[test]
fn hread_rom_of_50176_bytes_is_a_memory_with_the_file_contents() {
    let vals: Vec<u64> = (0..50176u64).map(|i| (i * 37 + 11) & 255).collect();
    let f = write("w1", &vals.iter().map(|v| format!("{v:02x}\n")).collect::<String>());
    let t = std::time::Instant::now();
    let (sv, _) = vhdl_to_sv(&top(&f.display().to_string(), 50176, 8, 16), Some("t")).unwrap();
    assert!(t.elapsed().as_secs() < 20, "took {:?}", t.elapsed());
    assert!(sv.contains("[0:50175]"), "a 50,176-entry memory");
    assert_eq!(rom_entries(&sv), vals);
    let _ = std::fs::remove_file(&f);
}

/// G96 + G97: 200,704 x 1 by `read` into a bit_vector (the 256 images),
/// more iterations than the old loop limit of 65,536.
#[test]
fn binary_read_rom_of_200704_bits_is_a_memory_with_the_file_contents() {
    let vals: Vec<u64> = (0..200704u64).map(|i| (i * 2654435761 >> 7) & 1).collect();
    let f = write("img", &vals.iter().map(|v| format!("{v}\n")).collect::<String>());
    let t = std::time::Instant::now();
    let (sv, _) = vhdl_to_sv(&top(&f.display().to_string(), 200704, 1, 18), Some("t")).unwrap();
    assert!(t.elapsed().as_secs() < 30, "took {:?}", t.elapsed());
    assert_eq!(rom_entries(&sv), vals);
    let _ = std::fs::remove_file(&f);
}

/// G96: a short file fails loudly instead of loading zeros.
#[test]
fn short_rom_file_is_an_error() {
    let f = write("short", "01\n02\n");
    let e = vhdl_to_sv(&top(&f.display().to_string(), 256, 8, 8), Some("t")).unwrap_err();
    assert!(e.contains("past the end"), "{e}");
    let _ = std::fs::remove_file(&f);
}

/// G99: a 64-entry array read by a runtime index is a balanced mux tree
/// (6 levels of 2:1 muxes), not a 64-deep priority chain (33 LUT levels on
/// a 64-entry hidden-layer read).
#[test]
fn small_array_read_is_a_mux_tree() {
    let src = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
entity h is
  port (clk : in std_logic; we : in std_logic; wa, ra : in unsigned(5 downto 0);
        d : in signed(15 downto 0); q : out signed(15 downto 0));
end entity;
architecture rtl of h is
  type hidden_t is array (0 to 63) of signed(15 downto 0);
  signal hidden : hidden_t := (others => (others => '0'));
begin
  process (clk) begin
    if rising_edge(clk) then
      if we = '1' then hidden(to_integer(wa)) <= d; end if;
      q <= hidden(to_integer(ra));
    end if;
  end process;
end architecture;
"#;
    let (sv, _) = vhdl_to_sv(src, Some("h")).unwrap();
    // The read compares no 6-bit constant with the address: it selects on
    // its bits.
    let compares = sv.lines().filter(|l| l.contains("== ra") || l.contains("ra ==")).count();
    assert_eq!(compares, 0, "{sv}");
    let selects = sv.lines().filter(|l| l.contains("= ra[")).count();
    assert!(selects >= 6, "one select per address bit:\n{sv}");
}
