# Auto Media

Rust + Tauri 桌面自动发布工具。详细方案见 `system_design.md`。

## 目录

- `src`: Rust/Tauri 源码和静态前端。
- `bin`: release 二进制输出目录。
- `conf`: 配置、状态库、认证状态和浏览器 profile。
- `data`: 待发布图片目录。
- `logs`: 运行日志目录。

## 构建

```powershell
cargo check
cargo test
cargo build --release
Copy-Item target\release\auto_media.exe bin\auto_media.exe -Force
```

## 当前进度

- 已实现 Tauri GUI、系统托盘、开机自启动注册、配置加载、日志、SQLite 状态库、10 分钟调度、20:00 后跳过扫描、前一天图片扫描、防重复发布状态。
- 小红书和知乎都已接入 CDP 浏览器启动骨架，使用独立浏览器 profile 打开登录页和发布页。
- 平台页面的选择器级自动填充、上传和点击发布仍集中在 `src/platforms/cdp_adapter.rs` 中继续深化。
