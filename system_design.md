# Auto Media 系统设计

## 1. 目标与边界

本项目用 Rust 实现一个带界面的桌面自动发布工具，首版目标如下：

- 支持 Windows 桌面运行，提供 GUI 和系统托盘。
- 支持小红书、知乎两个平台的登录状态检测、手机验证码登录引导、图片发布。
- 每 10 分钟醒来一次，在本地时间晚上 8 点前扫描 `data` 目录中“前一天”的图片；找到图片后自动发布到两个平台；晚上 8 点后只记录跳过并继续睡眠。
- 开机自启动，但不以 Windows Service 形式运行；自启动后默认最小化到托盘。
- 文章标题固定为 `挑战千万美金 - YYYYMMDD`，正文以图片为主；如果平台要求文字正文，则填充最小文本 `一张图片`。
- 设计先落到本文档，review 通过后再开始编码。

明确边界：

- 不绕过验证码、风控、滑块或平台安全校验；遇到额外验证时让用户在登录窗口中手动完成。
- 小红书和知乎的写入接口都存在非官方或页面自动化依赖，首版统一采用 CDP 浏览器自动化，并按“可维护、可观察、失败可恢复”设计，不假设页面和接口永久稳定。
- 默认一天同一张目标图片在同一平台只发布一次，避免循环重复发帖。

## 2. 外部调研依据

- Tauri v2 支持系统托盘，Rust 侧可用 `TrayIconBuilder` 创建托盘图标和菜单：<https://v2.tauri.app/learn/system-tray/>
- Tauri v2 提供 `tauri-plugin-autostart`，可在 Windows/macOS/Linux 上注册开机自启动：<https://v2.tauri.app/plugin/autostart/>
- 小红书社区项目 `redbook` 说明其写入能力依赖非官方创作者 API、Cookie 和签名系统，并提示接口可能变更：<https://github.com/lucasygu/redbook>
- 小红书社区项目 `XiaohongshuSkills` 走 Chrome DevTools Protocol，支持图文发布、登录检测和登录状态缓存，并注明页面选择器需要随平台改版维护：<https://github.com/white0dew/XiaohongshuSkills>
- 知乎自动发文项目 `zhihuSender` 使用 Cookie 与任务表思路，说明发布任务、Cookie、任务状态需要拆开管理：<https://github.com/NeekChaw/zhihuSender>

结论：首版统一使用 Rust 后端驱动浏览器自动化/CDP 完成登录状态读取和发文。原因是手机验证码登录、HTTP-only Cookie、CSRF、图片上传和页面风控都更适合在真实浏览器会话里处理。后续如果需要更高稳定性或更快发布速度，再把小红书/知乎的非官方 HTTP API 作为可插拔适配器追加。

## 3. 技术选型

- GUI：Tauri v2。
- 后端：Rust + Tokio async runtime。
- 前端：Tauri 内置 WebView，使用轻量 HTML/TypeScript 或 Svelte 均可；首版倾向轻量 HTML/TypeScript，降低工程复杂度。
- 浏览器自动化：Rust CDP 客户端，优先评估 `chromiumoxide` 或 `headless_chrome`；需要可连接本机 Edge/Chrome 或启动独立 Chromium 实例。
- 配置：`serde` + `toml`。
- 状态：`rusqlite`，状态库放在 `conf/state.sqlite`。
- 日志：`tracing` + `tracing-appender`，按天滚动到 `logs`。
- 密钥/Cookie：Windows 优先使用 DPAPI 或系统 Keychain；配置目录仅保存加密后的认证状态和非敏感元数据。
- 开机启动：`tauri-plugin-autostart`，Windows 下注册当前用户启动项，不创建 Windows Service。

## 4. 目录结构

应用源码全部放在 `src` 下；Cargo/Tauri 必要的工程元数据可以留在项目根目录。编译产物通过构建脚本复制到 `bin`。

