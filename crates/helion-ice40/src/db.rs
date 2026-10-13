//! Parser for the Project IceStorm routing databases.
//!
//! The text files in `data/` are extracted from IceStorm's `iceboxdb.py`
//! (ISC licensed, (C) Clifford Wolf). Each line is:
//! `<bit-pattern>\t<function>\t<src-net>\t<dst-net>`
//! where `<bit-pattern>` is a comma-separated list of `B<row>[<bit>]` or
//! `!B<row>[<bit>]` requirements. This module parses them into structured
//! entries and applies IceStorm's net-name normalization so net names match
//! the documented (`lutff_0/out`, `local_g0_0`, ...) scheme.

/// A single configuration bit requirement: row `b`, bit `i`, must be `val`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct BitReq {
    pub row: u8,
    pub bit: u8,
    pub val: bool,
}

/// The function class of a database entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DbFunc {
    /// Configurable buffer: setting the bits connects src to dst.
    Buffer,
    /// Configurable routing mux: setting the bits connects src to dst.
    Routing,
    /// Logic-cell config bits (`LC_0` .. `LC_7`): 20 bits of LC function.
    LcConfig(u8),
    /// Anything else (ColBufCtrl, IoCtrl, IOB_x/PINTYPE_x, RamConfig, ...).
    /// Kept as the raw function string for special-case handling.
    Other(String),
}

/// One parsed database entry.
#[derive(Clone, Debug)]
pub struct DbEntry {
    pub bits: Vec<BitReq>,
    pub func: DbFunc,
    pub src: String,
    pub dst: String,
}

/// Parse one `B<row>[<bit>]` / `!B<row>[<bit>]` token.
fn parse_bitreq(tok: &str) -> Option<BitReq> {
    let tok = tok.trim();
    let (tok, val) = match tok.strip_prefix('!') {
        Some(rest) => (rest, false),
        None => (tok, true),
    };
    let tok = tok.strip_prefix('B')?;
    let (row, bit) = tok.split_once('[')?;
    let bit = bit.strip_suffix(']')?;
    Some(BitReq {
        row: row.parse().ok()?,
        bit: bit.parse().ok()?,
        val,
    })
}

/// Parse one database line. Returns `None` for blank lines.
pub fn parse_line(line: &str) -> Option<DbEntry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut parts = line.split('\t');
    let bitpat = parts.next()?;
    let func = parts.next()?.to_string();
    let src = parts.next().unwrap_or("").to_string();
    let dst = parts.next().unwrap_or("").to_string();

    let mut bits = Vec::new();
    for tok in bitpat.split(',') {
        bits.push(parse_bitreq(tok)?);
    }

    let func = match func.as_str() {
        "buffer" => DbFunc::Buffer,
        "routing" => DbFunc::Routing,
        _ if func.starts_with("LC_") => {
            let idx: u8 = func[3..].parse().ok()?;
            DbFunc::LcConfig(idx)
        }
        other => DbFunc::Other(other.to_string()),
    };

    Some(DbEntry { bits, func, src, dst })
}

/// Parse a whole database text into entries.
pub fn parse_db(text: &str) -> Vec<DbEntry> {
    text.lines().filter_map(parse_line).collect()
}

/// Map an iCE40-8k RAM tile `inputN_M` net to its `ram/RADDR_*` /
/// `ram/WADDR_*` name, ported from IceStorm `icebox.py::netname_normalize`
/// (`ramb_8k` / `ramt_8k` flavors).
///
/// On 8k the RAMB tile (odd y) carries the read-address inputs and the
/// RAMT tile (even y) the write-address inputs; `ramb` selects which.
/// The index table is `[7,6,5,4,3,2,1,0,-1,...,-1,10,9,8]` addressed by
/// `N*4+M`; a `-1` entry (or a non-`input` name) is returned unchanged.
pub fn normalize_ram_input(net: &str, ramb: bool) -> String {
    const TABLE: [i8; 16] = [7, 6, 5, 4, 3, 2, 1, 0, -1, -1, -1, -1, -1, 10, 9, 8];
    let rest = match net.strip_prefix("input") {
        Some(r) => r,
        None => return net.to_string(),
    };
    let (ns, ms) = match rest.split_once('_') {
        Some(p) => p,
        None => return net.to_string(),
    };
    let (n, m): (usize, usize) = match (ns.parse(), ms.parse()) {
        (Ok(n), Ok(m)) => (n, m),
        _ => return net.to_string(),
    };
    let idx = n.checked_mul(4).and_then(|v| v.checked_add(m));
    let bit = match idx {
        Some(i) if i < TABLE.len() => TABLE[i],
        _ => return net.to_string(),
    };
    if bit < 0 {
        return net.to_string();
    }
    let port = if ramb { "RADDR" } else { "WADDR" };
    format!("ram/{port}_{bit}")
}

