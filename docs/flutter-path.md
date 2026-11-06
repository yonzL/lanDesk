# Flutter 界面路线（先决条件与差距）

目标：正式版用 Flutter（上游 RustDesk 的那套 UI 资产），当前先用 eframe 交付。
本文记录**为什么这件事不是"换个 UI"**，以及要做的具体步骤与顺序。

## 1. 现状事实

| 项 | 现状 |
|---|---|
| `flutter/` 目录 | 上游完整 Flutter 应用（含桌面/移动、i18n、设置页、会话页、工具栏） |
| Rust↔Dart 桥 | `flutter_rust_bridge` **v1**，版本锁死：`Cargo.toml` 的 `flutter_rust_bridge = "=1.80"`、`flutter/pubspec.yaml` 的 `1.80.1` |
| 生成的桥文件 | `flutter/lib/generated_bridge.dart`、`src/bridge_generated*.rs` 都是 gitignore 的，CI 里由 codegen 生成 |
| Flutter SDK | `D:\dev_ai\_tools\flutter.zip`（1.9GB），未解包，`flutter` 不在 PATH |
| 我们的 FFI 面 | `src/flutter_ffi.rs` 是上游的；**landesk 的能力（按地址开会话、地址簿 CRUD、密钥/身份、文件分发）都不在里面** |
| 我们现在的 UI | eframe/egui（约 1400 行），已实现 QQ 形态、画质/分辨率/剪贴板/声音开关、文件分发、身份管理 |

## 2. 三条必须先解决的事（按依赖顺序）

### ① 工具链：codegen + SDK

- `cargo install flutter_rust_bridge_codegen --version 1.80.1 --locked`
  （v1 的 codegen 是普通 crate；需要 `LIBCLANG_PATH`。AGENTS.md 说"不跑 codegen"是因为版本难对齐，
  但 1.80.1 可以直接装——这一点待本机验证，见第 4 节）
- 解包 SDK，`flutter config --enable-windows-desktop`，`flutter doctor`
- iOS/Android/macOS 一律不做：与"Windows 局域网远控"无关

### ② FFI 面：把 landesk 能力按 `flutter_ffi` 的形状暴露

上游 Flutter 调的是 `bind.mainXxx()` / `bind.sessionXxx(sessionId, ...)`，其中"会话"用 `SessionID` 索引，
由 `sessions` 注册表维护。我们现在的 `landesk::session::SessionHandle` 是独立的一套。

要让 Flutter UI 能用，需要：

| 需要的能力 | 对应上游形状 | 我们要加的 |
|---|---|---|
| 按 `地址:端口` 开会话 | `session_start` 系列 | `landesk_session_start(addr, control) -> SessionID`，并把它注册进 `sessions` |
| 会话输入/画面 | `session_send_mouse/key`、帧回调 | 复用我们已有的；帧改走 `flutter::push_global_event` |
| 地址簿 | 上游没有（按 ID 存的是最近会话） | 新 FFI：`landesk_book_list/upsert/remove/import_csv`，返回 JSON |
| 身份/密钥 | 无 | `landesk_key_list/import/forget`，`landesk_identity_default` |
| 文件分发 | 无 | `landesk_push(peers, files, dir) -> 进度 JSON` |
| 监听状态（服务端侧没有 UI，但主窗口可展示） | 无 | 可选 |

关键判断：**不要碰上游 `flutter/lib` 里的连接逻辑**，只加"landesk 页"（地址簿作为首页），
把 `home_page`/设备列表替换成我们的地址簿。SubnetDesk 就是这么做的（`lan_ui.dart` + 一套 LAN 页面）。

### ③ 编译形态

- Rust 侧：`cargo build --features flutter`（会编 `flutter_ffi`、`bridge_generated`）
- Dart 侧：`flutter build windows --release`
- 打包：两个 exe（`landesk-server.exe` 与 `landesk-client.exe` 都要带 `--features flutter` 才能用同一套会话内核）

## 3. 工作量与风险（诚实估计）

| 阶段 | 内容 | 估计 |
|---|---|---|
| A | 打通 codegen + SDK + 让上游 Flutter 应用**原样**编起来 | 半天～1 天（工具链不确定性最高） |
| B | 加 landesk FFI + 地址簿页替换首页 | 1～2 天 |
| C | 会话页接我们已跑通的会话（画面/输入/剪贴板/音频/文件） | 1～2 天（大部分逻辑已存在，主要是桥接） |
| D | 删掉 Flutter 侧不再需要的页面/依赖（中继、ID、更新、账号） | 半天 |

风险点：FRB v1 与当前 Dart/Flutter 版本的兼容；上游 Flutter 应用对 SDK 版本有要求（3.24.5 / Dart 3.5.4，
见 SubnetDesk 的 win7 说明）。

## 4. 本次验证记录

- 安装 codegen、解包 SDK 的实际结果：见第 6 节。


## 6. 实测结论（本机，2026-11）

| 步骤 | 结果 |
|---|---|
| `cargo install flutter_rust_bridge_codegen --version 1.80.1 --locked` | ✅ 72 秒装好（USTC 镜像）；AGENTS.md 里"版本难配"的结论在我们这套环境不成立 |
| 解包 Flutter SDK | ✅ `D:\dev_ai\_tools\_flutter_extract\flutter`；`flutter --version` → **Flutter 3.47.9 / Dart 3.13.7** |
| codegen 解析 `src/flutter_ffi.rs` | ✅ 生成 `src/bridge_generated.rs` + `.io.rs` |
| codegen 的 **ffigen** 步骤 | ❌ 失败且无输出（需要 Dart 侧的 ffigen 包，pub 访问在本机不通） |

所以 `--features flutter` 现在剩下的闸门是 **ffigen**（Dart 侧 C 头生成），与我们的源码无关。

把守方式（不依赖 codegen）：默认构建 `cargo check`、`session_smoke` 五个测试、冒烟四条路径；flutter-only 文件的语法用
`rustfmt --edition 2021 --check src/flutter_ffi.rs src/ui_interface.rs` 检查（它会解析文件，语法坏块会报错）。

做 Flutter 时的下一步：

1. 配 `PUB_HOSTED_URL=https://pub.flutter-io.cn` 或可用代理，让 ffigen 能 `dart pub get`；
2. 重跑 `flutter_rust_bridge_codegen`，确认生成物完整（"DUMMY CODE FOR BINDGEN" 块必须是注释）；
3. 再谈第 2 节的 FFI 面与地址簿页替换。

另外：这一轮清掉了一个隐形隐患——`flutter_ffi.rs` / `ui_interface.rs` 里**只在 `--features flutter` 下编译**的代码，
长期带着指向已删模块的引用（`hbbs_http::account`、`hbbs_http::downloader`），默认构建永远看不见。
这些云钩子现在已删，`--features flutter` 不会再报 E0433。
