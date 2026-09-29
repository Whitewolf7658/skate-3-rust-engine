//! Freestyle MX, line for line from `sdk/examples/freestyle-mx/main.lua`.
//! Left stick is the bike, right stick is the rider. The host owns the physics;
//! this owns the control scheme, the trick roster, the scoring and the recovery.
use super::{Context, Script};
use crate::Command;
use serde_json::Value;
use std::collections::BTreeMap;
use std::f64::consts::PI;

const KEY: &str = "mx";
// Each trick is one authored rider pose, blended in by how far the stick is
// pushed. `base` is what it is worth held to full extension.
const TRICKS: [(&str, f64); 8] = [
    ("Superman", 300.),
    ("Lazy Boy", 280.),
    ("Nac-Nac", 220.),
    ("Can-Can", 180.),
    ("Heel Clicker", 320.),
    ("Kiss of Death", 420.),
    ("Cordova", 400.),
    ("No-Hander", 150.),
];
// Extension must be under this at touchdown or the landing is cased.
const TUCKED: f64 = 0.35;
// A trick has to be out this long to count, so a flicked stick scores nothing.
const MIN_HELD: f64 = 0.15;
// Yaw away from the line of travel that counts as a whip, and how square the
// bike has to be by touchdown for it to have been brought back.
const WHIP_ANGLE: f64 = 0.55;
const WHIP_HOME: f64 = 0.4;
// Throttle held this long against a bike that will not move means stuck.
const STUCK_SECONDS: f64 = 1.5;
// How long after a bail before the rider is put back on, and how long to keep
// trying: the native skater has to finish its ragdoll before it can remount.
const REMOUNT_DELAY: f64 = 1.6;
const REMOUNT_WINDOW: f64 = 6.0;

struct Air {
    seconds: f64,
    spin: f64,
    flip: f64,
    whip: f64,
    tricks: BTreeMap<u32, f64>,
    best_trick: Option<u32>,
    best_held: f64,
    // The line the bike left the lip on. A whip is measured against where it
    // is going, not against where it was pointing when it took off.
    course: f64,
}

/// The fields of `sdk.vehicle.read('mx')` this script uses.
struct Bike {
    position: [f64; 3],
    heading: f64,
    speed: f64,
    velocity: [f64; 3],
    angular_velocity: [f64; 3],
    airborne: bool,
    occupied: bool,
    ready: bool,
    driving: bool,
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.)
}
fn vec3(v: &Value) -> [f64; 3] {
    [num(&v[0]), num(&v[1]), num(&v[2])]
}
fn point(p: [f64; 3]) -> [f32; 3] {
    p.map(|v| v as f32)
}

impl Bike {
    fn read(ctx: &Context) -> Option<Self> {
        let b = ctx.snapshot.get("vehicles")?.get(ctx.mod_id)?.get(KEY)?;
        Some(Self {
            position: vec3(&b["position"]),
            heading: num(&b["heading"]),
            speed: num(&b["speed"]),
            velocity: vec3(&b["velocity"]),
            angular_velocity: vec3(&b["angular_velocity"]),
            airborne: b["airborne"].as_bool().unwrap_or(false),
            occupied: b["occupied"].as_bool().unwrap_or(false),
            ready: b["ready"].as_bool().unwrap_or(false),
            driving: b["phase"] == "driving",
        })
    }
}

// Signed angle from a to b, wrapped to -pi..pi. Heading crosses +/-pi in the
// middle of an ordinary corner, and an unwrapped difference reads that as most
// of a rotation.
fn angle_between(a: f64, b: f64) -> f64 {
    let d = (b - a).rem_euclid(2. * PI);
    if d > PI { d - 2. * PI } else { d }
}

// Modifier plus a right-stick octant picks the trick. Holding no modifier, or
// leaving the stick centred, means the rider is riding rather than tricking.
fn selected(i: &Value) -> u32 {
    let (a, b) = (flag(&i["trick_a"]), flag(&i["trick_b"]));
    if !(a || b) {
        return 0;
    }
    let (x, y) = (num(&i["lean"]), num(&i["rider_y"]));
    if x.abs() < 0.4 && y.abs() < 0.4 {
        return 0;
    }
    let dir = if y.abs() > x.abs() {
        if y > 0. { 1 } else { 2 }
    } else if x > 0. {
        3
    } else {
        4
    };
    if b { 4 + dir } else { dir }
}

