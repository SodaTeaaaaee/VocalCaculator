# Vocal Calculator Research Sync

更新日期：2026-06-08

本文档是主 goal 的研究同步摘要。
执行模型必须把这里的结论视为“已完成的一手资料核验”，除非后续再次联网核实并形成更新记录。

## 1. 核心结论

### 1.1 GUI 结论

最终 GUI 主栈选择：`Slint`

原因：

1. 官方文档明确支持 Desktop 与 Android
2. Android 仅支持 Rust，这正符合本项目需求
3. 保留模式、声明式 UI、动画、平台样式都具备
4. 比 `Iced` 更适合当前“Windows + Android + 原生 retained-mode + 弱模型执行”的目标

关键来源：

1. Slint 总览：https://docs.slint.dev/
2. Slint Rust API：https://docs.slint.dev/latest/docs/rust/slint/
3. Slint Android Rust API：https://docs.slint.dev/latest/docs/rust/slint/android/index.html
4. Slint Android 平台页：https://docs.slint.dev/latest/docs/slint/guide/platforms/mobile/android/
5. Slint Desktop 平台页：https://docs.slint.dev/latest/docs/slint/guide/platforms/desktop/
6. Slint 动画：https://docs.slint.dev/latest/docs/slint/guide/language/coding/animation/
7. Slint 状态：https://docs.slint.dev/latest/docs/slint/guide/language/coding/states/
8. Slint 样式：https://docs.slint.dev/latest/docs/slint/reference/std-widgets/style/

### 1.2 为什么不是 Iced

`Iced` 官方仓库仍把自己描述为 `experimental software`。

关键来源：

1. Iced 仓库：https://github.com/iced-rs/iced

### 1.3 为什么不是 Tauri

`Tauri v2` 的官方文档在桌面打包、权限和测试方面很强，但它本质是 Web 前端 + Rust Core。
用户明确表达了尽量不用 Web 技术栈。
因此本项目不采用 Tauri。

关键来源：

1. Tauri 进程模型：https://v2.tauri.app/concept/process-model/
2. Tauri 前端模型：https://v2.tauri.app/start/frontend/
3. Tauri 测试页：https://v2.tauri.app/develop/tests/

### 1.4 音频结论

最终高层音频栈选择：`kira`

原因：

1. `StaticSoundData` 适合短音频
2. 可从文件或 `Cursor` 载入
3. 每次播放返回独立 handle
4. 支持 `stop(Tween)`、`pause(Tween)`、`resume(Tween)` 等控制
5. 与 Android 的 `cpal` 后端链路可兼容

关键来源：

1. Kira crate：https://docs.rs/kira/latest/kira/
2. Kira sound 模块：https://docs.rs/kira/latest/kira/sound/
3. StaticSoundData from_cursor 源码页：https://docs.rs/kira/latest/src/kira/sound/static_sound/data/from_file.rs.html
4. StaticSoundHandle：https://docs.rs/kira/latest/kira/sound/static_sound/struct.StaticSoundHandle.html
5. CPAL 平台支持：https://docs.rs/crate/cpal/latest

### 1.5 数值结论

最终计算数值栈选择：`rust_decimal`

原因：

1. 明确面向金融/定点十进制
2. 避免二进制浮点舍入误差
3. 支持宏与字符串序列化特性

关键来源：

1. rust_decimal：https://docs.rs/rust_decimal/latest/rust_decimal/

### 1.6 配置目录结论

最终目录栈选择：`sysdirs`

原因：

1. 明确支持 Windows 与 Android
2. Android 支持自动初始化或显式初始化
3. API 接近 `dirs`

关键来源：

1. sysdirs：https://docs.rs/sysdirs
2. sysdirs latest：https://docs.rs/crate/sysdirs/latest

### 1.7 Android 打包结论

最终 Android 打包工具选择：`cargo-apk`

原因：

1. 明确面向原生 Rust `cdylib`
2. 支持构建、运行、ADB 安装
3. 开发配置下可使用默认 debug keystore

关键来源：

1. cargo-apk 仓库：https://github.com/rust-mobile/cargo-apk

### 1.8 Slint 许可结论

Slint 官方说明：

1. Desktop / Mobile / Web 可免费使用 Community License
2. 但需要 attribution

关键来源：

1. Slint get started：https://slint.dev/get-started
2. Slint Community License 页面：https://slint.dev/get-community-license
3. Slint Royalty-free License PDF：https://slint.dev/agreements/slint-royalty-free-license.pdf

## 2. AR7778 外部仓库结论

目标仓库：

`https://github.com/evnchn-AR7778/AR7778-digitized-MIDI`

截至 2026-06-08 的本地核验结论：

1. HEAD commit：`485356dbc36900f0582661a6d60f0d934bb43e52`
2. 目录存在 `MIDIs/`、`Soundfont files/`、`Waveform/`
3. 仓库未发现明确 `LICENSE`
4. README 未授予再分发许可
5. `Process.txt` 说明了用 Polyphone + FluidSynth 的制作链

因此：

1. 可以工程上兼容该仓库
2. 不应默认把其资产视为可公开再分发
3. 音乐模式应优先只依赖 `256.sf2`
4. 不应依赖上游歌曲 MIDI 作为产品功能资源

关键来源：

1. 仓库主页：https://github.com/evnchn-AR7778/AR7778-digitized-MIDI
2. README：https://github.com/evnchn-AR7778/AR7778-digitized-MIDI/blob/main/README.md
3. Process.txt：https://raw.githubusercontent.com/evnchn-AR7778/AR7778-digitized-MIDI/main/Process.txt
4. GitHub 许可说明：https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository

## 3. 计算器规则参考

### 3.1 百分比

CASIO 官方百分比计算示例支持：

1. `A × B%`
2. `A 是 B 的百分之几`
3. `A 增加 B%`
4. `A 减少 B%`

关键来源：

1. CASIO 百分比说明：https://support.casio.com/global/en/calc/manual/fx-82MS_85MS_220PLUS_300MS_350MS_en/basic_calculations/percent_calculations.html
2. CASIO 百分比中文页：https://support.casio.com/global/tw/calc/manual/fx-100MS_570MS_991MS_tw/basic_calculations/percent_calculations.html

### 3.2 MU

CASIO FAQ 明确说明：

1. `[MU]` 用于 markup
2. `120 [MU] 25 [%]` => `160`
3. 再按 `=` => `40`

关键来源：

1. CASIO MU FAQ：https://support.casio.com/en/support/answer.php?cid=004001001001&num=4&qid=68326

## 4. 风险摘要

### 4.1 可接受风险

1. Windows 无代码签名
2. Android 可能先只产出 debug/dev APK
3. Linux/macOS/iOS 先不作为强制完成项

### 4.2 不可接受风险

1. 无 license 资产被当作“默认可公开发布资源”
2. 浮点实现计算器主逻辑
3. 为了省事重写成 Web UI
4. 运行时依赖磁盘读取短 WAV
5. 运行时模糊匹配损坏音频文件名

