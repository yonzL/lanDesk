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
| 我们现在的 UI | eframe/egui（约 1400 行），已实现 QQ 形态、画质/编码器/剪贴板/声音开关、文件分发、身份管理 |

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

## 7. 更新：先决条件已打通（本机实测）

之前的"ffigen 卡住"已解决，原因不是 ffigen 本身，而是 **pub 的包源**：

1. `flutter pub get` 必须能让 pub 访问网络。用 `PUB_HOSTED_URL=https://pub.flutter-io.cn` 会失败，报
   `Downloaded archive for image_picker_ios-0.8.13+15 had wrong content-hash`（镜像包的哈希是坏的）。
   **改用 pub.dev + `HTTPS_PROXY`/`https_proxy` 就通过**：
   ```bash
   export PATH="/path/to/flutter/bin:$PATH" LIBCLANG_PATH="C:\Program Files\LLVM\bin"
   export HTTPS_PROXY=http://127.0.0.1:7890 https_proxy=http://127.0.0.1:7890   # 大小写都要
   export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=http.sslBackend GIT_CONFIG_VALUE_0=openssl \
          GIT_CONFIG_KEY_1=http.proxy GIT_CONFIG_VALUE_1=http://127.0.0.1:7890   # pubspec 里有 GitHub git 依赖
   cd flutter && flutter pub get
   ```
2. 然后 codegen 完整成功（含 ffigen 与 Dart 格式化）：
   ```
   flutter_rust_bridge_codegen --rust-input ./src/flutter_ffi.rs --dart-output ./flutter/lib/generated_bridge.dart
   → Success! 生成 src/bridge_generated.rs + .io.rs + flutter/lib/generated_bridge.dart(447KB)
   ```
3. `cargo check -p rustdesk --lib --features flutter` → **通过**（只有上游 warning）。这是本项目第一次让 flutter 特性参与编译。

### 之后剩下的（都不是 codegen）

| 阻碍 | 状态 |
|---|---|
| SDK 版本 | ✅ 已按最新 Flutter（3.47.9）适配：`DialogTheme`/`TabBarTheme` → `...ThemeData`，4 处 |
| 登录/账号页 | ✅ 已删（`login.dart` 及 5 个使用点），`flutter analyze lib` 0 error |
| Developer Mode | ✅ 已开启；`flutter build windows` 已经越过这一步 |
| **VS “使用 C++ 的桌面开发” 工作负载** | ❌ 缺这个：`vcvars64.bat` 与 CMake 都没有，因此 `flutter build windows` 报 `Unable to find suitable Visual Studio toolchain`，Rust 侧的 C 依赖（`kcp-sys`/`ring`/`scrap`）也编不了 |

## 8. 推进：改到最新 Flutter（3.47.9）

决定：不在 3.24 上停留，直接把 Dart 侧改到最新 SDK。

已完成：

- **主题 API 改名**（4 处，`lib/common.dart`）：`DialogTheme` → `DialogThemeData`、`TabBarTheme` → `TabBarThemeData`，字段名不变（`elevation`/`shape`/`labelColor`），外观不变。
- **删掉云账号 UI**：`login.dart`（OAuth 登录对话框）删除，并清掉它的 5 个使用点——
  `toolbar.dart` 的 "Note" 菜单、`my_group.dart` 与 `address_book.dart` 的登录按钮、
  桌面设置页的整个 `Account` 页签、移动设置页的 Account 段与 note 选项、`web_model.dart` 给 JS 注册的 `loginDialog`。
  两处列表页改成说明文字：*lanDesk keeps machines in addressbook.json beside the exe; this window does not list them yet.*
- **`flutter analyze lib` → 0 error**（362 条 info/warning 是上游既有的 deprecated/风格提示）。
  剩下 3 个 error 在 `test/floating_mouse_hold_test.dart`：它用 `js.allowInterop`，新版 Dart 已移除；这是上游的 browser-only 测试，我们不跑它，**未改动**。

下一步（需要工具链/权限）：

1. `cargo build --lib --features flutter` 产出 `target/debug/librustdesk.dll`（Windows 的 CMake 从这里拷）；
2. `flutter build windows --debug`：带插件构建需要机器开启 **Developer Mode**（symlink 支持），本机当前无管理员权限，需手动开启；
3. 之后才是真正的 landesk 化：地址簿 FFI（`landesk_book_list` / `landesk_session_start`）+ 用地址簿替换 `my_group.dart` / `address_book.dart` 的列表。

