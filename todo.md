# lanDesk 待办与现状

本文是**工作追踪**，不是规格。设计上的决定与边界在 `goal-and-requirements.md` 与 `architecture.md`；这里只回答"做完什么了、还剩什么"。

最近更新：试用包 `dist/landesk-20261105.zip` 打完之后。

## 已完成

### 仓库与设计

- [x] 两份 durable spec 建立并随实现演化：`goal-and-requirements.md`（能力、非目标、替代方案）、`architecture.md`（拓扑、身份、会话、并发、模块取舍、已知边界、不变量）
- [x] RustDesk 基线导入：上游 `23e5bb4`，`libs/hbb_common` 由子模块改为内置（握手协议要改它的 protobuf）
- [x] AGPL-3.0 许可、README（状态 / 构建前置 / 两台机器试用指南 / 拒绝原因对照表）
- [x] CI：`cargo check -p hbb_common --all-targets`（`windows-latest`），多次运行成功
- [x] 大陆网络下的完整构建链路摸通并写进 README（USTC crates 镜像、GitHub git 源经代理 + openssl 后端、`chromium.googlesource.com` 子模块指向 GitHub 镜像、vcpkg 的 `opus/libvpx/aom/libyuv`、`libclang`）

### 身份（landesk 的差异所在）

- [x] `keyauth`（在 `libs/hbb_common`）：ed25519 双向证明、TOFU 指纹库、`AuthNonce`/`AuthProof`/`AuthOk` 三条消息、拒绝原因与日志行 — 15 个单测
- [x] 服务端直连监听（`--landesk-listen`，默认 21118）与强制执行点；未绑定公钥时连 Kx 都不做
- [x] 服务端在 landesk 模式下的四处放行：密码不参与、不比对设备 ID、不启动连接管理器、跳过上游 rendezvous 并由监听接管进程存活
- [x] Master 侧拨号（`--landesk-connect` / `--landesk-view`）：密钥交换 → 主机身份 → 回证明 → 等 `AuthOk`
- [x] 双进程冒烟脚本 `scripts/landesk-smoke.sh`：持钥两连成功、错钥被拒并留一行日志、指纹变更被拒、断言无出站

### 地址簿与配置

- [x] 地址簿 `addressbook.json`：组/子组两级、条目（显示名 + `地址:端口`）、指纹内嵌同一份文件
- [x] CSV 导入/导出（中英文表头、引号转义、按 `地址:端口` 覆盖式合并、全量校验后才落盘）
- [x] `--landesk-book-list` / `--landesk-book-verify`（TOFU 的"人工确认"那一步）
- [x] `landesk.json` + `--landesk-save-config`：让上游 `--install-service` 能在不额外改安装流程的情况下带 landesk 参数自启

### 会话（能看到、能操作）

- [x] 会话桥：已认证连接交给上游 `Remote`/`Session`，画面经 `InvokeUiSession::on_rgba` 交付（带递增序号，避免每帧 8MB 空拷）
- [x] 图形界面 `landesk-gui`：地址簿树、连接/只看、远端画面、断开、会话日志
- [x] 鼠标（移动/按下/抬起/滚轮）、可打印字符、修饰键（Ctrl/Shift/Alt/Meta 状态变化）、常见控制键与 F1–F12/Insert/Space
- [x] 多显示器切换、会话结束后回到可重连状态、会话提示进界面日志
- [x] **端到端测试** `tests/session_smoke.rs`：起真 Service、收真帧、持续推流、输入不报错、对端消失后会话自行结束（约 2 秒，测试串行化、`LANDESK_LOG=debug` 可取详细日志）
- [x] 关键 bug 修复：会话入口改用上游 `ui_session_interface::io_loop`——之前用内层 `Remote` 导致 `Session::sender` 从未接上、**所有输入被静默丢弃**

### 打包

- [x] `scripts/dist.sh` → `dist/landesk-<日期>.zip`：两个 exe + 一页 `使用说明.txt`，静态 CRT + 静态 vcpkg，不依赖 DLL
- [x] 干净目录实测发布包：genkey → save-config → 无参 `--server` 读配置监听 → CLI 拨号 → GUI 启动

## 未完成

按建议顺序排列；每条都写了"为什么还没做"与"从哪开始"。

### 高优先（影响试用体验）

- [ ] **文件传输**
  没做的原因（实测确认，已写进 `architecture.md`）：服务端只在**专设的文件传输会话**（`LoginRequest::FileTransfer`，`ConnType::FILE_TRANSFER`）里处理 `FileAction`；同一会话里发 `Data::SendFiles` 会被对端静默忽略。
  从哪开始：仿 `src/ui_session_interface.rs` 里 file-transfer 会话的构造，另起一个 `Session`，配套一套传输列表/进度/覆盖确认界面（回调 `update_folder_files`、`job_progress`、`confirm_delete_files`、`override_file_confirm` 目前是空实现）。
- [ ] **剪贴板的两机验证**
  没做的原因：同机两进程共享一个系统剪贴板，本地无法区分"真的同步了"与"本来就是一个剪贴板"。上游客户端的剪贴板循环已随会话启动，`on_texture`/`clipboard` 之类的 UI 回调在 Windows 上不是必需。
  从哪开始：两台机器上复制一段带标记的文本，双向各试一次；若不通，检查 `disable-clipboard` 选项与 `ClipboardSide` 的日志。
- [ ] **`--install-service` 实测**
  没做的原因：需要管理员权限，当前开发环境无法提权。
  从哪开始：在目标机上 `rustdesk.exe --landesk-save-config …` 后 `--install-service`，重启确认自启、确认服务读的是 exe 同目录的 `landesk.json`。

### 中优先（补齐能力）

- [ ] 音频、终端（上游会发，界面回调是空实现）
- [ ] 输入法/死键/IME 组合输入（现在只有可打印字符 + 控制键）
- [ ] 画质/FPS/编码策略的界面开关（上游 `OptionMessage` 支持，界面未暴露）
- [ ] 地址簿的图形化编辑（现在增删改靠 CSV 与命令行）

### 低优先（工程与发布）

- [ ] GitHub Release 发布 `dist/*.zip`（当前 `dist/` 是 gitignore 的，需要 token 才能发）
- [ ] 代码签名（未签名会被 SmartScreen 拦）
- [ ] 把主 crate 的构建也纳入 CI（需要 vcpkg + libclang，成本较高；目前主 crate 只在本地验证）
- [ ] 上游公网模块的物理删除（`rendezvous_mediator`、`hbbs_http`、`updater*`、`ui_cm_interface` 等）
  现状：landesk 模式**已不启动**它们，但代码仍在树里。删除点集中在会话与鉴权核心（`server/connection.rs`、`ipc.rs`、`flutter_ffi.rs`），需要能编译验证时才动。
- [ ] 用 Flutter 重做 Master 界面（SDK 已下载在 `D:\dev_ai\_tools\flutter.zip`，1.9GB；当前是纯 Rust 的 eframe 窗口）

## 已知边界与风险

- **一把全局私钥全网通用**：泄漏就要给所有机器换公钥。这是明确接受的取舍，写在 goal 里。
- **服务端不做并发仲裁**：多个 Master 同时操作会抢鼠标，与上游同形。
- **私钥保护只有文件系统 ACL**（与 Veyon 同路），DPAPI 是可选的加固，未实现。
- **主机指纹首次信任（TOFU）**：首次连接时操作者需要自己核对并 `--landesk-book-verify`，否则一直是"未验证"状态。
- **界面未做人工交互测试**：点击、拖拽、真实键鼠注入的效果只经过协议层与自动化测试，未做真人操作验证。