fn flag(v: &Value) -> bool {
    v.as_bool().unwrap_or(false)
}

#[derive(Default)]
pub(super) struct FreestyleMx {
    previous: BTreeMap<&'static str, bool>,
    interact_was_down: bool,
    reset_was_down: bool,
    trick: u32,
    extend: f64,
    air: Option<Air>,
    total: i64,
    combo: i64,
    best: i64,
    banner: String,
    banner_time: f64,
    stuck_for: f64,
    clock: f64,
    remount_until: f64,
    remount_at: Option<f64>,
    safe_spot: Option<[f64; 3]>,
    safe_heading: f64,
    out: Vec<Command>,
}

impl FreestyleMx {
    fn pressed(&mut self, ctx: &Context, key: &'static str) -> bool {
        let down = ctx.snapshot["keys"][key] == true;
        let edge = down && !self.previous.get(key).copied().unwrap_or(false);
        self.previous.insert(key, down);
        edge
    }
    fn setting(ctx: &Context, key: &str) -> Value {
        ctx.settings.get(key).cloned().unwrap_or(Value::Null)
    }
    fn tune(&mut self, ctx: &Context) {
        let pairs = [
            ("engine_force", "engine"),
            ("max_speed", "speed"),
            ("tire_grip", "tire_friction"),
            ("lean_max", "lean_max"),
            ("lean_rate", "lean_rate"),
            ("lean_expo", "lean_expo"),
            ("counter_steer", "counter_steer"),
            ("lean_yaw", "lean_yaw"),
            ("berm_assist", "berm_assist"),
            ("steer_rate", "steer_rate"),
            ("grip_assist", "grip_assist"),
            ("step_assist", "step_assist"),
            ("preload_release", "preload_release"),
            ("air_yaw", "air_yaw"),
            ("whip_rate", "whip_rate"),
            ("flip_rate", "flip_rate"),
            ("engine_volume", "engine_volume"),
        ];
        // A setting Lua reads as nil is simply absent from the table it sends.
        let tuning: serde_json::Map<_, _> = pairs
            .iter()
            .filter_map(|(field, setting)| {
                let v = Self::setting(ctx, setting);
                (!v.is_null()).then(|| (field.to_string(), v))
            })
            .collect();
        match serde_json::from_value(Value::Object(tuning)) {
            Ok(tuning) => self.out.push(Command::VehicleTune { key: KEY.into(), tuning }),
            Err(e) => self.out.push(Command::Log { text: format!("tune: {e}") }),
        }
    }
    fn say(&mut self, text: impl Into<String>) {
        self.banner = text.into();
        self.banner_time = 2.5;
    }
    fn nearby(ctx: &Context) -> ([f64; 3], f64) {
        let p = &ctx.snapshot["player"];
        let h = num(&p["heading"]);
        let pos = vec3(&p["position"]);
        ([pos[0] + h.sin() * 3., pos[1] + 1., pos[2] + h.cos() * 3.], h)
    }
    fn reset(&mut self, position: [f64; 3], heading: f64) {
        self.out.push(Command::VehicleReset {
            key: KEY.into(),
            position: point(position),
            heading: heading as f32,
        });
    }
    fn spawn(&mut self, ctx: &Context) {
        let bike = Bike::read(ctx);
        if let Some(bike) = bike.as_ref().filter(|b| b.occupied) {
            let p = bike.position;
            self.reset([p[0], p[1] + 1., p[2]], bike.heading);
            return;
        }
        if bike.is_some() {
            self.out.push(Command::VehicleRemove { key: KEY.into() });
        }
        let (p, h) = Self::nearby(ctx);
        self.out.push(Command::VehicleSpawn {
            key: KEY.into(),
            definition: "vehicle.json".into(),
            position: point(p),
            heading: h as f32,
        });
        self.tune(ctx);
    }
    // Put the bike back on its wheels where it is, and forget the run. Used by
    // the reset button, by the stuck detector and by the remount after a bail.
    fn recover(&mut self, position: [f64; 3], heading: f64) {
        self.reset(position, heading);
        self.trick = 0;
        self.extend = 0.;
        self.air = None;
        self.combo = 0;
        self.stuck_for = 0.;
    }

