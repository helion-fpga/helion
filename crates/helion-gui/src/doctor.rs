//! Headless `--version` / `--doctor` for the IDE binary.
//! No window, no GPU: the same strings the Mac user prints to sanity-check a build.

use helion_device::Device;
use std::fmt::Write;
use std::process::Command;

pub fn version_line() -> String {
    format!("helion-ide {}", env!("CARGO_PKG_VERSION"))
}

/// Compile-time target triple from cargo (`TARGET`). `aarch64-apple-darwin` on
/// the Mac build; whatever this host is when the Linux binary is built.
pub fn target_triple() -> &'static str {
    env!("HELION_TARGET")
}

/// Toolchain + HAD + target triple, one block the user can paste.
pub fn doctor_report() -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}", version_line());
    let _ = writeln!(s, "target {triple}", triple = target_triple());
    let _ = writeln!(
        s,
        "host {arch}-{os}",
        arch = std::env::consts::ARCH,
        os = std::env::consts::OS
    );
    match Command::new("rustc").arg("-vV").output() {
        Ok(o) if o.status.success() => {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if line.starts_with("release:")
                    || line.starts_with("host:")
                    || line.starts_with("llvm:")
                {
                    let _ = writeln!(s, "rustc {line}");
                }
            }
        }
        Ok(o) => {
            let _ = writeln!(
                s,
                "rustc not usable: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
        Err(e) => {
            let _ = writeln!(s, "rustc not on PATH: {e}");
        }
    }
    let _ = writeln!(s, "HAD path {}", Device::devices_dir().display());
    match Device::load_part("HL10T-C32-1") {
        Ok(dev) => {
            let _ = writeln!(
                s,
                "HAD {} idcode {:#010x} LUT6={} BRAM18={} DSP={}",
                dev.part,
                dev.idcode,
                dev.lut6_count(),
                dev.n_bram,
                dev.n_dsp
            );
            let _ = writeln!(s, "  {}", dev.report_die());
            for line in dev.report_featuremap().lines() {
                let _ = writeln!(s, "  {line}");
            }
        }
        Err(e) => {
            let _ = writeln!(s, "HAD load failed: {e}");
        }
    }
    let _ = writeln!(s, "ok");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_prints_toolchain_had_and_triple() {
        let d = doctor_report();
        assert!(d.starts_with("helion-ide 1.0.0"), "{d}");
        assert!(d.contains("target "), "{d}");
        assert!(d.contains("rustc release:") || d.contains("rustc not"), "{d}");
        assert!(
            d.contains("HAD HL10T-C32-1"),
            "doctor must load the HAD part: {d}"
        );
        assert!(
            d.contains("0x00011a1f") || d.contains("0x00011A1F"),
            "doctor must print the HAD idcode: {d}"
        );
        assert!(d.contains("LUT6=8192"), "{d}");
        assert!(d.contains("HAD path"), "{d}");
        assert!(d.contains("featuremap part=HL10T-C32-1"), "{d}");
        assert!(d.contains("BLE0.LUT.INIT[0] minor 0 bit 0"), "{d}");
        assert!(!d.contains("MISSING"), "{d}");
        assert!(d.contains(env!("HELION_TARGET")), "{d}");
        assert!(d.trim_end().ends_with("ok"), "{d}");
        // This Linux VM is not aarch64-apple-darwin; the Mac triple is a compile-time cfg.
        #[cfg(target_os = "macos")]
        {
            assert!(
                d.contains("aarch64-apple-darwin") || d.contains("x86_64-apple-darwin"),
                "{d}"
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(
                !d.contains("aarch64-apple-darwin"),
                "Linux build must not lie about being a Mac binary: {d}"
            );
        }
    }
}
