use dioxus::prelude::*;

use crate::components::network_panel::NearbyDevices;
use crate::components::settings_panel::SettingsContent;
use crate::components::this_device::ThisDevicePanel;
use crate::net::protocol::NodeId;
use crate::net::view::{BindStatus, PeerViewModel};
use crate::ui::command::WorkbenchTab;

#[derive(Props, Clone, PartialEq)]
pub struct WorkbenchProps {
    pub active_tab: WorkbenchTab,
    pub on_tab_change: EventHandler<WorkbenchTab>,
    pub display_name: String,
    pub save_status: String,
    pub fingerprint: String,
    pub network_mode_label: String,
    pub bind: BindStatus,
    pub allow_remote_control: bool,
    pub controller_names: Vec<String>,
    pub selected_executor_name: Option<String>,
    pub on_save_name: EventHandler<String>,
    pub on_toggle_remote_control: EventHandler<MouseEvent>,
    pub scanning: bool,
    pub peers: Vec<PeerViewModel>,
    pub on_scan: EventHandler<MouseEvent>,
    pub on_use_executor: EventHandler<NodeId>,
    pub on_stop_executor: EventHandler<NodeId>,
    pub audio_status: String,
    pub audio_muted: bool,
    pub audio_volume: f64,
    pub mode_indicator: String,
    pub dark_mode: bool,
    pub app_version: String,
    pub on_switch_audio_mode: EventHandler<()>,
    pub on_toggle_mute: EventHandler<()>,
    pub on_volume_changed: EventHandler<f64>,
    pub on_toggle_theme: EventHandler<()>,
    pub on_show_about: EventHandler<()>,
}

/// Wide-desktop chrome: 本机 / 附近 / 设置.
#[component]
pub fn Workbench(props: WorkbenchProps) -> Element {
    let current_tab = props.active_tab;
    let tabs = [
        WorkbenchTab::ThisDevice,
        WorkbenchTab::Nearby,
        WorkbenchTab::Settings,
    ];

    rsx! {
        aside { class: "workbench", aria_label: "状态工作区",
            div { class: "workbench-tabs", role: "tablist", aria_label: "工作区标签",
                for tab in tabs {
                    {
                        let selected = current_tab == tab;
                        let class = if selected {
                            "workbench-tab is-active"
                        } else {
                            "workbench-tab"
                        };
                        rsx! {
                            button {
                                class: "{class}",
                                role: "tab",
                                aria_selected: if selected { "true" } else { "false" },
                                onclick: move |_| props.on_tab_change.call(tab),
                                "{tab.label()}"
                            }
                        }
                    }
                }
            }

            div {
                class: "workbench-content",
                role: "tabpanel",
                {
                    match current_tab {
                        WorkbenchTab::ThisDevice => rsx! {
                            div { class: "workbench-panel workbench-panel--this-device",
                                ThisDevicePanel {
                                    display_name: props.display_name.clone(),
                                    save_status: props.save_status.clone(),
                                    fingerprint: props.fingerprint.clone(),
                                    network_mode_label: props.network_mode_label.clone(),
                                    bind: props.bind.clone(),
                                    allow_remote_control: props.allow_remote_control,
                                    controller_names: props.controller_names.clone(),
                                    selected_executor_name: props.selected_executor_name.clone(),
                                    on_save_name: props.on_save_name,
                                    on_toggle_remote_control: props.on_toggle_remote_control,
                                }
                            }
                        },
                        WorkbenchTab::Nearby => rsx! {
                            div { class: "workbench-panel workbench-panel--nearby",
                                NearbyDevices {
                                    scanning: props.scanning,
                                    peers: props.peers.clone(),
                                    on_scan: props.on_scan,
                                    on_use_executor: props.on_use_executor,
                                    on_stop_executor: props.on_stop_executor,
                                }
                            }
                        },
                        WorkbenchTab::Settings => rsx! {
                            div { class: "workbench-panel workbench-panel--settings",
                                SettingsContent {
                                    display_name: props.display_name.clone(),
                                    save_status: props.save_status.clone(),
                                    audio_status: props.audio_status.clone(),
                                    audio_muted: props.audio_muted,
                                    audio_volume: props.audio_volume,
                                    mode_indicator: props.mode_indicator.clone(),
                                    dark_mode: props.dark_mode,
                                    app_version: props.app_version.clone(),
                                    on_save_name: props.on_save_name,
                                    on_switch_audio_mode: props.on_switch_audio_mode,
                                    on_toggle_mute: props.on_toggle_mute,
                                    on_volume_changed: props.on_volume_changed,
                                    on_toggle_theme: props.on_toggle_theme,
                                    on_show_about: props.on_show_about,
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}
