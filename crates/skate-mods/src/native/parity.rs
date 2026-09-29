//! Each built-in port must emit exactly what its Lua original emits for the
//! same callbacks and snapshots.
use crate::{Manifest, vm::LuaVm};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

fn package(dir: &str) -> (PathBuf, Manifest, BTreeMap<String, Value>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk/examples").join(dir);
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(root.join("mod.json")).unwrap()).unwrap();
    let settings = manifest.settings.iter().map(|(k, s)| (k.clone(), s.default.clone())).collect();
    (root, manifest, settings)
}

fn bike(position: [f64; 3], heading: f64, speed: f64, airborne: bool, w: [f64; 3], phase: &str) -> Value {
    json!({"position": position, "rotation": [0, 0, 0, 1], "heading": heading, "speed": speed,
        "velocity": [heading.sin() * speed, if airborne { 3.0 } else { 0.0 }, heading.cos() * speed],
        "angular_velocity": w, "wheel_contacts": if airborne { 0 } else { 2 }, "airborne": airborne,
        "wheel_speed": speed, "phase": phase, "occupied": phase != "parked", "ready": true})
}

fn input(t: usize) -> Value {
    // A run: throttle, then tricks thrown in the air (trick_b + right stick),
    // tucked back before landing, an interact press and a reset press.
    let tricking = (70..95).contains(&t) || (160..200).contains(&t);
    json!({"pitch": 0.0, "throttle": 1.0, "steering": ((t as f64) * 0.1).sin() * 0.5,
        "brake": 0.0, "handbrake": false, "interact": t == 5 || t == 6, "pad_buttons": if t == 230 { 0x80 } else { 0 },
        "weight": if (40..50).contains(&t) { -1.0 } else { 0.0 }, "whip": if (120..130).contains(&t) { 1.0 } else { 0.0 },
        "lean": if tricking { 0.9 } else { 0.1 }, "rider_y": if t % 50 < 25 { 0.2 } else { -0.8 },
        "trigger_l": if (240..250).contains(&t) { 0.8 } else { 0.0 }, "trigger_r": if t < 240 { 0.9 } else { 0.0 },
        "trick_a": tricking && t < 100, "trick_b": tricking && t >= 100, "clutch": t % 90 == 0})
}

fn snapshot(t: usize, mod_id: &str) -> Value {
    let keys = if t == 1 || t == 2 { json!({"F9": true}) } else if t == 215 { json!({"KeyR": true}) } else { json!({}) };
    let airborne = (60..110).contains(&t) || (150..210).contains(&t);
    let heading = 0.3 + if (60..110).contains(&t) { (t as f64 - 60.) * 0.02 } else { 0.0 };
    let w = if airborne { [6.0, 2.5, 0.4] } else { [0.0; 3] };
    let vehicles = if t < 3 {
        json!({})
    } else {
        let phase = if t < 7 { "parked" } else { "driving" };
        let y = if t == 300 { -60.0 } else { 1.0 };
        json!({ mod_id: {"mx": bike([t as f64 * 0.4, y, 5.0], heading, if t > 260 && t < 290 { 0.1 } else { 9.0 }, airborne, w, phase)} })
    };
    json!({"player": {"position": [1.0, 0.0, 2.0], "velocity": [0, 0, 0], "heading": 0.7, "on_board": true, "state": 0,
            "category": 0, "bailing": false, "grind": {"active": false}},
        "vehicles": vehicles, "vehicle_input": input(t), "keys": keys, "actions": [], "map": {"name": "Test", "generation": 1},
        "tick": t, "paused": false, "replay": false, "network": null, "animation": null})
}

#[test]
fn freestyle_mx_port_matches_lua() {
    let (root, manifest, settings) = package("freestyle-mx");
    let mut lua = LuaVm::new(&root, &manifest, &settings, &snapshot(0, &manifest.id)).unwrap();
    let mut native = super::NativeVm::new(&manifest.id, &settings).unwrap();
    let mut compare = |name: &str, payload: Value, snap: &Value, lua: &mut LuaVm| {
        let a = lua.call(name, payload.clone(), snap).unwrap();
        let b = native.call(name, payload, snap).unwrap();
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "{name} at tick {}", snap["tick"]);
        a.len()
    };
    let mut total = compare("on_load", Value::Null, &snapshot(0, &manifest.id), &mut lua);
    for t in 1..320 {
        let snap = snapshot(t, &manifest.id);
        if t == 111 || t == 211 {
            total += compare("on_event", json!({"name": "vehicle_bailed", "key": "mx", "reason": if t == 111 { "landing" } else { "inverted" }}), &snap, &mut lua);
        }
        if t == 305 {
            total += compare("on_event", json!({"name": "world_changed", "map": snap["map"]}), &snap, &mut lua);
        }
        if t == 250 {
            total += compare("on_settings", json!({"key": "hud", "value": true}), &snap, &mut lua);
        }
        total += compare("on_fixed_update", json!({"dt": 1.0 / 60.0}), &snap, &mut lua);
    }
    assert!(total > 1000, "only {total} commands compared");
}