```text
D:\code\auto_media
├── Cargo.toml
├── tauri.conf.json
├── build.rs
├── src
│   ├── main.rs
│   ├── app.rs
│   ├── config.rs
│   ├── logging.rs
│   ├── scheduler.rs
│   ├── auth
│   │   ├── mod.rs
│   │   ├── cookie_store.rs
│   │   └── login_broker.rs
│   ├── browser
│   │   ├── mod.rs
│   │   └── cdp.rs
│   ├── platforms
│   │   ├── mod.rs
│   │   ├── xhs.rs
│   │   └── zhihu.rs
│   ├── publish
│   │   ├── mod.rs
│   │   ├── job.rs
│   │   └── image_scanner.rs
│   ├── startup.rs
│   ├── tray.rs
│   └── ui
│       ├── index.html
│       ├── main.ts
│       └── styles.css
├── bin
│   └── auto_media.exe
├── conf
│   ├── auto_media.toml
│   ├── state.sqlite
│   ├── browser_profiles
│   │   ├── xhs
│   │   └── zhihu
│   └── auth
│       ├── xhs.cookies.enc
│       └── zhihu.cookies.enc
├── data
└── logs
    └── auto_media.YYYY-MM-DD.log
```

说明：

- `src/ui` 是 Tauri 前端源码，仍属于 `src`。
- `bin/auto_media.exe` 由 `cargo build --release` 后复制生成，`target` 目录仍是 Cargo 临时构建目录，不作为交付目录。
- `conf/browser_profiles` 存平台隔离的浏览器 profile。
- `conf/auth/*.enc` 只存加密后的 Cookie/session，不存手机号和短信验证码。

## 5. 配置设计

`conf/auto_media.toml` 首版建议如下：

```toml
[app]
start_minimized = true
single_instance = true

[scheduler]
timezone = "Asia/Shanghai"
sleep_minutes = 10
cutoff_time = "20:00:00"
run_immediately_on_start = true

[data]
dir = "data"
image_patterns = [
  "{YYYYMMDD}*.jpg",
  "{YYYYMMDD}*.jpeg",
  "{YYYYMMDD}*.png",
  "{YYYYMMDD}*.webp",
  "{YYYY-MM-DD}*.jpg",
  "{YYYY-MM-DD}*.jpeg",
  "{YYYY-MM-DD}*.png",
  "{YYYY-MM-DD}*.webp"
]
multi_image_policy = "first_by_name"

[publish]
title_pattern = "挑战千万美金 - {YYYYMMDD}"
fallback_body_text = "一张图片"
publish_platforms = ["xhs", "zhihu"]

[platforms.xhs]
enabled = true
mode = "cdp"
login_url = "https://www.xiaohongshu.com"
creator_url = "https://creator.xiaohongshu.com"

[platforms.zhihu]
enabled = true
mode = "cdp"
login_url = "https://www.zhihu.com/signin"
write_url = "https://zhuanlan.zhihu.com/write"

[startup]
enabled = true
minimize_to_tray_on_autostart = true
```

待 review 确认的点：

- `data` 中图片命名是否就是按日期前缀匹配；如果实际是 `data/YYYYMMDD/*.jpg`，扫描器会改成目录优先。
- 同一天有多张图片时，首版默认取文件名排序第一张；也可以改成多张合成一篇或每张各发一篇。

## 6. 核心模块

### 6.1 Tauri Shell

职责：

- 创建主窗口、登录窗口和托盘菜单。
- 暴露前端命令：查看状态、手动触发检测、暂停/恢复、登录指定平台、打开目录、退出。
- 接收后端事件：需要登录、发布成功、发布失败、下一次唤醒时间、日志摘要。

主窗口展示：

- 当前运行状态：运行中 / 暂停 / 等待登录 / 发布中。
- 小红书、知乎登录状态。
- 最近一次扫描和发布结果。
- 下次唤醒时间。
- 开机自启动开关。
- 手动按钮：立即检测、登录小红书、登录知乎、打开 data、打开 logs。

托盘菜单：

