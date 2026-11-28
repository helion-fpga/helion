// scratch: dump router node ids for every tile-db name (not committed)
use helion_ice40::device::{tile_db, tile_kind};
use helion_ice40::route::Router;
fn main() {
    let r = Router::build();
    for y in 0..=33u8 { for x in 0..=33u8 {
        let Some(k) = tile_kind(x, y) else { continue };
        let mut seen = std::collections::HashSet::new();
        for e in tile_db(k) {
            if !matches!(e.func, helion_ice40::db::DbFunc::Buffer | helion_ice40::db::DbFunc::Routing) { continue; }
            for n in [&e.src, &e.dst] {
                if seen.insert(n.clone()) {
                    if let Some(id) = r.lookup(x, y, n) { println!("{id} {x} {y} {n}"); }
                }
            }
        }
    }}
}
