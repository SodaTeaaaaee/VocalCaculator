use dioxus::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::overlay::{Overlay, OverlayVariant, ToggleSwitch};
use crate::components::panel_controls::PanelSection;

// ---------------------------------------------------------------------------
// PeerDisplayInfo — per-peer row data for the peer list / routing matrix
// ---------------------------------------------------------------------------

/// Display information for a single discovered peer.
///
/// Uses simple `String` / `i32` props so the signal bridge can convert from
/// backend types without pulling networking internals into component props.
#[derive(Props, Clone, PartialEq)]
pub struct PeerDisplayInfo {
    pub name: String,
    pub address: String,
    pub is_connected: bool,
    /// Round-trip latency in milliseconds, or -1 if unknown.
    pub latency_ms: i32,
    /// Ordinal index of this peer in the list (assigned by the poll timer).
    pub index: i32,
    /// Stringified NodeId (UUID) — used by connect/disconnect callbacks.
    pub node_id_string: String,
}

// ---------------------------------------------------------------------------
// NetworkPanelProps
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct NetworkPanelProps {
    // -- visibility (forwarded to Overlay) --
    pub visible: bool,

    // -- network state --
    pub network_status: String,
    #[props(default)]
    pub remote_controlled: bool,
    #[props(default)]
    pub executing_remotely: bool,
    #[props(default)]
    pub scanning: bool,
    #[props(default)]
    pub allow_remote_control: bool,
    #[props(default)]
    pub audio_muted: bool,

    // -- peer list --
    #[props(default)]
    pub peers: Vec<PeerDisplayInfo>,
    /// Index of the currently connected peer, or -1 if none.
    #[props(default)]
    pub connected_peer_index: i32,

    // -- routing matrix --
    /// Number of nodes in the matrix (N for an N*N grid).
    #[props(default)]
    pub matrix_size: i32,
    /// Display names for each node column/row header.
    #[props(default)]
    pub peer_names: Vec<String>,
    /// Index of the local user's row in the matrix, or -1 if absent.
    #[props(default)]
    pub my_index: i32,
    /// Flat N*N boolean grid (row-major). Cell at (row, col) is
    /// `matrix_cells[row * matrix_size + col]`.
    #[props(default)]
    pub matrix_cells: Vec<bool>,

    // -- event handlers --
    pub onclose: EventHandler<MouseEvent>,
    /// Connect to the peer identified by its stringified NodeId.
    pub onconnect: EventHandler<String>,
    /// Disconnect from the peer identified by its stringified NodeId.
    pub ondisconnect: EventHandler<String>,
    pub onscan: EventHandler<MouseEvent>,
    pub ontoggle_remote_control: EventHandler<MouseEvent>,
    pub ontoggle_mute: EventHandler<MouseEvent>,
    /// A routing matrix cell was toggled: (row, col, new_value).
    pub onroute_toggled: EventHandler<(i32, i32, bool)>,
}

// ---------------------------------------------------------------------------
// NetworkPanel
// ---------------------------------------------------------------------------

