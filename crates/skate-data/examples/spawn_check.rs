//! Report, for each map, whether its authored spawn and its teleport
//! destinations have ground below them, and where
//! `spawn_ground::grounded_spawn` would move each point that has none.
//!
//!     cargo run -p skate-data --example spawn_check -- [--teleports teleports.json] <map.skate> [...]
fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut teleports = Vec::new();
    if args.first().map(String::as_str) == Some("--teleports") {
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
        for d in json["destinations"].as_array().unwrap() {
            if let Some(m) = d["matrix"].as_array() {
                let t = &m[3];
                let p = [0, 1, 2].map(|i| t[i].as_f64().unwrap() as f32);
                teleports.push((d["map"].as_str().unwrap().to_owned(), d["id"].as_str().unwrap().to_owned(), p));
            }
        }
        args.drain(..2);
    }
    for path in args {
        let map = match skate_data::skate_map::SkateMap::load(std::path::Path::new(&path)) {
            Ok(map) => map,
            Err(e) => { println!("{path}: {e}"); continue; }
        };
        let points = std::iter::once(("spawn".to_owned(), map.spawn))
            .chain(teleports.iter().filter(|t| t.0 == map.name).map(|t| (t.1.clone(), t.2)));
        for (label, p) in points {
            match skate_data::spawn_ground::grounded_spawn(&map, p) {
                Ok(None) => println!("{:<20} {:<24} {:?} ok", map.name, label, p),
                Ok(Some(q)) => println!("{:<20} {:<24} {:?} NO GROUND -> {:?}", map.name, label, p, q),
                Err(e) => println!("{:<20} {:<24} error {e}", map.name, label),
            }
        }
    }
}
