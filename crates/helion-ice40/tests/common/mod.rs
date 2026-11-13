//! Shared helpers for the iCE40 integration tests: finding the Project
//! IceStorm tools and the icepack/iceunpack round trip.
//!
//! The tools are looked up through `HELION_<TOOL>` (e.g. `HELION_ICEPACK`)
//! and then `PATH`. A missing tool fails the test: a round trip that cannot
//! run proves nothing, so it must not pass quietly.

#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Path of an IceStorm tool (`icepack`, `iceunpack`, ...). Panics with an
/// install hint when it cannot be found.
pub fn tool(name: &str) -> PathBuf {
    let var = format!("HELION_{}", name.to_ascii_uppercase());
    if let Some(p) = std::env::var_os(&var) {
        let p = PathBuf::from(p);
        assert!(p.is_file(), "{var}={} is not a file", p.display());
        return p;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(name);
            if p.is_file() {
                return p;
            }
        }
    }
    panic!(
        "{name} not found on PATH and {var} is not set. These tests check Helion's .asc with \
         Project IceStorm and fail without it: install it (brew install icestorm, \
         apt install fpga-icestorm) or point {var} at the binary."
    );
}

/// A scratch directory unique to this test process and stem; removed on drop.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(stem: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("helion-ice40-{}-{stem}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(dir)
    }
    pub fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(tool_name: &str, args: &[&Path]) {
    let exe = tool(tool_name);
    let out = Command::new(&exe).args(args).output().unwrap_or_else(|e| panic!("{} runs: {e}", exe.display()));
    assert!(
        out.status.success(),
        "{tool_name} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// icepack -> iceunpack round trip. Returns the re-emitted `.asc` text and
/// asserts that no set bit was lost or gained (iceunpack normalises the
/// text: it adds a `.comment`, which also lands in the `.bin` header, and
/// lists every tile and RAM, so the comparison is on set bits, RAM contents
/// and extra bits).
pub fn round_trip(asc: &str, stem: &str) -> String {
    let s = Scratch::new(stem);
    let (a, b, a2, b2, a3) = (s.path("in.asc"), s.path("in.bin"), s.path("rt.asc"), s.path("rt.bin"), s.path("rt2.asc"));
    std::fs::write(&a, asc).expect("write asc");
    run("icepack", &[&a, &b]);
    run("iceunpack", &[&b, &a2]);
    let rt = std::fs::read_to_string(&a2).expect("read round-tripped asc");
    assert_eq!(set_bits(asc), set_bits(&rt), "icepack/iceunpack round trip lost or gained bits ({stem})");
    // The unpacked text is a fixed point of pack/unpack.
    run("icepack", &[&a2, &b2]);
    run("iceunpack", &[&b2, &a3]);
    assert_eq!(rt, std::fs::read_to_string(&a3).expect("read asc"), "unpacked .asc is not a pack/unpack fixed point ({stem})");
    rt
}

/// Every set bit of an `.asc`: tile bits as `(section, x, y, row, col)`,
/// RAM contents as `(".ram_data", x, y, line, hex digit)` for non-zero
/// digits, and `.extra_bit` lines.
pub fn set_bits(asc: &str) -> HashSet<(String, u32, u32, usize, usize)> {
    let mut set = HashSet::new();
    let mut kind = String::new();
    let (mut x, mut y, mut row) = (0u32, 0u32, 0usize);
    for line in asc.lines() {
        if let Some(rest) = line.strip_prefix('.') {
            let mut it = rest.split_whitespace();
            kind = it.next().unwrap_or("").to_string();
            let a: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let b: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            if kind == "extra_bit" {
                let c: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                set.insert((kind.clone(), a, b, c, 0));
            }
            x = a;
            y = b;
            row = 0;
        } else if kind == "ram_data" {
            for (i, c) in line.chars().enumerate() {
                if c != '0' {
                    set.insert((kind.clone(), x, y, row, i * 16 + c.to_digit(16).unwrap_or(99) as usize));
                }
            }
            row += 1;
        } else if kind.ends_with("_tile") && !line.is_empty() && line.chars().all(|c| c == '0' || c == '1') {
            for (i, c) in line.chars().enumerate() {
                if c == '1' {
                    set.insert((kind.clone(), x, y, row, i));
                }
            }
            row += 1;
        }
    }
    set
}