    fn land(&mut self, bike: &Bike) {
        let Some(air) = self.air.take() else { return };
        if air.seconds < 0.3 {
            return;
        }
        if self.extend > TUCKED {
            // Still hanging off the bike at touchdown. This is the rule that
            // makes the trick a decision rather than a free bonus.
            self.combo = 0;
            self.say("CASED - tuck in before you land");
            return;
        }
        let mut score = 0.;
        let mut label: Option<String> = None;
        for (id, seconds) in &air.tricks {
            if *seconds >= MIN_HELD {
                score += TRICKS[*id as usize - 1].1 * seconds.min(1.5) / 1.5;
            }
        }
        // Each quarter turn of yaw, and each full flip.
        let spins = (air.spin.abs() / (PI / 2.)).floor() as i64;
        let flips = (air.flip / (PI * 2.)).floor() as i64;
        score += spins as f64 * 100. + flips as f64 * 500.;
        // A whip is thrown sideways and brought back square. Sent and not
        // brought back is not a whip, it is a case, and the host has already
        // said so by bailing the rider before this ever runs.
        let squared = angle_between(bike.heading, air.course).abs() < WHIP_HOME;
        if air.whip > WHIP_ANGLE && squared && flips == 0 {
            score += 200. + ((air.whip - WHIP_ANGLE) * 260.).floor();
            label = Some(if air.whip > 1.2 { "Big Whip" } else { "Whip" }.into());
        }
        score *= 1. + air.seconds.min(4.) / 4.;
        if score >= 1. {
            self.combo += 1;
            let banked = (score * self.combo as f64).floor() as i64;
            self.total += banked;
            if banked > self.best {
                self.best = banked;
            }
            let mut label = air
                .best_trick
                .map(|t| TRICKS[t as usize - 1].0.to_owned())
                .or(label)
                .unwrap_or_else(|| "Air".into());
            if flips > 0 {
                label = format!("{label} + {flips} flip");
            }
            if spins > 0 {
                label = format!("{label} + {}\u{b0}", spins * 90);
            }
            self.say(format!("{label}  {banked}  x{}", self.combo));
        }
    }

    fn score_air(&mut self, ctx: &Context, bike: &Bike, dt: f64) {
        if !flag(&Self::setting(ctx, "scoring")) {
            self.air = None;
            return;
        }
        if bike.airborne {
            let air = self.air.get_or_insert_with(|| {
                let v = bike.velocity;
                let flat = (v[0] * v[0] + v[2] * v[2]).sqrt();
                Air {
                    seconds: 0.,
                    spin: 0.,
                    flip: 0.,
                    whip: 0.,
                    tricks: BTreeMap::new(),
                    best_trick: None,
                    best_held: 0.,
                    course: if flat > 1. { v[0].atan2(v[2]) } else { bike.heading },
                }
            });
            air.seconds += dt;
            let w = bike.angular_velocity;
            air.spin += w[1] * dt;
            air.flip += (w[0] * w[0] + w[2] * w[2]).sqrt() * dt;
            air.whip = air.whip.max(angle_between(bike.heading, air.course).abs());
            if self.trick != 0 && self.extend > 0.5 {
                let held = air.tricks.get(&self.trick).copied().unwrap_or(0.) + dt;
                air.tricks.insert(self.trick, held);
                if held > air.best_held {
                    air.best_held = held;
                    air.best_trick = Some(self.trick);
                }
            }
        } else if self.air.is_some() {
            self.land(bike);
        }
    }