- 打开主界面。
- 立即检测。
- 暂停/恢复。
- 登录小红书。
- 登录知乎。
- 退出。

窗口关闭行为：点击关闭按钮时隐藏到托盘；只有托盘“退出”才真正结束进程。

### 6.2 Scheduler

调度规则：

1. 应用启动后，如果 `run_immediately_on_start = true`，先执行一次 tick。
2. 之后循环 `sleep_minutes = 10` 分钟。
3. 每次醒来读取本地时间。
4. 若当前时间大于等于 `20:00:00`，记录 `skip_after_cutoff`，不扫描、不发布，继续睡眠。
5. 若当前时间早于 `20:00:00`：
   - 计算目标日期 `target_date = today - 1 day`。
   - 扫描 `data` 目录匹配目标日期的图片。
   - 没有图片则记录 `no_image`，继续睡眠。
   - 找到图片则生成发布任务并调用发布流水线。

伪代码：

```rust
loop {
    if should_run_now(now, cutoff_time) {
        let target_date = local_today(now) - Duration::days(1);
        let image = image_scanner.find_target_image(target_date)?;
        if let Some(image) = image {
            publisher.publish_daily_image(target_date, image).await;
        }
    } else {
        log::info!("after cutoff, skip scan");
    }

    tokio::time::sleep(Duration::from_secs(config.scheduler.sleep_minutes * 60)).await;
}
```

### 6.3 Image Scanner

输入：`target_date`。

输出：`Option<TargetImage>`。

匹配策略：

- 将 pattern 中的 `{YYYYMMDD}` 和 `{YYYY-MM-DD}` 替换为目标日期。
- 支持扩展名：`jpg/jpeg/png/webp`。
- 排除临时文件、隐藏文件、零字节文件。
- 如果多张匹配，按 `multi_image_policy` 处理：
  - `first_by_name`：按文件名排序取第一张。
  - `newest`：取修改时间最新。
  - `error`：多张时报错并等待人工处理。

### 6.4 Publish Pipeline

发布任务字段：

```text
job_id = sha256(target_date + image_path + image_size + image_mtime)
title = "挑战千万美金 - YYYYMMDD"
body_text = "一张图片"
image_path = data 下匹配到的图片
platforms = xhs, zhihu
```

流程：

1. 查 `conf/state.sqlite`，如果该 `job_id + platform` 已成功，跳过该平台。
2. 对每个平台顺序发布，降低风控风险。
3. 发布前先调用 `validate_session()`。
4. 如果未登录或 Cookie 失效，发出 `AuthRequired(platform)` 事件，弹出对应平台的浏览器登录窗口，等待用户完成手机验证码登录。
5. 登录成功后重新校验 session。
6. 调用平台适配器上传图片、填标题、填正文、发布。
7. 记录状态：
   - `pending`
   - `auth_required`
   - `publishing`
   - `success`
   - `failed`
   - `skipped_duplicate`

部分失败处理：

- 小红书成功、知乎失败时，只重试知乎。
- 每个平台记录独立错误、最后尝试时间、远端文章 URL。
- 后续每 10 分钟 tick 可以继续补发失败平台，但成功平台不会重复发布。

## 7. 登录与 Cookie 设计

### 7.1 登录原则

- 使用平台官方登录页。
- 用户手动输入手机号和短信验证码。
- 应用不保存手机号、短信验证码、密码。
- 不自动破解验证码、不绕过风控。
- 登录成功后只保存必要 Cookie/session，且本地加密。

### 7.2 Login Broker

`LoginBroker` 负责在登录失效时启动一个可见的浏览器登录窗口：

1. 后端启动独立浏览器实例，使用平台独立 profile：
   - `conf/browser_profiles/xhs`
   - `conf/browser_profiles/zhihu`