#[component]
pub fn NetworkPanel(props: NetworkPanelProps) -> Element {
    rsx! {
        Overlay {
            visible: props.visible,
            title: "网络设置".to_string(),
            icon: Some(IconName::Network),
            variant: OverlayVariant::Large,
            onclose: move |evt| props.onclose.call(evt),

            NetworkPanelContent {
                network_status: props.network_status.clone(),
                remote_controlled: props.remote_controlled,
                executing_remotely: props.executing_remotely,
                scanning: props.scanning,
                allow_remote_control: props.allow_remote_control,
                audio_muted: props.audio_muted,
                peers: props.peers.clone(),
                connected_peer_index: props.connected_peer_index,
                matrix_size: props.matrix_size,
                peer_names: props.peer_names.clone(),
                my_index: props.my_index,
                matrix_cells: props.matrix_cells.clone(),
                onconnect: props.onconnect,
                ondisconnect: props.ondisconnect,
                onscan: props.onscan,
                ontoggle_remote_control: props.ontoggle_remote_control,
                ontoggle_mute: props.ontoggle_mute,
                onroute_toggled: props.onroute_toggled,
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct NetworkPanelContentProps {
    // -- network state --
    pub network_status: String,
    #[props(default)]
    pub remote_controlled: bool,
    #[props(default)]
    pub executing_remotely: bool,
    #[props(default)]
    pub scanning: bool,
    #[props(default)]
    pub allow_remote_control: bool,
    #[props(default)]
    pub audio_muted: bool,

    // -- peer list --
    #[props(default)]
    pub peers: Vec<PeerDisplayInfo>,
    /// Index of the currently connected peer, or -1 if none.
    #[props(default)]
    pub connected_peer_index: i32,

    // -- routing matrix --
    /// Number of nodes in the matrix (N for an N*N grid).
    #[props(default)]
    pub matrix_size: i32,
    /// Display names for each node column/row header.
    #[props(default)]
    pub peer_names: Vec<String>,
    /// Index of the local user's row in the matrix, or -1 if absent.
    #[props(default)]
    pub my_index: i32,
    /// Flat N*N boolean grid (row-major). Cell at (row, col) is
    /// `matrix_cells[row * matrix_size + col]`.
    #[props(default)]
    pub matrix_cells: Vec<bool>,

    // -- event handlers --
    /// Connect to the peer identified by its stringified NodeId.
    pub onconnect: EventHandler<String>,
    /// Disconnect from the peer identified by its stringified NodeId.
    pub ondisconnect: EventHandler<String>,
    pub onscan: EventHandler<MouseEvent>,
    pub ontoggle_remote_control: EventHandler<MouseEvent>,
    pub ontoggle_mute: EventHandler<MouseEvent>,
    /// A routing matrix cell was toggled: (row, col, new_value).
    pub onroute_toggled: EventHandler<(i32, i32, bool)>,
}

#[component]
pub fn NetworkPanelContent(props: NetworkPanelContentProps) -> Element {
    // -- status text & colour --
    let status_text = if props.network_status.is_empty() {
        "未连接".to_string()
    } else {
        props.network_status.clone()
    };

    let status_class = if props.executing_remotely {
        "network-info-row__text executing-remotely"
    } else if props.remote_controlled {
        "network-info-row__text remote-controlled"
    } else {
        "network-info-row__text connected"
    };

    // -- matrix cell helper --
    let matrix_size = props.matrix_size as usize;
    let cell_at = move |row: usize, col: usize| -> bool {
        if row < matrix_size
            && col < matrix_size
            && row * matrix_size + col < props.matrix_cells.len()
        {
            props.matrix_cells[row * matrix_size + col]
        } else {
            false
        }
    };

    rsx! {
        div { class: "network-panel-content",
            // ---- Connection status row ----
            div {
                class: "network-info-row",
                span {
                    class: "network-info-row__icon",
                    Icon { name: IconName::Network }
                }
                span {
                    class: status_class,
                    "{status_text}"
                }
            }

            // ---- Allow remote control toggle ----
            ToggleSwitch {
                on: props.allow_remote_control,
                label: "允许远程控制".to_string(),
                icon: Some(IconName::Lock),
                on_toggle: move |evt| props.ontoggle_remote_control.call(evt),
            }

            // ---- Mute mode toggle ----
            ToggleSwitch {
                on: props.audio_muted,
                label: "静音模式".to_string(),
                icon: Some(IconName::VolumeMuted),
                on_toggle: move |evt| props.ontoggle_mute.call(evt),
            }

            // ---- Peer list ----
            PanelSection {
                title: "已发现的节点".to_string(),
                icon: Some(IconName::Users),
                class: Some("network-section network-section--peers".to_string()),

                div {
                    class: "peer-list",

                    if props.peers.is_empty() {
                        div {
                            class: "peer-list__empty",
                            "暂无发现的节点"
                        }
                    } else {
                        for peer in props.peers.iter() {
                            {
                                let is_connected = peer.is_connected;
                                let row_class = if is_connected {
                                    "peer-row peer-row--connected"
                                } else {
                                    "peer-row"
                                };
                                let latency_display = if peer.latency_ms < 0 {
                                    "-".to_string()
                                } else {
                                    format!("{}ms", peer.latency_ms)
                                };
                                let node_id = peer.node_id_string.clone();
                                let node_id_for_disconnect = node_id.clone();

                                rsx! {
                                    div {
                                        class: "{row_class}",

                                        div {
                                            class: "peer-row__info",

                                            span {
                                                class: if is_connected { "peer-row__status-icon peer-row__status-icon--connected" } else { "peer-row__status-icon" },
                                                if is_connected {
                                                    Icon { name: IconName::Check }
                                                }
                                            }

                                            span {
                                                class: "peer-row__name",
                                                "{peer.name}"
                                            }

                                            span {
                                                class: "peer-row__address",
                                                "{peer.address}"
                                            }

                                            span {
                                                class: "peer-row__latency",
                                                "{latency_display}"
                                            }
                                        }

                                        div {
                                            class: "peer-row__actions",

                                            if is_connected {
                                                button {
                                                    class: "network-action-btn",
                                                    r#type: "button",
                                                    onclick: move |_| props.ondisconnect.call(node_id_for_disconnect.clone()),
                                                    "断开"
                                                }
                                            } else {
                                                button {
                                                    class: "network-action-btn",
                                                    r#type: "button",
                                                    onclick: move |_| props.onconnect.call(node_id.clone()),
                                                    "连接"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ---- Routing matrix header with scan button ----
            PanelSection {
                title: "路由矩阵".to_string(),
                icon: Some(IconName::Network),
                class: Some("network-section network-section--matrix".to_string()),

                div { class: "network-toolbar",
                    button {
                        class: if props.scanning { "network-action-btn network-action-btn--scan is-active" } else { "network-action-btn network-action-btn--scan" },
                        r#type: "button",
                        title: "扫描网络 (F5)",
                        onclick: move |evt| props.onscan.call(evt),
                        Icon { name: IconName::Search, class: Some("network-action-btn__icon".to_string()) }
                        if props.scanning {
                            span { "扫描中..." }
                        } else {
                            span { "扫描" }
                        }
                    }
                }

                // ---- Routing matrix grid ----
                div {
                    class: "routing-matrix",

                    if props.matrix_size == 0 {
                        // Empty state
                        div {
                            class: "routing-matrix__empty",
                            "暂无节点连接"
                        }
                    } else {
                        // Matrix grid
                        div {
                            class: "routing-matrix__grid",
                            style: "grid-template-columns: var(--name-col-w) repeat({props.matrix_size}, var(--cell-w));",

                        // Corner cell (empty top-left)
                        div { class: "matrix-corner" }

                        // Column headers
                        for col in 0..matrix_size {
                            {
                                let is_self_col = col as i32 == props.my_index;
                                let header_class = if is_self_col {
                                    "matrix-col-header is-self"
                                } else {
                                    "matrix-col-header"
                                };
                                let col_name = props.peer_names.get(col).cloned().unwrap_or_default();
                                rsx! {
                                    div {
                                        class: "{header_class}",
                                        title: "{col_name}",
                                        "{col_name}"
                                    }
                                }
                            }
                        }

                        // Data rows
                        for row in 0..matrix_size {
                            {
                                let is_self_row = row as i32 == props.my_index;
                                let row_header_class = if is_self_row {
                                    "matrix-row-header is-self"
                                } else {
                                    "matrix-row-header"
                                };
                                let row_name = props.peer_names.get(row).cloned().unwrap_or_default();

                                rsx! {
                                    // Row header
                                    div {
                                        class: "{row_header_class}",
                                        title: "{row_name}",
                                        "{row_name}"
                                    }

                                    // Matrix cells for this row
                                    for col in 0..matrix_size {
                                        {
                                            let is_diagonal = row == col;
                                            let is_editable = is_self_row && !is_diagonal;
                                            let checked = cell_at(row, col);

                                            let cell_class = if is_diagonal {
                                                "matrix-cell is-diagonal"
                                            } else if is_editable {
                                                "matrix-cell is-editable"
                                            } else {
                                                "matrix-cell is-readonly"
                                            };

                                            let checkbox_class = if checked {
                                                if is_self_row {
                                                    "matrix-checkbox matrix-checkbox--checked-self"
                                                } else {
                                                    "matrix-checkbox matrix-checkbox--checked"
                                                }
                                            } else {
                                                "matrix-checkbox"
                                            };

                                            let row_i32 = row as i32;
                                            let col_i32 = col as i32;

                                            rsx! {
                                                button {
                                                    class: "{cell_class}",
                                                    r#type: "button",
                                                    disabled: !is_editable,
                                                    aria_checked: if checked { "true" } else { "false" },
                                                    onclick: move |_evt: MouseEvent| {
                                                        if is_editable {
                                                            props.onroute_toggled.call((row_i32, col_i32, !checked));
                                                        }
                                                    },

                                                    div {
                                                        class: "{checkbox_class}",
                                                        if checked && !is_diagonal {
                                                            Icon { name: IconName::Check, class: Some("matrix-checkbox__mark".to_string()) }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        }
                    }
                }
            }
        }
    }
}