### 阶段 A 的最后一堵墙：Developer Mode

`flutter build windows --debug` 在 `flutter pub get` 之后会停在：

```
Building with plugins requires symlink support.
Please enable Developer Mode in your system settings.
```

Flutter 在 Windows 上把每个插件的平台实现软链到 `windows/flutter/ephemeral/.plugin_symlinks`，
这需要开发者模式（或管理员）。一次性设置：

```powershell
start ms-settings:developers     # 打开"开发人员模式"
# 或管理员 PowerShell：
reg add "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock" /t REG_DWORD /f /v AllowDevelopmentWithoutDevLicense /d 1
```

（同一次构建还会刷一堆 `file_picker` 的 "references file_picker:windows as the default plugin"
告警——那是它新版联邦插件布局的自引用提示，属噪音，与失败无关。）

## 9. FFI 面已就位（阶段 B 的后端部分）

界面需要的东西集中在 `src/landesk/ffi.rs`（**默认构建**里，可被 `cargo check`/测试覆盖），
`flutter_ffi.rs` 只有同名薄封装：

| 函数 | 返回 |
|---|---|
| `book_list()` | 地址簿 JSON 数组：`group/subgroup/name/addr/key/pin(verified\|unverified\|never)/fingerprint` |
| `book_upsert(group, subgroup, name, addr, key)` | 空串 = 成功；`error:…` = 失败（地址必须带端口） |
| `book_remove(addr)` / `book_verify(addr)` | 同上；删除会连指纹一起丢掉 |
| `identity_list()` | 可用身份 JSON 数组：`{name, path, public}` |
| `listen_status()` | `landesk-status.json` 的内容（没有则 `null`） |
| `open_remote_window(addr)` | 起一个 `--landesk-remote <addr>` 子进程；一个会话一个进程 |

**会话不在界面进程里开**，所以 Flutter 不需要接 `sessions` 注册表——它只需要调 `open_remote_window`，
和现在 eframe 的做法完全一致（这也是"每会话一个进程"决定的直接好处）。

剩下的 Dart 侧工作只剩页面：用 `book_list` 替换 `my_group.dart` / `address_book.dart` 的列表，
双击调 `open_remote_window`，设置页加一个"身份"区块（`identity_list` / import / forget）。

## 10. 陷阱：`pub get` 会改写两个文件

跟上面这些步骤跑一遍后，`flutter/` 里会有两个**本机工具链产生**的改动，不要当成分支决定提交：

- `flutter/pubspec.lock`：3.47 的解析结果（`analyzer` 6.7.0 → 6.4.1，去掉 `_macros`）；
- `flutter/analysis_options.yaml`：被注入 `analyzer: exclude:`（`build/**`、各平台目录）。

上一轮就是它们混进了文档提交，`e67e96a` 把它们回滚掉的。跑完 `pub get` 后先 `git checkout` 这两个文件。

## 11. 本机的构建入口（cc-rs / CMake 都要 MSVC 环境）

两个环境事实决定了怎么跑构建，写成一个脚本最省事（脚本本身不入库，属于个人环境）：

```bat
@echo off
rem vcvars64 会把 VCPKG_ROOT 改成 VS 自带的那个空 vcpkg，所以先存后恢复。
set "LD_VCPKG=%VCPKG_ROOT%"
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
if defined LD_VCPKG set "VCPKG_ROOT=%LD_VCPKG%"
cd /d D:\dev_ai\landesk
cargo %*
```

调用方式（bash 里）：

```bash
export VCPKG_ROOT="D:\dev_ai\_tools\vcpkg" LIBCLANG_PATH="C:\Program Files\LLVM\bin"
cmd //c "C:\...\cargo-msvc.bat test -p rustdesk --lib"
```

为什么必须这样：

1. **vcvars64 覆盖 `VCPKG_ROOT`**：它会把它设成 `…\VC\vcpkg`（里面没有我们的 opus/libvpx/aom/libyuv），
   于是 `magnum-opus` 报 `fatal error C1083: 'opus/opus_multistream.h' file not found`。必须先存后恢复。
2. **Git Bash 的 `link.exe`（coreutils）会抢在 MSVC linker 前面**：报 `link: extra operand`。
   vcvars 会把 MSVC 的 `bin\Hostx64\x64` 放进 PATH 前面，正好解决。
3. 构建缓存会记住旧的错误结果：改了 `VCPKG_ROOT` 后要先 `cargo clean -p magnum-opus -p scrap`。
