use dioxus::prelude::*;
use serde::Deserialize;

use crate::ui::command::WorkbenchTab;
use crate::ui::state::CalcContext;

/// Whether the wide-desktop workbench is the interaction surface.
#[derive(Clone, Copy)]
pub struct WorkbenchSurface(pub Signal<bool>);

const WORKBENCH_SURFACE_SCRIPT: &str = r#"
(() => {
    const send = () => {
        const width = window.innerWidth;
        const height = window.innerHeight;
        dioxus.send(width >= 941 && height > 0 && (width / height) > 0.8);
    };
    send();
    window.addEventListener("resize", send);
    await new Promise(() => {});
})();
"#;

const KEYBOARD_LISTENER_SCRIPT: &str = r#"
(() => {
    if (window.__vcKeyboardCleanup) {
        window.__vcKeyboardCleanup();
    }

    const handledKeys = new Set([
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
        "+", "-", "*", "/", "=", "Enter", ".", ",", "Backspace",
        "Escape", "Delete", "%", "r", "R", "s", "S", "p", "P",
        "u", "U", "n", "N", "m", "M", "a", "A", "b", "B",
        "x", "X", "c", "C", "t", "T", "v", "V", "o", "O",
        "F1", "F2", "F3", "F5"
    ]);

    const handledCodes = new Set([
        "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4",
        "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9",
        "NumpadAdd", "NumpadSubtract", "NumpadMultiply", "NumpadDivide",
        "NumpadDecimal", "NumpadComma", "NumpadEnter", "F1", "F2",
        "F3", "F5", "F9"
    ]);

    const editableTarget = (target) => {
        if (!(target instanceof Element)) {
            return false;
        }

        const editable = target.closest(
            'input, textarea, select, [contenteditable="true"], [contenteditable="plaintext-only"]'
        );

        return Boolean(editable && !editable.disabled && !editable.readOnly);
    };

    const activationTarget = (target) => {
        if (!(target instanceof Element)) {
            return false;
        }

        const activatable = target.closest('button, a[href], [role="button"]');
        return Boolean(activatable && !activatable.disabled);
    };

    const handled = (event) => {
        if (event.ctrlKey || event.altKey || event.metaKey) {
            return false;
        }

        return handledKeys.has(event.key) || handledCodes.has(event.code);
    };

    const send = (eventType, event, fromEditable = false) => {
        dioxus.send({
            eventType,
            key: event?.key ?? "",
            code: event?.code ?? "",
            ctrlKey: event?.ctrlKey ?? false,
            altKey: event?.altKey ?? false,
            metaKey: event?.metaKey ?? false,
            shiftKey: event?.shiftKey ?? false,
            repeat: event?.repeat ?? false,
            fromEditable,
        });
    };

    const onKeyDown = (event) => {
        const fromEditable = editableTarget(event.target);
        if (fromEditable && event.key !== "Escape") {
            return;
        }

        if (activationTarget(event.target) && event.key === "Enter") {
            return;
        }

        if (!handled(event)) {
            return;
        }

        event.preventDefault();
        event.stopPropagation();
        send("keydown", event, fromEditable);
    };

    const onKeyUp = (event) => {
        const fromEditable = editableTarget(event.target);
        if (fromEditable && event.key !== "Escape") {
            return;
        }

        if (activationTarget(event.target) && event.key === "Enter") {
            return;
        }

        if (!handled(event)) {
            return;
        }

        event.preventDefault();
        event.stopPropagation();
        send("keyup", event, fromEditable);
    };

    const onBlur = () => {
        send("keyup", null, false);
    };

    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("blur", onBlur, true);

    window.__vcKeyboardCleanup = () => {
        window.removeEventListener("keydown", onKeyDown, true);
        window.removeEventListener("keyup", onKeyUp, true);
        window.removeEventListener("blur", onBlur, true);
        window.__vcKeyboardCleanup = null;
    };
})();

await new Promise(() => {});
"#;

