# lanDesk 待办与现状

本文是**工作追踪**，不是规格。设计上的决定与边界在 `goal-and-requirements.md` 与 `architecture.md`；这里只回答"做完什么了、还剩什么"。

最近更新：把无界面连接管理器（CM）、剪贴板双向、编码器默认值修完之后。

## 已完成

### 仓库与设计

- [x] 两份 durable spec 建立并随实现演化：`goal-and-requirements.md`（能力、非目标、替代方案）、`architecture.md`（拓扑、身份、会话、并发、模块取舍、已知边界、不变量）
- [x] RustDesk 基线导入：上游 `23e5bb4`，`libs/hbb_common` 由子模块改为内置（握手协议要改它的 protobuf）
- [x] AGPL-3.0 许可、README（状态 / 构建前置 / 两台机器试用指南 / 拒绝原因对照表）
- [x] **产品命名统一**：交付物为 `landesk-server.exe`（被控端 + 命令行）与 `landesk-client.exe`（图形界面）；内部 app name 改为 `lanDesk`（配置目录、日志、服务名、IPC 管道都跟着它，并因此自动关掉上游更新检查）；安装路径改为按当前 exe 名字推导，不再假设 `{app_name}.exe`
- [x] CI：`cargo check -p hbb_common --all-targets`（`windows-latest`），多次运行成功
- [x] 大陆网络下的完整构建链路摸通并写进 README（USTC crates 镜像、GitHub git 源经代理 + openssl 后端、`chromium.googlesource.com` 子模块指向 GitHub 镜像、vcpkg 的 `opus/libvpx/aom/libyuv`、`libclang`）

### 身份（landesk 的差异所在）

- [x] `keyauth`（在 `libs/hbb_common`）：ed25519 双向证明、TOFU 指纹库、`AuthNonce`/`AuthProof`/`AuthOk` 三条消息、拒绝原因与日志行 — 15 个单测
- [x] 服务端直连监听（`--landesk-listen`，默认 21118）与强制执行点；未绑定公钥时连 Kx 都不做
- [x] 服务端在 landesk 模式下的放行：密码不参与、不比对设备 ID、跳过上游 rendezvous 并由监听接管进程存活
- [x] Master 侧拨号（`--landesk-connect` / `--landesk-view`）：密钥交换 → 主机身份 → 回证明 → 等 `AuthOk`
- [x] 双进程冒烟脚本 `scripts/landesk-smoke.sh`：持钥两连成功、错钥被拒并留一行日志、指纹变更被拒、断言无出站

### 地址簿与配置

- [x] 地址簿 `addressbook.json`：组/子组两级、条目（显示名 + `地址:端口`）、指纹内嵌同一份文件
- [x] **剪贴板传文件**：会话里自动开启上游 `enable-file-copy-paste`，在资源管理器复制文件即可到远端粘贴落盘；对端确认这一项有测试覆盖
- [x] CSV 导入/导出（中英文表头、引号转义、按 `地址:端口` 覆盖式合并、全量校验后才落盘）
- [x] `--landesk-book-list` / `--landesk-book-verify`（TOFU 的"人工确认"那一步）
- [x] `landesk.json` + `--landesk-save-config`：让上游 `--install-service` 能在不额外改安装流程的情况下带 landesk 参数自启

### 会话（能看到、能操作）

- [x] 会话桥：已认证连接交给上游 `Remote`/`Session`，画面经 `InvokeUiSession::on_rgba` 交付（带递增序号，避免每帧 8MB 空拷）
- [x] 图形界面 `landesk-client`：地址簿树、连接/只看、远端画面、断开、会话日志
- [x] 鼠标（移动/按下/抬起/滚轮）、可打印字符、修饰键（Ctrl/Shift/Alt/Meta 状态变化）、常见控制键与 F1–F12/Insert/Space
- [x] 多显示器切换、会话结束后回到可重连状态、会话提示进界面日志
- [x] **端到端测试** `tests/session_smoke.rs`：起真 Service、收真帧、持续推流、输入不报错、对端消失后会话自行结束（约 2 秒，测试串行化、`LANDESK_LOG=debug` 可取详细日志）
- [x] 关键 bug 修复：会话入口改用上游 `ui_session_interface::io_loop`——之前用内层 `Remote` 导致 `Session::sender` 从未接上、**所有输入被静默丢弃**