    // A bike wedged against geometry, or one that has fallen out of the world,
    // is the one failure the rider cannot ride out of. Both put it back on its
    // wheels at the last place it was actually moving.
    fn unstick(&mut self, ctx: &Context, bike: &Bike, i: &Value, dt: f64) -> bool {
        if bike.position[1] < -50. {
            let p = self.safe_spot.unwrap_or_else(|| Self::nearby(ctx).0);
            self.recover([p[0], p[1] + 1., p[2]], self.safe_heading);
            self.say("OUT OF THE WORLD - put back");
            return true;
        }
        let trying = num(&i["trigger_r"]) > 0.5 || num(&i["trigger_l"]) > 0.5;
        if trying && bike.speed.abs() < 0.5 && !bike.airborne {
            self.stuck_for += dt;
        } else {
            self.stuck_for = 0.;
        }
        if self.stuck_for > STUCK_SECONDS {
            let p = bike.position;
            self.recover([p[0], p[1] + 0.6, p[2]], bike.heading);
            self.say("UNSTUCK");
            return true;
        }
        if !bike.airborne && bike.speed.abs() > 2. {
            self.safe_spot = Some(bike.position);
            self.safe_heading = bike.heading;
        }
        false
    }

    // Two lines and a banner. The control list lives in the mod window, where
    // it can be read once rather than sat on screen for the whole session.
    fn hud(&mut self, ctx: &Context, bike: Option<&Bike>) {
        let lines = match bike {
            None => vec!["FREESTYLE MX | F9 spawns the bike".to_owned()],
            Some(bike) => {
                let name = (self.trick != 0).then(|| {
                    format!("{} {:.0}%", TRICKS[self.trick as usize - 1].0, self.extend * 100.)
                });
                let mut lines = vec![
                    format!(
                        "{:.0} km/h{}",
                        bike.speed.abs() * 3.6,
                        if bike.airborne { "  AIR" } else { "" }
                    ),
                    format!(
                        "SCORE {}   x{}   best {}{}",
                        self.total,
                        self.combo,
                        self.best,
                        name.map(|n| format!("   {n}")).unwrap_or_default()
                    ),
                ];
                if !bike.ready {
                    lines[0] = "loading the bike...".into();
                }
                if !self.banner.is_empty() {
                    lines.push(self.banner.clone());
                }
                lines
            }
        };
        let show = flag(&Self::setting(ctx, "hud"));
        for i in 1..=7 {
            let key = format!("mx-hud-{i}");
            match lines.get(i - 1).filter(|_| show) {
                Some(text) => self.out.push(Command::Overlay { key, text: text.clone() }),
                None => self.out.push(Command::Remove { key }),
            }
        }
    }

