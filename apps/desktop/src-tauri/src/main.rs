// 不设这一行时，Windows 会给程序分配一个**控制台** —— 用户看到的就是
// "程序开着，另一个终端窗口一直挂着；关掉程序，那个窗口也跟着关"。
//
// 这一行标的是 PE 子系统（GUI 而非 console）。注意它是**rustc 的 crate 属性**，
// 不是 Cargo.toml 的 manifest key —— 写成 `[[bin]] windows_subsystem = "windows"`
// 会被 cargo 报 `unused manifest key: bin.0.windows_subsystem`（实测踩过：
// 那个警告被淹在编译日志里，很容易以为已经生效了）。
//
// ⚠ 只对**这个 crate 的二进制**生效，且必须在 `main.rs` 上，不能写在 lib.rs。
//
// ⚠⚠ **不要加 `cfg_attr(not(debug_assertions), ...)`**（实测：加了等于没加）。
// 我上一轮以为"调试时保留控制台"是个体贴的设计，于是包成
// `cfg_attr(not(debug_assertions), windows_subsystem = "windows")` ——
// 结果 `--release` 打出来的 exe **仍然是 console 子系统**，照旧弹终端窗口。
// 而验证那一步又读错了文件（读了 `target/release/` 下一个 9 月 30 日的旧产物，
// 恰好是 GUI 的），于是"验证通过、交付了一个没修好的包"。
//
// 教训有两条，都记在这儿：
//  1. 这个属性上 **`cfg_attr` 不可靠** —— 用不带条件的写法。
//  2. 验证必须读**当次构建的那个产物**：`tauri build --target X` 出的在
//     `target/X/release/`，不是 `target/release/`。后者可能躺着上一个别的配置的旧文件。
#![windows_subsystem = "windows"]

fn main() {
    notera_desktop_lib::run()
}
