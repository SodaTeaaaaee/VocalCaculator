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
use vocal_calculator::components::network_panel::PeerDisplayInfo as PeerDisplayProps;
use vocal_calculator::ui::bridge::{
    create_router, handle_action, handle_connect_peer, handle_digit, handle_disconnect_peer,
    handle_operator, handle_route_approval, handle_route_toggled, handle_save_display_name,
    handle_scan_peers, handle_toggle_remote_control, init_networking, set_router_user_mute,
    start_network_event_loop, toggle_theme,
};
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

fn close_all_panels(ctx: &mut CalcContext) {
    *ctx.audio.about_visible.write() = false;
    *ctx.settings.panel_visible.write() = false;
    *ctx.net.panel_visible.write() = false;
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
            connected_peer_index: Signal::new(-1),
            peers: Signal::new(vec![]),
            matrix_node_ids: Signal::new(vec![]),
            matrix_size: Signal::new(0),
            matrix_cells: Signal::new(vec![]),
            peer_names: Signal::new(vec![]),
            my_index: Signal::new(0),
            remote_controlled: Signal::new(false),
            executing_remotely: Signal::new(false),
            allow_remote_control: Signal::new(false),
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

        // Open persistent storage -- this also loads or generates the
        // DeviceIdentity (Ed25519 keypair + stable node_id).
        let storage = Storage::open(&config_dir).expect("Failed to initialise storage");
        let app_config = storage.config().clone();

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
        // identity and paired-device data.
        let storage = Arc::new(storage);
        init_networking(tx, storage);

        // Spawn the async event loop that consumes UiEvents and updates
        // CalcContext signals.
        start_network_event_loop(ctx_hook, rx);
    });

    // Snapshot display values from context signals so that the
    // GenerationalRef temporaries are dropped before the move closures
    // below.  This avoids E0505 "cannot move out of `ctx` because it is
    // borrowed".
    let display_text = (*ctx.display.text.read()).clone();
    let history_text = (*ctx.display.history.read()).clone();
    let memory_indicator = (*ctx.display.memory_indicator.read()).clone();
    let mode_indicator = (*ctx.audio.mode_indicator.read()).clone();
    let error_state = *ctx.display.is_error.read();
    let audio_status = (*ctx.audio.audio_status.read()).clone();
    let audio_muted = *ctx.audio.muted.read();
    let audio_volume = *ctx.audio.volume.read();
    let dark_mode = *ctx.audio.dark_mode.read();
    let network_status = (*ctx.net.status.read()).clone();
    let remote_controlled = *ctx.net.remote_controlled.read();
    let executing_remotely = *ctx.net.executing_remotely.read();
    let about_visible = *ctx.audio.about_visible.read();
    let settings_panel_visible = *ctx.settings.panel_visible.read();
    let network_panel_visible = *ctx.net.panel_visible.read();
    let scanning = *ctx.net.scanning.read();
    let allow_remote_control = *ctx.net.allow_remote_control.read();
    let connected_peer_index = *ctx.net.connected_peer_index.read();
    let matrix_size = *ctx.net.matrix_size.read();
    let peers: Vec<PeerDisplayProps> = (*ctx.net.peers.read())
        .iter()
        .map(|peer| PeerDisplayProps {
            name: (*peer.name.read()).clone(),
            address: (*peer.address.read()).clone(),
            is_connected: *peer.is_connected.read(),
            route_active: *peer.route_active.read(),
            approval_pending: *peer.approval_pending.read(),
            trust_label: (*peer.trust_label.read()).clone(),
            latency_ms: *peer.latency_ms.read(),
            index: *peer.index.read(),
            node_id_string: (*peer.node_id_string.read()).clone(),
        })
        .collect();
    let peer_names = (*ctx.net.peer_names.read()).clone();
    let my_index = *ctx.net.my_index.read();
    let matrix_cells = (*ctx.net.matrix_cells.read()).clone();
    let app_version = (*ctx.app_version.read()).clone();
    let settings_display_name = (*ctx.settings.display_name.read()).clone();
    let settings_save_status = (*ctx.settings.save_status.read()).clone();
    let keyboard_pressed = *ctx.keyboard_pressed.read();
    let last_keyboard_action = (*ctx.last_keyboard_action.read()).clone();

    rsx! {
        document::Style { "{APP_CSS}" }

        CalculatorUI {
            // -- Display data (read from context signals) --
            display_text: display_text,
            history_text: history_text,
            memory_indicator: memory_indicator,
            mode_indicator: mode_indicator,
            error_state: error_state,
            audio_status: audio_status,
            audio_muted: audio_muted,
            audio_volume: audio_volume,
            dark_mode: dark_mode,

            // -- Network status --
            network_status: network_status,
            remote_controlled: remote_controlled,
            executing_remotely: executing_remotely,

            // -- Overlay visibility --
            about_visible: about_visible,
            settings_panel_visible: settings_panel_visible,
            network_panel_visible: network_panel_visible,
            scanning: scanning,
            allow_remote_control: allow_remote_control,

            // -- Peer / routing data --
            peers: peers,
            connected_peer_index: connected_peer_index,
            matrix_size: matrix_size,
            peer_names: peer_names,
            my_index: my_index,
            matrix_cells: matrix_cells,

            // -- App metadata --
            app_version: app_version,

            // -- Settings --
            settings_display_name: settings_display_name,
            settings_save_status: settings_save_status,

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

            // -- About dialog --
            on_show_about: {
                let mut ctx = ctx.clone();
                move |_: ()| {
                    close_all_panels(&mut ctx);
                    *ctx.audio.about_visible.write() = true;
                }
            },
            on_close_about: { let mut ctx = ctx.clone(); move |_: ()| { *ctx.audio.about_visible.write() = false; } },

            // -- Settings panel --
            on_show_settings: {
                let mut ctx = ctx.clone();
                move |_: ()| {
                    close_all_panels(&mut ctx);
                    *ctx.settings.panel_visible.write() = true;
                }
            },
            on_close_settings: { let mut ctx = ctx.clone(); move |_: ()| { *ctx.settings.panel_visible.write() = false; } },
            on_save_display_name: { let ctx = ctx.clone(); move |name: String| handle_save_display_name(ctx.clone(), name) },

            // -- Network panel --
            on_show_network_settings: {
                let mut ctx = ctx.clone();
                move |_: ()| {
                    close_all_panels(&mut ctx);
                    *ctx.net.panel_visible.write() = true;
                }
            },
            on_close_network_settings: { let mut ctx = ctx.clone(); move |_: ()| { *ctx.net.panel_visible.write() = false; } },
            on_connect_to_peer: { let ctx = ctx.clone(); move |id: String| handle_connect_peer(ctx.clone(), id) },
            on_disconnect_peer: { let ctx = ctx.clone(); move |id: String| handle_disconnect_peer(ctx.clone(), id) },
            on_approve_route_request: { let ctx = ctx.clone(); move |id: String| handle_route_approval(ctx.clone(), id, true) },
            on_deny_route_request: { let ctx = ctx.clone(); move |id: String| handle_route_approval(ctx.clone(), id, false) },
            on_scan_peers: { let ctx = ctx.clone(); move |_: ()| handle_scan_peers(ctx.clone()) },
            on_toggle_remote_control: { let ctx = ctx.clone(); move |_: ()| handle_toggle_remote_control(ctx.clone()) },
            on_route_toggled: { let ctx = ctx.clone(); move |(row, col, value): (i32, i32, bool)| handle_route_toggled(ctx.clone(), row, col, value) },

            // -- Keyboard handler (dispatch through bridge) --
            keyboard_pressed: keyboard_pressed,
            last_keyboard_action: last_keyboard_action,
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