    fn fixed_update(&mut self, ctx: &Context, dt: f64) {
        self.clock += dt;
        if self.pressed(ctx, "F9") {
            self.spawn(ctx);
        }
        let Some(bike) = Bike::read(ctx) else {
            self.hud(ctx, None);
            return;
        };

        // Getting back on after a crash. The native skater has to finish its
        // ragdoll before the host will let it mount, and the host silently
        // refuses an enter it cannot honour, so this keeps asking for a few
        // seconds rather than assuming the first attempt took.
        if self.remount_at.is_some_and(|at| self.clock >= at) {
            let p = self.safe_spot.unwrap_or(bike.position);
            let b = bike.position;
            self.recover([b[0], b[1] + 0.6, b[2]], self.safe_heading);
            let h = self.safe_heading;
            self.out.push(Command::Teleport {
                position: point([p[0] + h.cos() * 1.2, p[1] + 0.2, p[2] - h.sin() * 1.2]),
                heading: h as f32,
                on_board: false,
            });
            self.remount_at = None;
            self.remount_until = self.clock + REMOUNT_WINDOW;
        }
        if self.remount_until > self.clock {
            if bike.occupied {
                self.remount_until = 0.;
            } else if flag(&Self::setting(ctx, "remount")) {
                self.out.push(Command::VehicleEnter { key: KEY.into() });
            }
        }

        let i = &ctx.snapshot["vehicle_input"];
        let interact = flag(&i["interact"]);
        if interact && !self.interact_was_down {
            self.out.push(if bike.occupied {
                Command::VehicleExit { key: KEY.into() }
            } else {
                Command::VehicleEnter { key: KEY.into() }
            });
            self.remount_until = 0.;
        }
        self.interact_was_down = interact;
        let reset_down = bike.occupied && i["pad_buttons"].as_u64().unwrap_or(0) & 0x80 != 0;
        if self.pressed(ctx, "KeyR") || (reset_down && !self.reset_was_down) {
            let (mut p, mut h) = Self::nearby(ctx);
            if bike.occupied {
                let b = bike.position;
                p = [b[0], b[1] + 1., b[2]];
                h = bike.heading;
            }
            self.recover(p, h);
        }
        self.reset_was_down = reset_down;

        if bike.driving && !self.unstick(ctx, &bike, i, dt) {
            let want = selected(i);
            if want != 0 {
                self.trick = want;
            }
            // Extension eases out and snaps back, so a half-thrown trick reads
            // as a half-thrown trick rather than an on/off pose swap.
            if want != 0 {
                self.extend = (self.extend + dt * 4.).min(1.);
            } else {
                self.extend = (self.extend - dt * 5.).max(0.);
                if self.extend == 0. {
                    self.trick = 0;
                }
            }
            // Throwing the rider off the bike means you are not steering it.
            // The stick does one job at a time.
            let tricking = self.extend > 0.05;
            // The left trigger brakes, and once you are stopped it backs you
            // up. The host reads reverse as negative throttle.
            let lt = num(&i["trigger_l"]);
            let reversing = bike.speed.abs() < 1.2 && lt > 0.25;
            let controls = skate_vehicles::Controls {
                throttle: (if reversing { -lt } else { num(&i["trigger_r"]) }) as f32,
                brake: (if reversing { 0. } else { lt }) as f32,
                steering: num(&i["steering"]) as f32,
                weight: num(&i["weight"]) as f32,
                whip: num(&i["whip"]) as f32,
                lean: (if tricking { 0. } else { num(&i["lean"]) }) as f32,
                handbrake: flag(&i["handbrake"]),
                clutch: flag(&i["clutch"]),
                trick: self.trick,
                trick_extend: self.extend as f32,
                ..Default::default()
            };
            self.out.push(Command::VehicleControl { key: KEY.into(), controls });
            self.score_air(ctx, &bike, dt);
        }
        if self.banner_time > 0. {
            self.banner_time -= dt;
            if self.banner_time <= 0. {
                self.banner.clear();
            }
        }
        self.hud(ctx, Some(&bike));
    }
}

impl Script for FreestyleMx {
    fn call(&mut self, name: &str, payload: &Value, ctx: &Context) -> Result<Vec<Command>, String> {
        match name {
            "on_load" => self.hud(ctx, None),
            "on_fixed_update" => self.fixed_update(ctx, num(&payload["dt"])),
            "on_settings" => {
                let bike = Bike::read(ctx);
                if bike.is_some() {
                    self.tune(ctx);
                }
                self.hud(ctx, bike.as_ref());
            }
            "on_event" if payload["name"] == "world_changed" => {
                self.previous.clear();
                self.interact_was_down = false;
                self.reset_was_down = false;
                self.trick = 0;
                self.extend = 0.;
                self.air = None;
                self.combo = 0;
                self.remount_at = None;
                self.remount_until = 0.;
                self.safe_spot = None;
                self.hud(ctx, None);
            }
            "on_event" if payload["name"] == "vehicle_bailed" && payload["key"] == KEY => {
                self.air = None;
                self.trick = 0;
                self.extend = 0.;
                self.combo = 0;
                self.say(match payload["reason"].as_str() {
                    Some("landing") => "CASED IT",
                    Some("inverted") => "TIPPED OVER",
                    _ => "CRASH",
                });
                if flag(&Self::setting(ctx, "remount")) {
                    self.remount_at = Some(self.clock + REMOUNT_DELAY);
                }
            }
            _ => {}
        }
        Ok(std::mem::take(&mut self.out))
    }
}
