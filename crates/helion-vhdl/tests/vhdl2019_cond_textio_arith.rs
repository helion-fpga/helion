use helion_ir::CellKind;
use helion_vhdl::{synth_vhdl, synth_vhdl_path};
use std::path::PathBuf;

// ============================================================================
// 1. VHDL-2019 Inline Conditional Expressions (`when ... else`)
// ============================================================================

#[test]
fn vhdl2019_inline_conditional_expressions_in_arith_concat_and_process() {
    let vhd = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity vhdl2019_cond_top is
    port (
        clk : in  std_logic;
        s0  : in  std_logic;
        s1  : in  std_logic;
        a   : in  unsigned(3 downto 0);
        b   : in  unsigned(3 downto 0);
        c   : in  unsigned(3 downto 0);
        d   : in  unsigned(3 downto 0);
        y_q : out unsigned(3 downto 0);
        z_q : out std_logic_vector(3 downto 0)
    );
end entity;

architecture rtl of vhdl2019_cond_top is
    signal comb_sum : unsigned(3 downto 0);
begin
    -- Parenthesized VHDL-2019 conditional expressions inside arithmetic
    comb_sum <= (a when s0 = '1' else b) + (c when s1 = '1' else d);

    process(clk)
    begin
        if rising_edge(clk) then
            -- Sequential conditional signal assignment with chained when/else
            y_q <= comb_sum when s0 = '1' else a when s1 = '1' else b;
            -- Inline conditional inside concatenation and type cast
            z_q <= std_logic_vector((a(1 downto 0) when s0 = '1' else b(1 downto 0)) & (c(1 downto 0) when s1 = '1' else d(1 downto 0)));
        end if;
    end process;
end architecture;
"#;
    let d = synth_vhdl(vhd).expect("VHDL-2019 conditional expressions must synthesize");
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert_eq!(ffs, 8, "y_q(4) + z_q(4) must infer 8 HFFs, got {ffs}");
    assert!(
        luts >= 8,
        "VHDL-2019 inline conditional expressions must map LUT6 cones, got {luts}"
    );
}

// ============================================================================
// 2. `std.textio` & `ieee.std_logic_textio` Compile-Time ROM/BRAM Initialization
// ============================================================================

#[test]
fn vhdl_textio_hread_and_bread_rom_bram_initialization() {
    let mut dir: PathBuf = std::env::temp_dir();
    dir.push(format!("helion_vhdl_textio_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");

    std::fs::write(dir.join("init_hex.mem"), "A5\n3C\nF0\n0F\n").expect("write hex mem");
    std::fs::write(
        dir.join("init_bin.mem"),
        "10100101\n00111100\n11110000\n00001111\n",
    )
    .expect("write bin mem");

    let vhd_path = dir.join("textio_rom.vhd");
    let vhd = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
use std.textio.all;
use ieee.std_logic_textio.all;

entity textio_rom is
    port (
        clk   : in  std_logic;
        addr  : in  std_logic_vector(1 downto 0);
        hex_q : out std_logic_vector(7 downto 0);
        bin_q : out std_logic_vector(7 downto 0)
    );
end entity;

architecture rtl of textio_rom is
    type rom_t is array (0 to 3) of std_logic_vector(7 downto 0);

    impure function init_rom_hex(filename : in string) return rom_t is
        file f : text open read_mode is filename;
        variable l : line;
        variable rom : rom_t;
    begin
        for i in 0 to 3 loop
            if not endfile(f) then
                readline(f, l);
                hread(l, rom(i));
            end if;
        end loop;
        return rom;
    end function;

    impure function init_rom_bin return rom_t is
        file f : text open read_mode is "init_bin.mem";
        variable l : line;
        variable rom : rom_t;
    begin
        while not endfile(f) loop
            readline(f, l);
            bread(l, rom(0));
        end loop;
        return rom;
    end function;

    signal rom_h : rom_t := init_rom_hex("init_hex.mem");
    constant rom_b : rom_t := init_rom_bin;
begin
    process(clk)
    begin
        if rising_edge(clk) then
            hex_q <= rom_h(to_integer(unsigned(addr)));
            bin_q <= rom_b(to_integer(unsigned(addr)));
        end if;
    end process;
end architecture;
"#;
    std::fs::write(&vhd_path, vhd).expect("write vhd");

    let d = synth_vhdl_path(&vhd_path).expect("synth textio_rom.vhd");
    let brams: Vec<_> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Bram18 | CellKind::Bram18e1 { .. }))
        .collect();
    assert_eq!(brams.len(), 2, "rom_h and rom_b must infer 2 BRAM18 cells");
    for b in &brams {
        let init = b.attrs.get("INIT").unwrap_or("");
        assert!(
            init.contains("a5") && init.contains("3c") && init.contains("f0") && init.contains("f"),
            "BRAM INIT must carry textio-loaded words, got {init}"
        );
    }
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(ffs, 16, "hex_q(8) + bin_q(8) must infer 16 HFFs, got {ffs}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ============================================================================
// 3. `numeric_std_unsigned` & `std_logic_arith` Compatibility
// ============================================================================

#[test]
fn vhdl_numeric_std_unsigned_and_std_logic_arith_operators_and_conversions() {
    let vhd = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std_unsigned.all;
use ieee.std_logic_arith.all;
use ieee.std_logic_unsigned.all;

entity slv_arith_top is
    port (
        clk   : in  std_logic;
        a     : in  std_logic_vector(3 downto 0);
        b     : in  std_logic_vector(3 downto 0);
        sh    : in  std_logic_vector(1 downto 0);
        sum_q : out std_logic_vector(3 downto 0);
        shl_q : out std_logic_vector(3 downto 0);
        shr_q : out std_logic_vector(3 downto 0);
        cmp_q : out std_logic
    );
end entity;

architecture rtl of slv_arith_top is
begin
    process(clk)
    begin
        if rising_edge(clk) then
            sum_q <= (a + b) - conv_std_logic_vector(3, 4);
            shl_q <= shl(a, sh);
            shr_q <= shr(b, sh);
            if (a <= b) and (conv_integer(a) < 12) then
                cmp_q <= '1';
            else
                cmp_q <= '0';
            end if;
        end if;
    end process;
end architecture;
"#;
    let d = synth_vhdl(vhd).expect("numeric_std_unsigned / std_logic_arith must synthesize");
    let ffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert_eq!(
        ffs,
        4 + 4 + 4 + 1,
        "sum_q(4) + shl_q(4) + shr_q(4) + cmp_q(1) must infer 13 HFFs, got {ffs}"
    );
    assert!(
        luts >= 10,
        "std_logic_vector arithmetic, shl/shr, and comparisons must map LUT6 cones, got {luts}"
    );
}
