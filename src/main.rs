use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::prelude::*;
use vocal_calculator::app::config::AppConfig;
use vocal_calculator::app::network_mode::{self, NetworkMode};
use vocal_calculator::app::storage::Storage;
use vocal_calculator::audio::{AudioMode, VocalAudio};
use vocal_calculator::components::calculator::CalculatorUI;
use vocal_calculator::net::protocol::NodeId;
use vocal_calculator::net::view::BindStatus;
use vocal_calculator::ui::bridge::{
    create_router, handle_action, handle_connect_peer, handle_digit, handle_disconnect_peer,
    handle_operator, handle_save_display_name, handle_scan_peers, handle_toggle_remote_control,
    init_networking, set_router_user_mute, start_network_event_loop, toggle_theme,
};
use vocal_calculator::ui::command::WorkbenchTab;
use vocal_calculator::ui::events::create_ui_channel;
use vocal_calculator::ui::state::{
    AudioUiState, CalcContext, CalcDisplay, NetUiState, SettingsState,
};

const APP_CSS: &str = concat!(
    include_str!("styles/main.css"),
    "\n",
    include_str!("styles/calculator.css"),
    "\n",
    include_str!("styles/display.css"),
    "\n",
    include_str!("styles/button.css"),
    "\n",
    include_str!("styles/status_bar.css"),
    "\n",
    include_str!("styles/panels.css"),
    "\n",
    include_str!("styles/workbench.css"),
    "\n",
    r#"
.calculator-status-stack {
  grid-row: 2;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 2px;
}
.calculator-status-stack .status-bar,
.calculator-status-stack .presence-banner {
  grid-row: auto;
  align-self: stretch;
}
button.status-chip {
  cursor: pointer;
  font: inherit;
}
"#,
);

static VOLUME_SAVE_GENERATION: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "android")]
const LOG_TAG: &str = "VocalCalculator";

fn main() {
    init_logging();

    // Resolve the process-wide NetworkMode exactly once, before any
    // networking or UI initialisation happens. Priority: CLI
    // `--network-mode` > env `VOCAL_CALCULATOR_NETWORK_MODE` > config
    // `[network] mode` > legacy `[network] enabled` fallback. An
    // invalid value at any of those levels is a hard error -- we never
    // silently fall back to `Lan`.
    let app_config = AppConfig::load();
    match network_mode::resolve_from_process(&app_config.network) {
        Ok(mode) => network_mode::set(mode),
        Err(e) => {
            eprintln!("网络模式解析失败：{e}（有效值：lan, offline, loopback-test）");
            std::process::exit(2);
        }
    }

    launch_app();
}

#[cfg(not(target_os = "android"))]
fn init_logging() {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("vocal_calculator=info,info"),
    )
    .try_init();
}

#[cfg(target_os = "android")]
fn init_logging() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag(LOG_TAG)
            .with_max_level(log::LevelFilter::Info),
    );
}

#[cfg(not(target_os = "android"))]
fn launch_app() {
    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("VocalCalculator")
        .with_decorations(false)
        .with_resizable(true)
        .with_inner_size(dioxus::desktop::LogicalSize::new(1180.0, 760.0))
        .with_min_inner_size(dioxus::desktop::LogicalSize::new(320.0, 520.0));

    dioxus::LaunchBuilder::desktop()
        .with_cfg(dioxus::desktop::Config::new().with_window(window))
        .launch(App);
}

#[cfg(target_os = "android")]
fn launch_app() {
    dioxus::LaunchBuilder::mobile().launch(App);
}

fn audio_mode_from_config(value: &str) -> AudioMode {
    match value {
        "broken" => AudioMode::Broken,
        "music" => AudioMode::Music,
        "silent" => AudioMode::Silent,
        _ => AudioMode::Normal,
    }
}

fn audio_mode_to_config(mode: AudioMode) -> &'static str {
    match mode {
        AudioMode::Normal => "normal",
        AudioMode::Broken => "broken",
        AudioMode::Music => "music",
        AudioMode::Silent => "silent",
    }
}

fn save_config(update: impl FnOnce(&mut AppConfig)) {
    let mut app_config = AppConfig::load();
    update(&mut app_config);
    if let Err(e) = app_config.save() {
        log::error!("Failed to save config: {}", e);
    }
}

fn schedule_volume_save(volume: f64) {
    let generation = VOLUME_SAVE_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if VOLUME_SAVE_GENERATION.load(Ordering::Relaxed) == generation {
            save_config(|cfg| cfg.volume = volume.clamp(0.0, 1.0));
        }
    });
}

