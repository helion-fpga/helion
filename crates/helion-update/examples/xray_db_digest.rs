//! Print the `Family` records of `helion_update::xray_db::FAMILIES` for a
//! Project X-Ray database checkout at the pinned commit:
//!
//!     cargo run -p helion-update --example xray_db_digest -- <db root> [family...]
//!
//! Run it on a `git` checkout of the pinned commit (not on a directory
//! Helion downloaded), and paste the output into `xray_db.rs`.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: xray_db_digest <db root> [family...]");
        std::process::exit(2);
    };
    let root = std::path::PathBuf::from(root);
    let mut names: Vec<String> = args.collect();
    if names.is_empty() {
        names = vec!["artix7".into(), "spartan7".into(), "zynq7".into()];
    }
    for name in names {
        match helion_update::xray_db::family_digest(&root, &name) {
            Ok((digest, files, bytes)) => println!("    Family {{ name: \"{name}\", files: {files}, bytes: {bytes}, digest: \"{digest}\" }},"),
            Err(e) => {
                eprintln!("{name}: {e}");
                std::process::exit(1);
            }
        }
    }
}