/// IceStorm net-name normalization, ported from `icebox.py::netname_normalize`
/// (logic-tile flavor; RAM `inputN_M` names are handled by
/// [`normalize_ram_input`], applied by the caller for RAM tile kinds).
pub fn normalize_netname(net: &str) -> String {
    let mut n = net.to_string();
    // input_2_N -> wire_logic_cluster/lc_N/in_2  (handled before other replaces)
    if let Some(rest) = n.strip_prefix("input_2_") {
        n = format!("wire_logic_cluster/lc_{rest}/in_2");
    }
    n = n.replace("lc_trk_", "local_");
    n = n.replace("lc_", "lutff_");
    n = n.replace("wire_logic_cluster/", "");
    n = n.replace("wire_io_cluster/", "");
    n = n.replace("wire_mult/", "");
    n = n.replace("wire_con_box/", "");
    n = n.replace("wire_bram/", "");
    // (logic|neigh)_op_XXX_N -> neigh_op_XXX_N  (except slf_op)
    if let Some(rest) = n.strip_prefix("logic_op_") {
        n = format!("neigh_op_{rest}");
    } else if let Some(rest) = n.strip_prefix("neigh_op_") {
        n = format!("neigh_op_{rest}");
    }
    if n.starts_with("lutff_7/cen") || n.starts_with("lutff_7/clk") || n.starts_with("lutff_7/s_r") {
        n = n.replacen("lutff_7/", "lutff_global/", 1);
    }
    if n.starts_with("io_1/cen") || n.starts_with("io_1/inclk") || n.starts_with("io_1/outclk") {
        n = n.replacen("io_1/", "io_global/", 1);
    }
    if n == "carry_in_mux/cout" {
        n = "carry_in_mux".to_string();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bit_pattern() {
        let e = parse_line("B0[4],!B1[4],B1[7]\tbuffer\ta\tb").unwrap();
        assert_eq!(
            e.bits,
            vec![
                BitReq { row: 0, bit: 4, val: true },
                BitReq { row: 1, bit: 4, val: false },
                BitReq { row: 1, bit: 7, val: true },
            ]
        );
        assert_eq!(e.func, DbFunc::Buffer);
    }

    #[test]
    fn parses_lc_config() {
        let e = parse_line("B0[36],B1[36]\tLC_4").unwrap();
        assert_eq!(e.func, DbFunc::LcConfig(4));
    }

    #[test]
    fn ram_input_normalization_matches_icebox() {
        // icebox.py netname_normalize, ramb_8k/ramt_8k flavors:
        // inputN_M -> ram/RADDR_{[7,6,5,4,3,2,1,0,-1*5,10,9,8][N*4+M]}.
        assert_eq!(normalize_ram_input("input0_0", true), "ram/RADDR_7");
        assert_eq!(normalize_ram_input("input0_7", true), "ram/RADDR_0");
        assert_eq!(normalize_ram_input("input2_5", true), "ram/RADDR_10");
        assert_eq!(normalize_ram_input("input2_7", true), "ram/RADDR_8");
        assert_eq!(normalize_ram_input("input0_3", false), "ram/WADDR_4");
        assert_eq!(normalize_ram_input("input2_6", false), "ram/WADDR_9");
        // Non-input names pass through untouched.
        assert_eq!(normalize_ram_input("ram/RDATA_3", true), "ram/RDATA_3");
        assert_eq!(normalize_ram_input("local_g0_1", false), "local_g0_1");
    }

    #[test]
    fn normalization_matches_icebox() {
        assert_eq!(normalize_netname("wire_logic_cluster/lc_3/out"), "lutff_3/out");
        assert_eq!(normalize_netname("lc_trk_g1_2"), "local_g1_2");
        assert_eq!(normalize_netname("input_2_5"), "lutff_5/in_2");
        assert_eq!(normalize_netname("logic_op_lft_0"), "neigh_op_lft_0");
        assert_eq!(normalize_netname("lutff_7/clk"), "lutff_global/clk");
    }

    #[test]
    fn all_data_files_parse() {
        for (name, text) in [
            ("logic_8k", include_str!("../data/logic_8k.txt")),
            ("io_l", include_str!("../data/io_l.txt")),
            ("io_r", include_str!("../data/io_r.txt")),
            ("io_t", include_str!("../data/io_t.txt")),
            ("io_b", include_str!("../data/io_b.txt")),
            ("ramb_8k", include_str!("../data/ramb_8k.txt")),
            ("ramt_8k", include_str!("../data/ramt_8k.txt")),
        ] {
            let entries = parse_db(text);
            assert!(!entries.is_empty(), "{name} parsed to zero entries");
        }
    }
}