### 打包

- [x] `scripts/dist.sh` → `dist/landesk-<日期>.zip`：两个 exe + 一页 `使用说明.txt`，静态 CRT + 静态 vcpkg，不依赖 DLL
- [x] 干净目录实测发布包：genkey → save-config → 无参 `--server` 读配置监听 → CLI 拨号 → GUI 启动

### 试用期修掉的问题（真机三台机测试驱动）

- [x] **无界面连接管理器**（`--landesk-cm`）：用户令牌进程、只监听 `_cm`、不画窗口、不参与授权。绕过它的代价是文本剪贴板读不到用户桌面——上游靠 CM 干这件事是有理由的
- [x] **文本剪贴板双向通**（用户实测）：客户端侧自建轮询（上游 `clipboard_listener` 在非 Flutter UI 里不报变更），服务端读回经 CM
- [x] **Ctrl+C/X/V**：egui-winit 把这三个吐成 `Event::Copy/Cut/Paste`，不再发按键事件；自己组包后可用
- [x] **编码器默认值真生效**：原来写在 session 未 initialize 的配置上，登录消息里 `prefer` 一直是 Auto（服务端选了最慢的 AV1）；改为直写 `PeerConfig[peer]`，默认 vp9
- [x] **编码线程退化路径**：Windows 读不到 CPU 占用时上游退到单线程，改为按一半核数
- [x] **帧率问题的真相**：客户端一直是 debug 构建（同机 2560×1600：debug 3.5 fps / release 26.1 fps），fps 控制只是把这个事实放大成“越调越卡”
- [x] 架构笔记 `docs/upstream-architecture.md`（链路图 + 日志位置 + fps 螺旋 + 编码线程闸）与 `docs/subnetdesk-notes.md`（同类产品取舍对比）
- [x] **来源网络白名单（默认只收私有网段）**：`src/landesk/net.rs`，accept 后按 CIDR 判断，公网来源在握手前丢弃；`--landesk-allow-network` / `landesk.json` 的 `allowed_networks` 可替换默认值
- [x] **配置读取顺序 bug**：`public.key` 自动读取排在 `landesk.json` 之前，导致目录里有这个文件时整份配置被跳过（自定义端口静默失效）；已改为先读 JSON、再用 `public.key` 兜底

## 未完成

按建议顺序排列；每条都写了"为什么还没做"与"从哪开始"。

### 高优先（影响试用体验）

- [x] **文件分发（扇出）**——已完成：同一批文件同时推给多台 Service（每台一个独立会话并行 + 逐台进度），文件操作支持删除/改名（同目录移动）/新建目录，下载也可用。
  “启动文件”按决定砍掉。
  实现要点（供后来者）：无界面 Service 用上游 `handle_fs` 在本进程执行文件操作、结果经 `local_reply` 回投；上传约定是 `to`=目标目录+文件名、`file_num=0`；landesk 模式不启动查询 IPC，因此同机可并存多个 Service。
- [ ] **剪贴板的两机验证**
  现状：客户端剪贴板循环随会话启动，文本与文件通道都已就位（文件通道有测试覆盖到“对端确认开关”这一步）；同机无法区分“真的同步”与“本来就是一个剪贴板”。
  从哪开始：两台机器上分别复制一段带标记的文本、一个文件，双向各试一次；若文本不通，检查 `disable-clipboard` 选项与 `ClipboardSide` 的日志。
- [ ] **`--install-service` 实测**
  没做的原因：需要管理员权限，当前开发环境无法提权。
  已做的准备：安装路径改为按**当前 exe 名字**推导（不再拼 `{app_name}.exe`），且 landesk 不再重命名 exe；app name 已改为 `lanDesk`（配置目录、服务名、IPC 管道都跟着它）。
  从哪开始：在目标机上 `landesk-server.exe --landesk-save-config …` 后 `--install-service`，重启确认自启、确认服务读的是 exe 同目录的 `landesk.json`。

### 中优先（补齐能力）

- [ ] 监听改成监督循环：配置变了自动重启监听，并对外暴露 `stopped/starting/listening/failed` 状态（现在改端口必须重启进程）

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
