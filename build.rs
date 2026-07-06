use std::env;
use std::fs;
use std::path::PathBuf;

use skera::{Plan, SubsetFlags, subset_font};
use write_fonts::read::{
    FontRef,
    collections::IntSet,
    types::{NameId, Tag},
};

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let font_dir = PathBuf::from("resource/fonts");

    // Rebuild if source fonts change
    println!("cargo:rerun-if-changed=resource/fonts/NotoSansSC-Regular.otf");
    println!("cargo:rerun-if-changed=resource/fonts/JetBrainsMonoNerdFont-Regular.ttf");

    // ---- NotoSansSC: subset to ~80 CJK chars used in the UI + Rust runtime ----
    let cjk_chars: Vec<char> = "语音计算器普通已启用已连接等待授权远程控制无设备音效个正常内存指示器设置网络关于名称保存已保存不能为空连接超时被拒绝中断不可达访问失败显示静音全清除退格乘除加减等于小数点百分号根号记忆召回清除存储溢出输入无效不能以零错误社区许可证界面精度搞怪音乐扫描中暂无节点连接路由矩阵允许模式设备输入名称本机远程未知保存失败已连接未连接个音效音频正常"
        .chars()
        .collect();
    subset_font_with_skera(
        &font_dir.join("NotoSansSC-Regular.otf"),
        &out_dir.join("NotoSansSC-Regular.subset.otf"),
        &cjk_chars,
    );

    // ---- JetBrainsMonoNerdFont: ASCII printable + Nerd Font glyphs used in UI ----
    let mut nerd_chars: Vec<char> = Vec::new();
    // ASCII printable (space through tilde)
    for c in 0x20u32..=0x7E {
        nerd_chars.push(char::from_u32(c).unwrap());
    }
    // Font Awesome / Nerd Font glyphs still used by Dioxus UI text fallbacks.
    let nerd_codepoints: &[u32] = &[
        0xF001, // fa-music
        0xF007, // fa-user
        0xF00D, // fa-xmark (close)
        0xF013, // fa-gear (settings)
        0xF023, // fa-lock
        0xF026, // fa-volume-xmark (mute)
        0xF027, // fa-volume-low
        0xF028, // fa-volume-high
        0xF05A, // fa-circle-info (about)
        0xF0AC, // fa-globe (network)
        0xF0C2, // fa-cloud
        0xF0E7, // fa-bolt (remote exec)
        0xF185, // fa-moon (dark mode)
        0xF186, // fa-sun (light mode)
    ];
    for &cp in nerd_codepoints {
        nerd_chars.push(char::from_u32(cp).unwrap());
    }
    // Additional symbols used in UI
    nerd_chars.extend([
        '\u{221A}', // square root
        '\u{232B}', // backspace
        '\u{2713}', // checkmark
        '\u{00D7}', // multiplication sign
        '\u{00F7}', // division sign
        '\u{00B1}', // plus-minus sign
    ]);
    subset_font_with_skera(
        &font_dir.join("JetBrainsMonoNerdFont-Regular.ttf"),
        &out_dir.join("JetBrainsMonoNerdFont-Regular.subset.ttf"),
        &nerd_chars,
    );
}

/// Subset a font using the skera crate (Google Fonts general-purpose subsetter).
/// Preserves cmap, GSUB, GPOS and all tables needed for UI rendering.
fn subset_font_with_skera(src: &std::path::Path, dst: &std::path::Path, chars: &[char]) {
    let data = fs::read(src).unwrap_or_else(|e| panic!("failed to read {}: {e}", src.display()));
    let font = FontRef::new(&data)
        .unwrap_or_else(|e| panic!("failed to parse font {}: {e:?}", src.display()));

    // Build the unicode set
    let mut unicodes = IntSet::<u32>::empty();
    for &c in chars {
        unicodes.insert(c as u32);
    }

    let gids = IntSet::empty();

    // Drop variable-font tables that static fonts don't need (and skera may
    // panic on when the tables are present but vestigial).
    let drop_tables: IntSet<Tag> = [
        Tag::new(b"HVAR"),
        Tag::new(b"VVAR"),
        Tag::new(b"gvar"),
        Tag::new(b"STAT"),
        Tag::new(b"avar"),
        Tag::new(b"cvar"),
        Tag::new(b"MVAR"),
    ]
    .iter()
    .copied()
    .collect();

    // Retain all layout scripts (required for CJK shaping)
    let mut layout_scripts = IntSet::<Tag>::empty();
    layout_scripts.invert();

    // Retain all layout features (kern, liga, mark, mkmk, etc.)
    let mut layout_features = IntSet::<Tag>::empty();
    layout_features.invert();

    // Retain name IDs 0..=6 (copyright through description)
    let mut name_ids = IntSet::<NameId>::empty();
    name_ids.insert_range(NameId::from(0)..=NameId::from(6));

    // Default language: English + Simplified Chinese
    let mut name_languages = IntSet::<u16>::empty();
    name_languages.insert(0x0409); // English (US)
    name_languages.insert(0x0804); // Chinese (Simplified)

    let plan = Plan::new(
        &gids,
        &unicodes,
        &font,
        SubsetFlags::SUBSET_FLAGS_NO_HINTING,
        &drop_tables,
        &layout_scripts,
        &layout_features,
        &name_ids,
        &name_languages,
    );

    let subset = subset_font(&font, &plan)
        .unwrap_or_else(|e| panic!("failed to subset {}: {e:?}", src.display()));

    fs::write(dst, &subset).unwrap_or_else(|e| panic!("failed to write {}: {e}", dst.display()));
}
