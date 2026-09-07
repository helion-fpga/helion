//! Public GitHub Pages HTML (`docs/*.html`) must document the app.
//! README.md and FM-HEL-*.md are not the site.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn public_html(docs: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(docs).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("html") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let body = fs::read_to_string(&path).unwrap();
        out.push((name, body));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        !out.is_empty(),
        "no docs/*.html — this test must read the shipped site"
    );
    out
}

#[test]
fn public_html_documents_the_app() {
    let docs = root().join("docs");
    let pages = public_html(&docs);
    let joined: String = pages.iter().map(|(_, b)| b.as_str()).collect();
    let home = pages
        .iter()
        .find(|(n, _)| n == "index.html")
        .map(|(_, b)| b.as_str())
        .expect("docs/index.html");

    for (name, body) in &pages {
        assert!(
            !body.contains("role=\"marquee\""),
            "{name} still has role=marquee"
        );
        assert!(
            !body.contains("class=\"marquee\"") && !body.contains("class='marquee'"),
            "{name} still has class=marquee"
        );
        assert!(
            !body.contains("marquee-track"),
            "{name} still has marquee-track"
        );
        assert!(
            !body.contains('\u{2014}'),
            "{name} contains U+2014 em dash"
        );
    }

    let home_l = home.to_ascii_lowercase();
    assert!(
        home_l.contains("original") && home_l.contains("fpga") && home_l.contains("family"),
        "home must state an original FPGA family"
    );
    assert!(
        home_l.contains("cad")
            && (home_l.contains("synth")
                || home_l.contains("pack")
                || home_l.contains("place")
                || home_l.contains("pathfinder")
                || home_l.contains("route")),
        "home must name the in-repo CAD (synth / pack / place / route / PathFinder)"
    );
    assert!(
        home_l.contains("ide") || home.contains("helion-ide"),
        "home must name the desktop IDE"
    );
    assert!(
        home_l.contains("not a wrapper")
            || home_l.contains("not a vendor")
            || (home_l.contains("vendor") && home_l.contains("bitstream") && home_l.contains("no ")),
        "home must say Helion is not a vendor-tool wrapper"
    );

    assert!(
        joined.contains("helion-ide") && joined.contains("helion"),
        "site must name helion-ide and the helion CLI"
    );
    assert!(
        joined.contains("HL10T-C32-1") || joined.contains("HAD"),
        "site must name the HAD part"
    );
    assert!(
        joined.contains("WNS_PS=9640"),
        "site must publish gold WNS_PS=9640"
    );
    assert!(
        joined.contains("0000000111111110"),
        "site must publish gold LED waveform 0000000111111110"
    );
    let legal = joined.to_ascii_lowercase();
    assert!(
        legal.contains("apache-2.0")
            && legal.contains("mit")
            && (legal.contains("unisim")
                || legal.contains("project x-ray")
                || legal.contains("vendor bitstream")
                || legal.contains("had-only")
                || legal.contains("device facts")),
        "site must carry the legal fence (license + no vendor bitstream / HAD-only facts)"
    );
}