#[component]
fn App() -> Element {
    let audio_ref = use_hook(|| Rc::new(RefCell::new(VocalAudio::new())));

    // Provide CalcContext so child components can access shared state via
    // use_context::<CalcContext>().
    let ctx = use_context_provider(|| CalcContext {
        display: CalcDisplay {
            text: Signal::new("0".to_string()),
            history: Signal::new(String::new()),
            memory_indicator: Signal::new(String::new()),
            is_error: Signal::new(false),
        },
        audio: AudioUiState {
            mode_indicator: Signal::new(String::new()),
            mode: Signal::new(AudioMode::Normal),
            volume: Signal::new(1.0),
            muted: Signal::new(false),
            dark_mode: Signal::new(false),
            about_visible: Signal::new(false),
            audio_status: Signal::new(String::new()),
        },
        net: NetUiState {
            panel_visible: Signal::new(false),
            scanning: Signal::new(false),
            status: Signal::new(String::new()),
            peers: Signal::new(vec![]),
            remote_controlled: Signal::new(false),
            executing_remotely: Signal::new(false),
            allow_remote_control: Signal::new(false),
            bind: Signal::new(BindStatus::Offline),
            local_node_id: Signal::new(None),
            local_fingerprint: Signal::new(String::new()),
            controllers: Signal::new(vec![]),
            selected_executor: Signal::new(None),
            workbench_tab: Signal::new(WorkbenchTab::ThisDevice),
        },
        settings: SettingsState {
            panel_visible: Signal::new(false),
            display_name: Signal::new(String::new()),
            save_status: Signal::new(String::new()),
        },
        app_version: Signal::new(env!("CARGO_PKG_VERSION").to_string()),
        keyboard_pressed: Signal::new(false),
        last_keyboard_action: Signal::new(String::new()),
    });

    // One-time initialisation: create the calculator Router, open
    // persistent storage (which loads or creates the DeviceIdentity),
    // create the UiEvent channel, start networking, and launch the
    // event loop that bridges async network events to Dioxus signals.
    let ctx_hook = ctx.clone();
    let audio_ref_hook = audio_ref.clone();
    use_hook(move || {
        // Determine the config directory (same path AppConfig uses).
        let config_dir = sysdirs::config_dir()
            .map(|p| p.join("vocal_calculator"))
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        // Storage is required only for the authenticated network identity.
        // A migration/identity failure must not take down the local calculator.
        let storage = match Storage::open(&config_dir) {
            Ok(storage) => Some(storage),
            Err(error) => {
                log::error!(
                    "Persistent storage unavailable; networking disabled, local calculator remains available: {}",
                    error
                );
                None
            }
        };
        let app_config = storage
            .as_ref()
            .map(|storage| storage.config().clone())
            .unwrap_or_else(AppConfig::load);

        let configured_mode = audio_mode_from_config(&app_config.audio_mode);
        let configured_volume = app_config.volume.clamp(0.0, 1.0);
        let mut init_ctx = ctx_hook.clone();
        *init_ctx.audio.mode.write() = configured_mode;
        *init_ctx.audio.mode_indicator.write() = configured_mode.name().to_string();
        *init_ctx.audio.volume.write() = configured_volume;
        *init_ctx.audio.muted.write() = app_config.muted;
        *init_ctx.audio.dark_mode.write() = app_config.dark_mode;
        *init_ctx.settings.display_name.write() = app_config.network.display_name.clone();
        *init_ctx.net.allow_remote_control.write() = app_config.network.allow_remote_control;
        *init_ctx.net.status.write() = match network_mode::current() {
            NetworkMode::Lan => "已启用".to_string(),
            NetworkMode::Offline => "离线模式".to_string(),
            NetworkMode::LoopbackTest => "回环测试模式".to_string(),
        };

        let audio_status = {
            let mut audio = audio_ref_hook.borrow_mut();
            if let Some(audio) = audio.as_mut() {
                audio.set_mode(configured_mode);
                audio.set_volume(configured_volume);
                format!("音频正常 ({} 个音效)", audio.sound_count())
            } else {
                "无音频设备".to_string()
            }
        };
        *init_ctx.audio.audio_status.write() = audio_status;

        create_router(ctx_hook.clone(), audio_ref_hook.clone());
        set_router_user_mute(app_config.muted);
        dioxus::document::eval(&format!(
            r#"document.documentElement.setAttribute("data-theme", "{}")"#,
            if app_config.dark_mode {
                "dark"
            } else {
                "light"
            }
        ));

        // Create the typed channel that bridges the async networking
        // runtime with the UI thread.
        let (tx, rx) = create_ui_channel();

        // Start networking, passing the event sender so the runtime can
        // push UiEvents, and the Storage so the NetworkManager can read
        // the stable authenticated device identity.
        if let Some(storage) = storage {
            init_networking(tx, Arc::new(storage));
        } else {
            *init_ctx.net.status.write() = "网络存储不可用，本机计算器仍可正常使用".to_string();
        }

        // Spawn the async event loop that consumes UiEvents and updates
        // CalcContext signals.
        start_network_event_loop(ctx_hook, rx);
    });

    rsx! {
        document::Style { "{APP_CSS}" }

        CalculatorUI {
            // -- Calculator event handlers (dispatch through bridge) --
            on_digit_pressed: { let ctx = ctx.clone(); move |d: u8| handle_digit(ctx.clone(), d) },
            on_decimal_point: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "decimal-point") },
            on_operator_pressed: { let ctx = ctx.clone(); move |op: String| handle_operator(ctx.clone(), &op) },
            on_equals: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "equals") },
            on_percent: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "percent") },
            on_mu: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "mu") },
            on_square_root: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "sqrt") },
            on_backspace: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "backspace") },
            on_clear_input: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "clear") },
            on_all_clear: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "all-clear") },
            on_plus_minus: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "plus-minus") },
            on_memory_recall: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "memory-recall") },
            on_memory_add: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "memory-add") },
            on_memory_subtract: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "memory-subtract") },
            on_memory_clear: { let ctx = ctx.clone(); move |_: ()| handle_action(ctx.clone(), "memory-clear") },

            // -- Audio callbacks --
            on_switch_audio_mode: {
                let mut ctx = ctx.clone();
                let audio_ref = audio_ref.clone();
                move |_: ()| {
                    let next = ctx.audio.mode.read().next();
                    if let Some(audio) = audio_ref.borrow_mut().as_mut() {
                        audio.set_mode(next);
                    }
                    *ctx.audio.mode.write() = next;
                    *ctx.audio.mode_indicator.write() = next.name().to_string();
                    save_config(|cfg| cfg.audio_mode = audio_mode_to_config(next).to_string());
                }
            },
            on_toggle_mute: {
                let mut ctx = ctx.clone();
                move |_: ()| {
                    let current = *ctx.audio.muted.read();
                    let next = !current;
                    *ctx.audio.muted.write() = next;
                    set_router_user_mute(next);
                    save_config(|cfg| cfg.muted = next);
                }
            },
            on_volume_changed: {
                let mut ctx = ctx.clone();
                let audio_ref = audio_ref.clone();
                move |v: f64| {
                    *ctx.audio.volume.write() = v;
                    if let Some(audio) = audio_ref.borrow_mut().as_mut() {
                        audio.set_volume(v);
                    }
                    schedule_volume_save(v);
                }
            },

            // -- Theme toggle --
            on_toggle_theme: { let ctx = ctx.clone(); move |_: ()| toggle_theme(ctx.clone()) },

            on_save_display_name: { let ctx = ctx.clone(); move |name: String| handle_save_display_name(ctx.clone(), name) },
            on_use_executor: { let ctx = ctx.clone(); move |id: NodeId| handle_connect_peer(ctx.clone(), id.to_string()) },
            on_stop_executor: { let ctx = ctx.clone(); move |id: NodeId| handle_disconnect_peer(ctx.clone(), id.to_string()) },
            on_scan_peers: { let ctx = ctx.clone(); move |_: ()| handle_scan_peers(ctx.clone()) },
            on_toggle_remote_control: { let ctx = ctx.clone(); move |_: ()| handle_toggle_remote_control(ctx.clone()) },

            on_keyboard_action: { let ctx = ctx.clone(); move |action: String| handle_action(ctx.clone(), &action) },
            on_keyboard_pressed: {
                let mut ctx = ctx.clone();
                move |pressed: bool| {
                    *ctx.keyboard_pressed.write() = pressed;
                }
            },
            on_last_action: {
                let mut ctx = ctx.clone();
                move |action: String| {
                    *ctx.last_keyboard_action.write() = action;
                }
            },
        }
    }
}
