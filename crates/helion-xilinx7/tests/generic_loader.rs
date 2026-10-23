//! Every die the X-Ray DB ships should be loadable by name, not just the two
//! that had hand-written loader functions.
use helion_xilinx7::Device;

fn db() -> std::path::PathBuf {
    std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../work/prjxray-db")
                .canonicalize()
                .expect("set HELION_XILINX_DB")
        })
}

#[test]
fn every_die_in_the_db_loads() {
    let root = db();
    let mut dies = Vec::new();
    for fam in ["artix7", "zynq7"] {
        let d = root.join(fam);
        if !d.is_dir() {
            continue;
        }
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            // A die dir is a bare device name; a part dir has a package suffix.
            if name.starts_with("xc7") && !name.contains("clg") && !name.contains("csg")
                && !name.contains("fgg") && !name.contains("fbg") && !name.contains("ffg")
                && !name.contains("sbg") {
                dies.push(name);
            }
        }
    }
    dies.sort();
    dies.dedup();
    assert!(dies.len() >= 5, "expected several dies, got {dies:?}");

    let mut loaded = Vec::new();
    let mut failed = Vec::new();
    for die in &dies {
        match Device::load_part_from_db(&root, die) {
            Ok(dev) => {
                println!(
                    "OK   {die}: {} tiles, idcode {:#010x}",
                    dev.tile_count(),
                    dev.part().idcode
                );
                loaded.push(die.clone());
            }
            Err(e) => {
                println!("FAIL {die}: {e}");
                failed.push((die.clone(), e));
            }
        }
    }
    println!("loaded {}/{} dies", loaded.len(), dies.len());
    assert!(
        failed.is_empty(),
        "these dies failed to load: {:?}",
        failed.iter().map(|(d, _)| d).collect::<Vec<_>>()
    );
}

#[test]
fn generic_loader_matches_the_hand_written_loaders() {
    let root = db();
    let a = Device::load_part_from_db(&root, "xc7a35t").expect("generic xc7a35t");
    let b = Device::load_xc7a35t(&root).expect("hand xc7a35t");
    assert_eq!(a.tile_count(), b.tile_count());
    assert_eq!(a.part().idcode, b.part().idcode);

    let c = Device::load_part_from_db(&root, "xc7z020clg484-1").expect("generic z020");
    let d = Device::load_xc7z020(&root).expect("hand z020");
    assert_eq!(c.tile_count(), d.tile_count());
    assert_eq!(c.part().idcode, d.part().idcode);
    assert_eq!(c.part().idcode, 0x0372_7093);
}

#[test]
fn unknown_part_is_a_clear_error() {
    let e = Device::load_part_from_db(&db(), "xc9zzz999").unwrap_err();
    assert!(e.contains("no X-Ray database"), "got: {e}");
}
