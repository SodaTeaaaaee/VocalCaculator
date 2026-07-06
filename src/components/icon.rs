use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconName {
    Bolt,
    Check,
    Info,
    Moon,
    Music,
    Network,
    Settings,
    Sun,
    VolumeLow,
    VolumeHigh,
    VolumeMuted,
    X,
}

#[derive(Props, Clone, PartialEq)]
pub struct IconProps {
    pub name: IconName,
    #[props(default)]
    pub class: Option<String>,
}

#[component]
pub fn Icon(props: IconProps) -> Element {
    let class = props
        .class
        .as_deref()
        .map(|name| format!("vc-icon {name}"))
        .unwrap_or_else(|| "vc-icon".to_string());

    match props.name {
        IconName::Bolt => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M13 2 3 14h8l-1 8 10-12h-8l1-8Z" }
            }
        },
        IconName::Check => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "m20 6-11 11-5-5" }
            }
        },
        IconName::Info => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                circle { cx: "12", cy: "12", r: "10" }
                path { d: "M12 16v-4" }
                path { d: "M12 8h.01" }
            }
        },
        IconName::Moon => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M12 3a6 6 0 0 0 9 7 9 9 0 1 1-9-7Z" }
            }
        },
        IconName::Music => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M9 18V5l12-2v13" }
                circle { cx: "6", cy: "18", r: "3" }
                circle { cx: "18", cy: "16", r: "3" }
            }
        },
        IconName::Network => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                rect { x: "16", y: "16", width: "6", height: "6", rx: "1" }
                rect { x: "2", y: "16", width: "6", height: "6", rx: "1" }
                rect { x: "9", y: "2", width: "6", height: "6", rx: "1" }
                path { d: "M5 16v-3a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v3" }
                path { d: "M12 8v8" }
            }
        },
        IconName::Settings => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z" }
                path { d: "M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06A1.7 1.7 0 0 0 15 19.4a1.7 1.7 0 0 0-1 .6 1.7 1.7 0 0 0-.4 1.1V21a2 2 0 1 1-4 0v-.08A1.7 1.7 0 0 0 8.6 19.4a1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-.6-1 1.7 1.7 0 0 0-1.1-.4H3a2 2 0 1 1 0-4h.08A1.7 1.7 0 0 0 4.6 8.6a1.7 1.7 0 0 0-.34-1.87l-.06-.06A2 2 0 1 1 7.03 3.84l.06.06A1.7 1.7 0 0 0 9 4.6c.37-.14.7-.35 1-.6.25-.3.4-.7.4-1.1V3a2 2 0 1 1 4 0v.08A1.7 1.7 0 0 0 15.4 4.6a1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.7 1.7 0 0 0 19.4 9c.14.37.35.7.6 1 .3.25.7.4 1.1.4H21a2 2 0 1 1 0 4h-.08a1.7 1.7 0 0 0-1.52.6Z" }
            }
        },
        IconName::Sun => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                circle { cx: "12", cy: "12", r: "4" }
                path { d: "M12 2v2" }
                path { d: "M12 20v2" }
                path { d: "m4.93 4.93 1.41 1.41" }
                path { d: "m17.66 17.66 1.41 1.41" }
                path { d: "M2 12h2" }
                path { d: "M20 12h2" }
                path { d: "m6.34 17.66-1.41 1.41" }
                path { d: "m19.07 4.93-1.41 1.41" }
            }
        },
        IconName::VolumeLow => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M11 5 6 9H3v6h3l5 4V5Z" }
                path { d: "M15.5 8.5a5 5 0 0 1 0 7" }
            }
        },
        IconName::VolumeHigh => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M11 5 6 9H3v6h3l5 4V5Z" }
                path { d: "M15.5 8.5a5 5 0 0 1 0 7" }
                path { d: "M18.5 5.5a9 9 0 0 1 0 13" }
            }
        },
        IconName::VolumeMuted => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M11 5 6 9H3v6h3l5 4V5Z" }
                path { d: "m16 9 5 5" }
                path { d: "m21 9-5 5" }
            }
        },
        IconName::X => rsx! {
            svg { class, view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M18 6 6 18" }
                path { d: "m6 6 12 12" }
            }
        },
    }
}
