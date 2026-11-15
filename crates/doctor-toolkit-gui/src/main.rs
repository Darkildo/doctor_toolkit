//! Graphical browser for the game files of the EARS engine.
//!
//! Usage: `doctor-toolkit-gui [GAME_DIR [STREAM]]`, e.g. `doctor-toolkit-gui "The Godfather II" gf2/frontend`;
//! without a directory the last game opens again (Preferences). `--version`
//! prints the version, `--write-icons <dir>` writes the program icons as an
//! icon theme tree (for `tools/install-desktop.sh`). Panics are written to
//! the log file (`doctor_toolkit::report`) and shown in the window instead of
//! closing it. Settings live in `~/.config/doctor-toolkit` ([`settings`]).
//!
//! Setting `DOCTOR_TOOLKIT_GUI_SELFTEST=<stream>` opens that stream, scrolls
//! through its resources while measuring frame times, prints the result to
//! stderr and exits. It checks that the interface stays responsive on large files.
//!
//! `DOCTOR_TOOLKIT_GUI_SHOT=<file.png>` saves a screenshot and exits,
//! `DOCTOR_TOOLKIT_GUI_SELECT=<index>` selects a resource first and
//! `DOCTOR_TOOLKIT_GUI_CHANGES=1` shows the Changes panel and
//! `DOCTOR_TOOLKIT_GUI_MODS=1` the mods window and
//! `DOCTOR_TOOLKIT_GUI_SEARCH=<query>` searches once the game index is ready.
//! `DOCTOR_TOOLKIT_GUI_SHELL=<window>[,…]` opens windows of the shell
//! (palette, preferences, about, shortcuts, …; see `app::Capture`).
//! `DOCTOR_TOOLKIT_GUI_MODEL` and `DOCTOR_TOOLKIT_GUI_TEXTURE` preset the
//! model and texture viewers (see their modules);
//! `DOCTOR_TOOLKIT_GUI_IMPORT=<file.glb>` opens the model import window for
//! the selected mesh or metamodel with that file.
//! These modes start from the default settings and layout and save nothing,
//! unless `DOCTOR_TOOLKIT_CONFIG_DIR` names a settings folder.
//!
//! The interface renders with wgpu. Without any wgpu adapter it falls back to
//! OpenGL through glow, and models get the software preview;
//! `DOCTOR_TOOLKIT_GUI_RENDERER=glow` (or the renderer preference) forces that path.
//!
//! `DOCTOR_TOOLKIT_GUI_BENCH=instances:<n>` or `model:<stream>:<index>` runs
//! the renderer benchmark without a window (see `bench.rs`).

mod anim_graph_view;
mod anim_view;
mod app;
mod apx_view;
// The mixer's device side needs the `audio` feature; its tests run without.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
mod audio_mix;
#[cfg(feature = "audio")]
mod audio_out;
mod bench;
mod build_info;
mod changes_view;
mod character_view;
mod collision_view;
mod cutscene_view;
mod dialogue_view;
mod external_view;
mod game_light;
mod game_view;
mod hex;
mod jobs;
mod level_view;
mod logic_editor;
mod logic_live;
mod logic_view;
mod model_view;
mod nav;
mod node_graph;
mod profile;
mod render;
mod resource_view;
mod sbk_view;
mod script_editor;
mod script_live;
mod script_view;
mod settings;
mod text_view;
mod texture_view;
mod video_view;
mod views;
mod wizard_view;

use std::path::PathBuf;

use doctor_toolkit_mods::guard::{Guarded, print_version_or_help};
use doctor_toolkit_mods::icon;

