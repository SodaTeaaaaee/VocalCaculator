use dioxus::prelude::*;

use crate::components::overlay::{Overlay, ToggleSwitch};

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
            title: "\u{F0AC} \u{7F51}\u{7EDC}\u{8BBE}\u{7F6E}", // globe + "网络设置"
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
    let mut scan_hover = use_signal(|| false);

    // -- status text & colour --
    let status_text = if props.network_status.is_empty() {
        "\u{672A}\u{8FDE}\u{63A5}".to_string() // "未连接"
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
                    "\u{F0AC}"
                }
                span {
                    class: status_class,
                    "{status_text}"
                }
            }

            // ---- Allow remote control toggle ----
            ToggleSwitch {
                on: props.allow_remote_control,
                label: "\u{F023} \u{5141}\u{8BB8}\u{8FDC}\u{7A0B}\u{63A7}\u{5236}", // lock + "允许远程控制"
                on_toggle: move |evt| props.ontoggle_remote_control.call(evt),
            }

            // ---- Mute mode toggle ----
            ToggleSwitch {
                on: props.audio_muted,
                label: "\u{F026} \u{9759}\u{97F3}\u{6A21}\u{5F0F}", // volume-off + "静音模式"
                on_toggle: move |evt| props.ontoggle_mute.call(evt),
            }

            // ---- Peer list ----
            div {
                class: "settings-section-header",

                span {
                    class: "settings-section-header__text",
                    "\u{F0C0} \u{5DF2}\u{53D1}\u{73B0}\u{7684}\u{8282}\u{70B9}" // "已发现的节点"
                }
            }

            div {
                class: "peer-list",

                if props.peers.is_empty() {
                    div {
                        class: "peer-list__empty",
                        "\u{6682}\u{65E0}\u{53D1}\u{73B0}\u{7684}\u{8282}\u{70B9}" // "暂无发现的节点"
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
                                "\u{2014}".to_string() // em-dash for unknown
                            } else {
                                format!("{}ms", peer.latency_ms)
                            };
                            let status_icon = if is_connected {
                                "\u{F00C}" // check-circle
                            } else {
                                "\u{F111}" // circle (disconnected)
                            };
                            let node_id = peer.node_id_string.clone();
                            let node_id_for_disconnect = node_id.clone();

                            rsx! {
                                div {
                                    class: "{row_class}",

                                    div {
                                        class: "peer-row__info",

                                        span {
                                            class: "peer-row__status-icon",
                                            "{status_icon}"
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
                                                class: "skeu-btn skeu-btn--func",
                                                style: "width: 64px; font-size: max(9px, 2.2vh);",
                                                onclick: move |_| props.ondisconnect.call(node_id_for_disconnect.clone()),
                                                "\u{65AD}\u{5F00}" // "断开"
                                            }
                                        } else {
                                            button {
                                                class: "skeu-btn skeu-btn--func",
                                                style: "width: 64px; font-size: max(9px, 2.2vh);",
                                                onclick: move |_| props.onconnect.call(node_id.clone()),
                                                "\u{8FDE}\u{63A5}" // "连接"
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
            div {
                class: "settings-section-header",

                span {
                    class: "settings-section-header__text",
                    "\u{8DEF}\u{7531}\u{77E9}\u{9635}" // "路由矩阵"
                }

                div { class: "settings-save-row__spacer" }

                button {
                    class: if *scan_hover.read() { "skeu-btn skeu-btn--func skeu-btn--keyboard-active" } else { "skeu-btn skeu-btn--func" },
                    style: "width: 56px; font-size: max(9px, 2.2vh);",
                    title: "扫描网络 (F5)",
                    onmouseenter: move |_| scan_hover.set(true),
                    onmouseleave: move |_| scan_hover.set(false),
                    onclick: move |evt| props.onscan.call(evt),
                    if props.scanning {
                        "\u{626B}\u{63CF}\u{4E2D}..." // "扫描中..."
                    } else {
                        "\u{626B}\u{63CF}" // "扫描"
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
                        "\u{6682}\u{65E0}\u{8282}\u{70B9}\u{8FDE}\u{63A5}" // "暂无节点连接"
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
                                                div {
                                                    class: "{cell_class}",
                                                    onclick: move |_evt: MouseEvent| {
                                                        if is_editable {
                                                            props.onroute_toggled.call((row_i32, col_i32, !checked));
                                                        }
                                                    },

                                                    div {
                                                        class: "{checkbox_class}",
                                                        if checked && !is_diagonal {
                                                            span {
                                                                class: "matrix-checkbox__mark",
                                                                "\u{2713}" // checkmark
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