2. 通过 CDP 连接浏览器。
3. 打开对应平台登录页。
4. GUI 显示“请在弹出的登录窗口中使用手机号验证码完成登录”。
5. 用户完成短信验证码登录；如果出现滑块/人机验证，也由用户手动完成。
6. 后端轮询登录状态，成功后通过 CDP 读取包含 HTTP-only 的 Cookie。
7. Cookie 加密写入 `conf/auth/{platform}.cookies.enc`。
8. 关闭登录窗口或保留 profile 供后续复用。

这样可以满足“弹出登录界面让我输入登录信息、手机验证码登录”，同时避开 Tauri WebView 读取 HTTP-only Cookie 的限制。

### 7.3 Cookie 校验

每个平台实现：

```rust
async fn validate_session(&self) -> Result<SessionStatus>;
```

状态：

- `Valid { account_name }`
- `Expired`
- `Missing`
- `NetworkError`
- `RiskVerificationRequired`

校验失败时，不直接清空 Cookie；先标记为失效，等待用户重新登录。重新登录成功后覆盖旧 Cookie。

## 8. 平台适配器

统一接口：

```rust
#[async_trait]
pub trait PlatformAdapter {
    fn platform(&self) -> Platform;
    async fn validate_session(&self) -> Result<SessionStatus>;
    async fn login_interactive(&self) -> Result<SessionStatus>;
    async fn publish_image_article(&self, job: &PublishJob) -> Result<PublishResult>;
}
```

### 8.1 小红书适配器

首版方案：CDP 驱动小红书创作者中心网页。

流程：

1. 载入已加密保存的 Cookie 到浏览器 profile。
2. 打开创作者中心或发布页。
3. 检查是否已登录；未登录则触发 Login Broker。
4. 新建图文笔记。
5. 上传目标图片。
6. 填写标题：`挑战千万美金 - YYYYMMDD`。
7. 正文填 `一张图片`，或按平台允许仅图片。
8. 点击发布。
9. 等待发布成功提示或跳转结果，记录远端链接或发布结果。

后续可选增强：

- 将 `redbook`/`Spider_XHS` 中的创作者 API 签名逻辑移植到 Rust，做 `mode = "native_api"`。
- 保留 CDP 作为 fallback。

风险：

- 小红书创作者中心 DOM 可能改版，需要更新选择器。
- 账号风控、频率限制、内容审核会导致发布失败或延迟可见。

### 8.2 知乎适配器

首版方案：CDP 驱动知乎专栏写作页。

流程：

1. 载入已加密保存的 Cookie 到浏览器 profile。
2. 打开 `https://zhuanlan.zhihu.com/write`。
3. 检查是否已登录；未登录则触发 Login Broker。
4. 如登录页默认不是手机验证码模式，则通过 CDP 切换到手机号验证码登录。
5. 登录成功后进入写作页。
6. 填写标题：`挑战千万美金 - YYYYMMDD`。
7. 在正文区域插入图片；如编辑器要求文本，则添加 `一张图片`。
8. 点击发布。
9. 等待成功提示或文章 URL，记录结果。

后续可选增强：

- 参考浏览器 Network 请求和 CSRF 处理，追加 `mode = "native_api"`。
- CDP 保留为登录和异常验证 fallback。

风险：

- 知乎编辑器可能使用富文本内部结构，粘贴/上传图片需要针对 DOM 细节维护。
- 登录流程可能要求滑块、人机校验或其他额外验证，首版只承诺让用户在官方登录页完成。

## 9. 开机自启动与托盘

实现方案：

- 使用 Tauri autostart 插件注册当前用户启动项。
- 启动参数加 `--autostart`。
- 检测到 `--autostart` 时：
  - 创建托盘。
  - 不展示主窗口，或创建后立即隐藏。
  - 后台 scheduler 正常运行。
- 非自启动手动打开时展示主窗口。

不使用：

- Windows Service。
- 计划任务管理员权限模式。
- 后台常驻的独立守护进程。

## 10. 日志与可观测性

日志目录：`logs`。

日志内容：