/// Track whether F2/F3 should change a workbench tab instead of an overlay.
pub fn use_workbench_surface() -> Signal<bool> {
    #[cfg(target_os = "android")]
    {
        use_signal(|| false)
    }

    #[cfg(not(target_os = "android"))]
    {
        let mut wide = use_signal(|| true);
        use_future(move || async move {
            let mut eval = document::eval(WORKBENCH_SURFACE_SCRIPT);
            loop {
                match eval.recv::<bool>().await {
                    Ok(is_wide) => wide.set(is_wide),
                    Err(_) => break,
                }
            }
        });
        wide
    }
}

pub fn close_overlays(ctx: &CalcContext) {
    let mut about = ctx.audio.about_visible;
    let mut settings = ctx.settings.panel_visible;
    let mut nearby = ctx.net.panel_visible;
    about.set(false);
    settings.set(false);
    nearby.set(false);
}

pub fn activate_settings(ctx: &CalcContext, workbench_surface: bool) {
    if workbench_surface {
        close_overlays(ctx);
        let mut tab = ctx.net.workbench_tab;
        tab.set(WorkbenchTab::Settings);
        return;
    }
    if *ctx.settings.panel_visible.read() {
        let mut settings = ctx.settings.panel_visible;
        settings.set(false);
        return;
    }
    close_overlays(ctx);
    let mut settings = ctx.settings.panel_visible;
    settings.set(true);
}

pub fn activate_nearby(ctx: &CalcContext, workbench_surface: bool) {
    if workbench_surface {
        close_overlays(ctx);
        let mut tab = ctx.net.workbench_tab;
        tab.set(WorkbenchTab::Nearby);
        return;
    }
    if *ctx.net.panel_visible.read() {
        let mut nearby = ctx.net.panel_visible;
        nearby.set(false);
        return;
    }
    close_overlays(ctx);
    let mut nearby = ctx.net.panel_visible;
    nearby.set(true);
}

fn reveal_nearby(ctx: &CalcContext, workbench_surface: bool) {
    if workbench_surface {
        close_overlays(ctx);
        let mut tab = ctx.net.workbench_tab;
        tab.set(WorkbenchTab::Nearby);
        return;
    }
    if *ctx.net.panel_visible.read() {
        return;
    }
    close_overlays(ctx);
    let mut nearby = ctx.net.panel_visible;
    nearby.set(true);
}

fn activate_about(ctx: &CalcContext) {
    let already_open = *ctx.audio.about_visible.read();
    close_overlays(ctx);
    if !already_open {
        let mut about = ctx.audio.about_visible;
        about.set(true);
    }
}

