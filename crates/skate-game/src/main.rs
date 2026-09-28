mod fps_overlay;
mod animation;
mod crash_report;
mod crash_context;
mod multiplayer;
mod apt_vm;
mod apt_display;
mod apt_movie;
mod apt_text;
mod apt_scene;
mod hud_runtime;
mod scoring_runtime;
mod scoring_hud;
mod animation_pose;
mod app;
mod skate_audio;
mod assets;
mod camera;
mod config;
mod setup;
mod updater;
mod map_library;
mod map_render;
mod map_transition;
mod difficulty;
mod graph_host;
mod graph_runtime;
mod input;
mod session_marker;
mod physics;
mod skater_animation;
mod verification;
mod performance;
mod profiling;
mod graphics_menu;
mod modding;
mod customiser;
mod customiser_parts;
mod customiser_material;
mod custom_models;
mod teleport_menu;
mod render_capacity;
mod retail_render;
mod retail_character;
mod retail_irradiance;
mod presentation;
mod replay;
mod world;
mod grind_world;
mod skate_world;
#[cfg(target_arch = "wasm32")]
mod web;

/// Startup failures: stderr natively, console + page message on the web.
fn report(message: impl std::fmt::Display) {
    #[cfg(target_arch = "wasm32")]
    web::report(&message.to_string());
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{message}");
}

fn main() -> bevy::app::AppExit {
    #[cfg(target_arch = "wasm32")]
    let args = match web::install() {
        Ok(args) => args,
        Err(error) => { report(error); return bevy::app::AppExit::error(); }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    #[cfg(not(target_arch = "wasm32"))]
    {
        match updater::recover() {
            Ok(true) => return bevy::app::AppExit::Success,
            Err(error) => { report(error); return bevy::app::AppExit::Success; },
            Ok(false) => {}
        }
        if let Some(code) = crash_report::entry() { std::process::exit(code); }
    }
    // Web builds log through Bevy's LogPlugin (browser console) instead.
    #[cfg(not(target_arch = "wasm32"))]
    let _trace = match profiling::init() {
        Ok(guard) => guard,
        Err(error) => { report(error); return bevy::app::AppExit::error(); }
    };
    let _startup = bevy::log::info_span!("startup").entered();
    eprintln!("REPORT_META stage=configuration_and_installation");
    let config = match bevy::log::info_span!("load_configuration_and_map").in_scope(|| config::Config::from_args(args)) {
        Ok(config) => config,
        Err(error) => {
            report(error);
            return bevy::app::AppExit::error();
        }
    };
    profiling::map_metadata(&config);
    eprintln!("REPORT_META startup=map_fingerprint:{:016x} difficulty:{} multiplayer_requested:{} custom_appearance:{} renderer:{}", config.map_fingerprint, config.difficulty.key(), config.multiplayer.host.is_some() || config.multiplayer.direct.is_some(), config.multiplayer.appearance.is_some(), if cfg!(target_arch = "wasm32") {"WebGPU"} else {"Vulkan"});
    eprintln!("REPORT_META stage=gameplay_configuration");
    if let Err(error) = skate_data::input_config::StockGameplayConfig::load(&config.asset_root) {
        report(error);
        return bevy::app::AppExit::error();
    }
    eprintln!("REPORT_META stage=asset_manifest");
    let manifest = match bevy::log::info_span!("load_manifest").in_scope(|| skate_data::GameAssets::load(&config.asset_root)) {
        Ok(manifest) => manifest,
        Err(error) => {
            report(error);
            return bevy::app::AppExit::error();
        }
    };
    eprintln!("REPORT_META stage=stock_graphs");
    let graphs = match bevy::log::info_span!("load_graphs").in_scope(|| graph_runtime::StockGraphs::load(&config.asset_root, &manifest)) {
        Ok(graphs) => graphs,
        Err(error) => {
            report(error);
            return bevy::app::AppExit::error();
        }
    };
    eprintln!("REPORT_META stage=map_validation");
    if let Some(map) = &config.map {
        if let Err(error) = skate_world::validate_runtime(map) {
            report(error);
            return bevy::app::AppExit::error();
        }
    }
    eprintln!("SKATE_DIFFICULTY mode={} native_index={}", config.difficulty.key(), config.difficulty as u32);
    eprintln!("REPORT_META stage=physics_initialization");
    let mut physics = match bevy::log::info_span!("load_physics").in_scope(|| physics::GamePhysics::load_with_difficulty(&config.asset_root, config.map.as_ref(), config.difficulty)) {
        Ok(physics) => physics,
        Err(error) => {
            report(error);
            return bevy::app::AppExit::error();
        }
    };
    if config.multiplayer.spawn_offset != 0.0 {
        let mut spawn = physics.board.part_transforms()[skate_core::physics::board::BodyId::Deck.index()];
        spawn.translation.x += config.multiplayer.spawn_offset;
        physics.board.set_transform(spawn);
    }
    eprintln!("REPORT_META stage=skater_initialization");
    let skater = match bevy::log::info_span!("load_skater").in_scope(|| physics::SkaterRuntime::load(&config.asset_root, &graphs, &physics, config.difficulty.key())) {
        Ok(skater) => skater,
        Err(error) => {
            report(error);
            return bevy::app::AppExit::error();
        }
    };
    eprintln!("REPORT_META stage=controls_initialization");
    let controls = match physics::PlayerControls::load(&config.asset_root) {
        Ok(controls) => controls,
        Err(error) => { report(error); return bevy::app::AppExit::error(); }
    };
    if config.check_assets {
        if let Err(error) = camera::CameraRuntime::load(&config.asset_root) {
            report(error);
            return bevy::app::AppExit::error();
        }
        eprintln!("SKATE_ASSETS_READY");
        return bevy::app::AppExit::Success;
    }
    eprintln!("REPORT_META stage=renderer_and_app_initialization");
    let mut app = app::build(config, manifest, graphs, physics, skater);
    app.insert_resource(controls);
    eprintln!("REPORT_META stage=app_run");
    drop(_startup);
    app.run()
}

#[cfg(test)]
#[path = "tests/action_host.rs"]
mod action_host_tests;