fn main() -> eframe::Result {
    if print_version_or_help(
        "doctor-toolkit-gui",
        "doctor-toolkit-gui [GAME_DIR [STREAM]]\n       doctor-toolkit-gui --write-icons DIR\n       doctor-toolkit-gui --render-cutscene GAME_DIR FOLDER_STREAM [options]",
    ) {
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--write-icons") {
        std::process::exit(write_icons());
    }
    if std::env::args().nth(1).as_deref() == Some("--render-cutscene") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        std::process::exit(cutscene_view::render::main(&args));
    }
    profile::init();
    doctor_toolkit::report::init("doctor-toolkit-gui");
    let game_dir = std::env::args_os().nth(1).map(PathBuf::from);
    let stream = std::env::args().nth(2);
    let selftest = std::env::var("DOCTOR_TOOLKIT_GUI_SELFTEST").ok();
    if let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_BENCH") {
        let code = bench::run(&spec, game_dir.as_deref());
        profile::write_report();
        std::process::exit(code);
    }
    let store = settings::Store::for_run();
    let (settings, settings_error) = store.load();
    if let Some(error) = &settings_error {
        doctor_toolkit::report::log(&format!("settings ignored: {error}"));
    }
    doctor_toolkit_mods::i18n::set_language(
        Some(settings.language.as_str()).filter(|l| !l.is_empty()),
    );
    let window = &settings.window;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("doctor_toolkit")
        .with_app_id(icon::Variant::Editor.name())
        .with_icon(icon::icon_data(icon::Variant::Editor))
        .with_inner_size([window.width.max(400.0), window.height.max(300.0)])
        .with_maximized(window.maximized);
    if let (Some(x), Some(y)) = (window.x, window.y) {
        viewport = viewport.with_position([x, y]);
    }
    let renderer = choose_renderer(settings.renderer);
    profile::mark("renderer chosen");
    let mut options = eframe::NativeOptions {
        viewport,
        renderer,
        ..Default::default()
    };
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.native_adapter_selector = Some(std::sync::Arc::new(select_adapter));
        // Batched draws of the scene renderer; the profiler also times the
        // scene pass on the GPU (stage 67).
        let features = render::optional_features(profile::enabled());
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = base(adapter);
            descriptor.required_features |= adapter.features() & features;
            descriptor.required_limits =
                render::device_limits(adapter, descriptor.required_limits.clone());
            descriptor
        });
    }
    let result = eframe::run_native(
        "doctor_toolkit",
        options,
        Box::new(move |cc| {
            profile::mark("window and device");
            let mut app = app::BrowserApp::new(cc, game_dir, stream, selftest, settings, store);
            if let Some(error) = settings_error {
                app.warn(doctor_toolkit_mods::i18n::trf(
                    "app.settings_ignored",
                    &[("error", &error)],
                ));
            }
            profile::mark("editor created");
            Ok(Box::new(Guarded::new(app)))
        }),
    );
    profile::write_report();
    result
}

/// `--write-icons <dir>`: the icons of both programs as `hicolor/…` files.
fn write_icons() -> i32 {
    let Some(dir) = std::env::args_os().nth(2).map(PathBuf::from) else {
        eprintln!("usage: doctor-toolkit-gui --write-icons DIR");
        return 2;
    };
    match icon::write_icons(&dir) {
        Ok(files) => {
            for file in files {
                println!("{}", file.display());
            }
            0
        }
        Err(error) => {
            eprintln!("cannot write icons into {}: {error}", dir.display());
            1
        }
    }
}

/// wgpu when an adapter exists, otherwise OpenGL; the environment and then
/// the preference can force one.
fn choose_renderer(preference: settings::Renderer) -> eframe::Renderer {
    use eframe::wgpu;
    match std::env::var("DOCTOR_TOOLKIT_GUI_RENDERER").as_deref() {
        Ok("glow") => return eframe::Renderer::Glow,
        Ok("wgpu") => return eframe::Renderer::Wgpu,
        _ => {}
    }
    match preference {
        settings::Renderer::Glow => return eframe::Renderer::Glow,
        settings::Renderer::Wgpu => return eframe::Renderer::Wgpu,
        settings::Renderer::Auto => {}
    }
    let backends =
        wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY | wgpu::Backends::GL);
    // Vulkan, Metal or DirectX 12 first: starting OpenGL (EGL) only to
    // learn that a better adapter exists costs a tenth of a second.
    let any_adapter = |backends: wgpu::Backends| {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        !pollster::block_on(instance.enumerate_adapters(backends)).is_empty()
    };
    let primary = backends & wgpu::Backends::PRIMARY;
    let found = (!primary.is_empty() && any_adapter(primary))
        || (!(backends - primary).is_empty() && any_adapter(backends - primary));
    if !found {
        eprintln!("No wgpu adapter found; using OpenGL and the software model preview.");
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    }
}

/// Picks the best adapter that can present to the window: a GPU before a
/// software rasteriser, which in turn beats failing to start.
fn select_adapter(
    adapters: &[eframe::wgpu::Adapter],
    surface: Option<&eframe::wgpu::Surface<'_>>,
) -> Result<eframe::wgpu::Adapter, String> {
    adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .min_by_key(|a| render::adapter_rank(a))
        .cloned()
        .ok_or_else(|| format!("none of {} wgpu adapter(s) can present", adapters.len()))
}