/// Props for the [`KeyboardHandler`] component.
#[derive(Props, Clone, PartialEq)]
pub struct KeyboardHandlerProps {
    /// Fired with an action string (e.g. `"digit:5"`, `"add"`, `"equals"`)
    /// when a calculator-relevant key is pressed.
    pub on_keyboard_action: EventHandler<String>,
    /// Fired by global keyboard shortcuts for app controls.
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_toggle_mute: EventHandler<()>,
    pub on_toggle_theme: EventHandler<()>,
    pub on_scan_peers: EventHandler<()>,
    /// Fired with `true` on key-down (accepted key), `false` on key-up.
    pub on_keyboard_pressed: EventHandler<bool>,
    /// Fired with the action string of the most recently accepted key-down.
    pub on_last_action: EventHandler<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyboardMessage {
    event_type: String,
    key: String,
    code: String,
    ctrl_key: bool,
    alt_key: bool,
    meta_key: bool,
    repeat: bool,
    from_editable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyboardShortcut {
    Calculator(&'static str),
    Escape,
    ToggleTheme,
    SwitchAudioMode,
    ToggleMute,
    ShowAbout,
    ShowSettings,
    ShowNearby,
    ScanPeers,
}

impl KeyboardMessage {
    fn is_key_down(&self) -> bool {
        self.event_type == "keydown"
    }

    fn is_key_up(&self) -> bool {
        self.event_type == "keyup"
    }

    fn has_blocked_modifier(&self) -> bool {
        self.ctrl_key || self.alt_key || self.meta_key
    }
}

fn digit_action_from_code(code: &str) -> Option<&'static str> {
    match code {
        "Numpad0" => Some("digit:0"),
        "Numpad1" => Some("digit:1"),
        "Numpad2" => Some("digit:2"),
        "Numpad3" => Some("digit:3"),
        "Numpad4" => Some("digit:4"),
        "Numpad5" => Some("digit:5"),
        "Numpad6" => Some("digit:6"),
        "Numpad7" => Some("digit:7"),
        "Numpad8" => Some("digit:8"),
        "Numpad9" => Some("digit:9"),
        _ => None,
    }
}

fn digit_action_from_key(key: &str) -> Option<&'static str> {
    match key {
        "0" => Some("digit:0"),
        "1" => Some("digit:1"),
        "2" => Some("digit:2"),
        "3" => Some("digit:3"),
        "4" => Some("digit:4"),
        "5" => Some("digit:5"),
        "6" => Some("digit:6"),
        "7" => Some("digit:7"),
        "8" => Some("digit:8"),
        "9" => Some("digit:9"),
        _ => None,
    }
}

fn shortcut_from_message(message: &KeyboardMessage) -> Option<KeyboardShortcut> {
    if message.has_blocked_modifier() {
        return None;
    }

    if let Some(action) = digit_action_from_key(&message.key) {
        return Some(KeyboardShortcut::Calculator(action));
    }

    if let Some(action) = digit_action_from_code(&message.code) {
        return Some(KeyboardShortcut::Calculator(action));
    }

    match message.code.as_str() {
        "NumpadAdd" => return Some(KeyboardShortcut::Calculator("add")),
        "NumpadSubtract" => return Some(KeyboardShortcut::Calculator("subtract")),
        "NumpadMultiply" => return Some(KeyboardShortcut::Calculator("multiply")),
        "NumpadDivide" => return Some(KeyboardShortcut::Calculator("divide")),
        "NumpadDecimal" | "NumpadComma" => {
            return Some(KeyboardShortcut::Calculator("decimal-point"));
        }
        "NumpadEnter" => return Some(KeyboardShortcut::Calculator("equals")),
        "F1" => return Some(KeyboardShortcut::ShowAbout),
        "F2" => return Some(KeyboardShortcut::ShowSettings),
        "F3" => return Some(KeyboardShortcut::ShowNearby),
        "F5" => return Some(KeyboardShortcut::ScanPeers),
        "F9" => return Some(KeyboardShortcut::Calculator("plus-minus")),
        _ => {}
    }

    match message.key.as_str() {
        "+" => Some(KeyboardShortcut::Calculator("add")),
        "-" => Some(KeyboardShortcut::Calculator("subtract")),
        "*" => Some(KeyboardShortcut::Calculator("multiply")),
        "/" => Some(KeyboardShortcut::Calculator("divide")),
        "=" | "Enter" => Some(KeyboardShortcut::Calculator("equals")),
        "." | "," => Some(KeyboardShortcut::Calculator("decimal-point")),
        "Backspace" => Some(KeyboardShortcut::Calculator("backspace")),
        "Delete" => Some(KeyboardShortcut::Calculator("clear")),
        "Escape" => Some(KeyboardShortcut::Escape),
        "%" => Some(KeyboardShortcut::Calculator("percent")),
        _ => match message.key.to_ascii_lowercase().as_str() {
            "a" => Some(KeyboardShortcut::Calculator("memory-add")),
            "b" => Some(KeyboardShortcut::Calculator("memory-subtract")),
            "c" => Some(KeyboardShortcut::Calculator("clear")),
            "m" => Some(KeyboardShortcut::Calculator("memory-recall")),
            "n" => Some(KeyboardShortcut::ShowNearby),
            "o" => Some(KeyboardShortcut::SwitchAudioMode),
            "p" => Some(KeyboardShortcut::Calculator("percent")),
            "r" | "s" => Some(KeyboardShortcut::Calculator("sqrt")),
            "t" => Some(KeyboardShortcut::ToggleTheme),
            "u" => Some(KeyboardShortcut::Calculator("mu")),
            "v" => Some(KeyboardShortcut::ToggleMute),
            "x" => Some(KeyboardShortcut::Calculator("memory-clear")),
            _ => None,
        },
    }
}

fn overlay_is_open(ctx: &CalcContext) -> bool {
    *ctx.audio.about_visible.read()
        || *ctx.settings.panel_visible.read()
        || *ctx.net.panel_visible.read()
}

fn close_top_overlay(ctx: &CalcContext) -> bool {
    if *ctx.audio.about_visible.read() {
        let mut about = ctx.audio.about_visible;
        about.set(false);
        true
    } else if *ctx.settings.panel_visible.read() {
        let mut settings = ctx.settings.panel_visible;
        settings.set(false);
        true
    } else if *ctx.net.panel_visible.read() {
        let mut nearby = ctx.net.panel_visible;
        nearby.set(false);
        true
    } else {
        false
    }
}

fn emit_calculator_action(props: &KeyboardHandlerProps, action: &'static str) {
    props.on_keyboard_pressed.call(true);
    props.on_last_action.call(action.to_owned());
    props.on_keyboard_action.call(action.to_owned());
}

fn dispatch_shortcut(
    props: &KeyboardHandlerProps,
    ctx: &CalcContext,
    workbench_surface: bool,
    shortcut: KeyboardShortcut,
    from_editable: bool,
    repeat: bool,
) {
    if from_editable {
        if shortcut == KeyboardShortcut::Escape {
            close_top_overlay(ctx);
        }
        return;
    }

    if repeat && !matches!(shortcut, KeyboardShortcut::Calculator(_)) {
        return;
    }

    match shortcut {
        KeyboardShortcut::Calculator(action) => {
            if !overlay_is_open(ctx) {
                emit_calculator_action(props, action);
            }
        }
        KeyboardShortcut::Escape => {
            if !close_top_overlay(ctx) {
                emit_calculator_action(props, "all-clear");
            }
        }
        KeyboardShortcut::ToggleTheme => props.on_toggle_theme.call(()),
        KeyboardShortcut::SwitchAudioMode => props.on_switch_audio_mode.call(()),
        KeyboardShortcut::ToggleMute => props.on_toggle_mute.call(()),
        KeyboardShortcut::ShowAbout => activate_about(ctx),
        KeyboardShortcut::ShowSettings => activate_settings(ctx, workbench_surface),
        KeyboardShortcut::ShowNearby => activate_nearby(ctx, workbench_surface),
        KeyboardShortcut::ScanPeers => {
            reveal_nearby(ctx, workbench_surface);
            props.on_scan_peers.call(());
        }
    }
}

fn dispatch_keyboard_message(
    props: &KeyboardHandlerProps,
    ctx: &CalcContext,
    workbench_surface: bool,
    message: KeyboardMessage,
) {
    if message.is_key_up() {
        props.on_keyboard_pressed.call(false);
        return;
    }

    if !message.is_key_down() {
        return;
    }

    if let Some(shortcut) = shortcut_from_message(&message) {
        dispatch_shortcut(
            props,
            ctx,
            workbench_surface,
            shortcut,
            message.from_editable,
            message.repeat,
        );
    }
}

/// Window-level keyboard listener for calculator and app shortcuts.
#[component]
pub fn KeyboardHandler(props: KeyboardHandlerProps) -> Element {
    let ctx = use_context::<CalcContext>();
    let WorkbenchSurface(workbench_surface) = use_context::<WorkbenchSurface>();

    use_future(move || {
        let props = props.clone();
        let ctx = ctx.clone();

        async move {
            let mut eval = document::eval(KEYBOARD_LISTENER_SCRIPT);

            loop {
                match eval.recv::<KeyboardMessage>().await {
                    Ok(message) => {
                        dispatch_keyboard_message(&props, &ctx, workbench_surface(), message)
                    }
                    Err(err) => {
                        log::warn!("Keyboard listener stopped: {}", err);
                        break;
                    }
                }
            }
        }
    });

    rsx! {
        div {
            aria_hidden: "true",
            class: "keyboard-focus-trap",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(key: &str, code: &str) -> KeyboardMessage {
        KeyboardMessage {
            event_type: "keydown".to_string(),
            key: key.to_string(),
            code: code.to_string(),
            ctrl_key: false,
            alt_key: false,
            meta_key: false,
            repeat: false,
            from_editable: false,
        }
    }

    fn action_for(key: &str, code: &str) -> Option<&'static str> {
        match shortcut_from_message(&msg(key, code)) {
            Some(KeyboardShortcut::Calculator(action)) => Some(action),
            _ => None,
        }
    }

    #[test]
    fn maps_digits_from_number_row_and_numpad() {
        assert_eq!(action_for("7", "Digit7"), Some("digit:7"));
        assert_eq!(action_for("Unidentified", "Numpad3"), Some("digit:3"));
    }

    #[test]
    fn maps_core_calculator_keys() {
        assert_eq!(action_for("+", "Equal"), Some("add"));
        assert_eq!(action_for("-", "Minus"), Some("subtract"));
        assert_eq!(action_for("*", "NumpadMultiply"), Some("multiply"));
        assert_eq!(action_for("/", "Slash"), Some("divide"));
        assert_eq!(action_for("=", "Equal"), Some("equals"));
        assert_eq!(action_for("Enter", "Enter"), Some("equals"));
        assert_eq!(action_for(".", "Period"), Some("decimal-point"));
        assert_eq!(action_for(",", "Comma"), Some("decimal-point"));
        assert_eq!(action_for("Backspace", "Backspace"), Some("backspace"));
        assert_eq!(action_for("Delete", "Delete"), Some("clear"));
    }

    #[test]
    fn maps_function_and_memory_keys() {
        assert_eq!(action_for("%", "Digit5"), Some("percent"));
        assert_eq!(action_for("p", "KeyP"), Some("percent"));
        assert_eq!(action_for("r", "KeyR"), Some("sqrt"));
        assert_eq!(action_for("s", "KeyS"), Some("sqrt"));
        assert_eq!(action_for("u", "KeyU"), Some("mu"));
        assert_eq!(action_for("F9", "F9"), Some("plus-minus"));
        assert_eq!(action_for("m", "KeyM"), Some("memory-recall"));
        assert_eq!(action_for("a", "KeyA"), Some("memory-add"));
        assert_eq!(action_for("b", "KeyB"), Some("memory-subtract"));
        assert_eq!(action_for("x", "KeyX"), Some("memory-clear"));
    }

    #[test]
    fn maps_app_shortcuts() {
        assert_eq!(
            shortcut_from_message(&msg("F1", "F1")),
            Some(KeyboardShortcut::ShowAbout)
        );
        assert_eq!(
            shortcut_from_message(&msg("F2", "F2")),
            Some(KeyboardShortcut::ShowSettings)
        );
        assert_eq!(
            shortcut_from_message(&msg("F3", "F3")),
            Some(KeyboardShortcut::ShowNearby)
        );
        assert_eq!(
            shortcut_from_message(&msg("n", "KeyN")),
            Some(KeyboardShortcut::ShowNearby)
        );
        assert_eq!(
            shortcut_from_message(&msg("F5", "F5")),
            Some(KeyboardShortcut::ScanPeers)
        );
        assert_eq!(
            shortcut_from_message(&msg("t", "KeyT")),
            Some(KeyboardShortcut::ToggleTheme)
        );
        assert_eq!(
            shortcut_from_message(&msg("o", "KeyO")),
            Some(KeyboardShortcut::SwitchAudioMode)
        );
        assert_eq!(
            shortcut_from_message(&msg("v", "KeyV")),
            Some(KeyboardShortcut::ToggleMute)
        );
    }

    #[test]
    fn ignores_ctrl_alt_meta_shortcuts() {
        let mut event = msg("1", "Digit1");
        event.ctrl_key = true;
        assert_eq!(shortcut_from_message(&event), None);

        let mut event = msg("+", "NumpadAdd");
        event.alt_key = true;
        assert_eq!(shortcut_from_message(&event), None);

        let mut event = msg("m", "KeyM");
        event.meta_key = true;
        assert_eq!(shortcut_from_message(&event), None);
    }
}