- 应用启动、退出、版本。
- 配置加载结果。
- 每次 tick 的时间、是否跳过、目标日期。
- 图片扫描结果。
- 每个平台登录状态。
- 发布任务状态转换。
- 平台错误摘要和可恢复建议。

日志脱敏：

- 不输出 Cookie、手机号、验证码、完整请求头。
- 图片路径可以输出。
- 远端文章 URL 可以输出。

GUI 中显示最近 50 条摘要日志；完整日志从托盘或主窗口按钮打开 `logs` 目录查看。

## 11. 状态库设计

`conf/state.sqlite` 表：

```sql
CREATE TABLE publish_jobs (
  job_id TEXT PRIMARY KEY,
  target_date TEXT NOT NULL,
  title TEXT NOT NULL,
  image_path TEXT NOT NULL,
  image_size INTEGER NOT NULL,
  image_mtime INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE publish_platform_status (
  job_id TEXT NOT NULL,
  platform TEXT NOT NULL,
  status TEXT NOT NULL,
  remote_url TEXT,
  last_error TEXT,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (job_id, platform)
);

CREATE TABLE app_kv (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

用途：

- 防重复发布。
- 支持单平台失败后补发。
- GUI 展示最近状态。

## 12. 构建与交付

开发命令：

```powershell
cargo run
```

发布命令：

```powershell
cargo build --release
```

构建后处理：

- 将 `target\release\auto_media.exe` 复制到 `bin\auto_media.exe`。
- 若 Tauri 需要额外资源，资源随安装包或 `bin` 同级资源目录复制。
- 初始化默认 `conf\auto_media.toml`，不覆盖用户已有配置。
- 确保 `logs`、`data` 目录存在。

可以追加 `xtask` 或 `justfile` 做一键：

```powershell
cargo run -p xtask -- dist
```

## 13. 测试计划

单元测试：

- 时间窗口判断：20:00 前扫描，20:00 及以后跳过。
- `target_date = today - 1 day`。
- 图片 pattern 展开。
- 多图策略。
- job_id 和防重复逻辑。
- 配置加载和默认值。

集成测试：

- 使用 mock `PlatformAdapter` 验证小红书成功、知乎失败后的补发逻辑。
- 使用临时 `conf/state.sqlite` 验证状态落库。
- 使用临时 `data` 目录验证扫描器。

手动 E2E：

- 测试账号登录小红书，确认手机验证码登录、Cookie 保存、登录失效后弹窗。
- 测试账号登录知乎，确认手机验证码登录、Cookie 保存、登录失效后弹窗。
- 准备前一天图片，手动触发“立即检测”，确认两个平台各发布一次。
- 改系统时间或注入 clock，验证 20:00 后不扫描。
- 验证开机自启动后只进入托盘。

## 14. 已知风险与缓解

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 平台 DOM 改版 | 自动发布失败 | 选择器集中配置和日志定位；CDP 操作加截图/HTML dump 开关 |
| Cookie 失效 | 发布中断 | 每次发布前校验；失效即弹登录窗口 |
| 账号风控 | 需要人工验证或发布失败 | 不绕过验证；降低发布频率；失败后给出明确状态 |
| 同一天多图规则不明确 | 可能发错图 | 配置化 `multi_image_policy`，review 时确认 |
| 非官方 API 不稳定 | 维护成本上升 | 首版用官方页面自动化；后续 API 化作为可选适配器 |
| 本地 Cookie 泄露 | 账号风险 | DPAPI/Keychain 加密；日志脱敏；不保存验证码 |

## 15. Review 待确认

1. `data` 目录图片规则：按文件名前缀 `YYYYMMDD`，还是按子目录 `data/YYYYMMDD/`？
2. 同一天多张图片：取第一张、取最新、报错，还是多张一起发？
3. 正文是否需要包含文字 `一张图片`，还是只上传图片且无文字？
4. 首版是否接受 CDP/浏览器自动化作为发布方式，等稳定后再考虑移植非官方 HTTP API？
5. 发布失败是否需要当天持续补发，还是失败一次后等待手动“立即检测”？
