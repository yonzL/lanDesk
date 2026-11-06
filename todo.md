# lanDesk 待办与现状

本文是**工作追踪**，不是规格。设计上的决定与边界在 `goal-and-requirements.md` 与 `architecture.md`；技术细节的链路图在 `docs/upstream-architecture.md`、`docs/file-clipboard-chain.md`、`docs/subnetdesk-notes.md`。

最近更新：文件剪贴板（含 Ctrl+C/V）在 Win11 真机通过、监听监督循环落地之后。

## 已完成

### 仓库与设计

- [x] 两份 durable spec 随实现演化：`goal-and-requirements.md`（能力、非目标、替代方案）、`architecture.md`（拓扑、身份、会话、并发、模块取舍、已知边界、不变量）
- [x] RustDesk 基线导入：上游 `23e5bb4`，`libs/hbb_common` 由子模块改为内置（握手协议要改它的 protobuf）
- [x] AGPL-3.0 许可、README（状态 / 构建前置 / 两台机器试用指南 / 拒绝原因对照表）
- [x] **产品命名统一**：交付物 `landesk-server.exe`（被控端 + 命令行）与 `landesk-client.exe`（图形界面）；app name 改为 `lanDesk`（配置目录、日志、服务名、IPC 管道都跟着它，因此自动关掉上游更新检查）；安装路径按当前 exe 名推导
- [x] CI：`cargo check -p hbb_common --all-targets` 与主 crate、客户端的 `cargo check`（`windows-latest`）
- [x] 大陆网络下的完整构建链路摸通并写进 README（USTC crates 镜像、GitHub git 源经代理 + openssl 后端、子模块镜像、vcpkg 的 `opus/libvpx/aom/libyuv`、`libclang`）

### 身份（landesk 的差异所在）

- [x] `keyauth`（`libs/hbb_common`）：ed25519 双向证明、TOFU 指纹库、`AuthNonce`/`AuthProof`/`AuthOk`、`AUTH_VERSION` 版本比对、拒绝原因与日志行 — 15 个单测
- [x] 服务端直连监听（`--landesk-listen`，默认 21118）与强制执行点；未绑定公钥时连 Kx 都不做
- [x] landesk 模式的服务端放行：密码不参与、不比对设备 ID、跳过上游 rendezvous，由监听监督循环接管进程存活
- [x] Master 侧拨号（`--landesk-connect` / `--landesk-view`）：密钥交换 → 主机身份 → 回证明 → 等 `AuthOk`
- [x] 双进程冒烟脚本 `scripts/landesk-smoke.sh`：持钥两连成功、错钥被拒并留一行日志、指纹变更被拒、断言无出站

### 监听与运维能力

- [x] **来源网络白名单（默认只收私有网段）**：`src/landesk/net.rs`，accept 后按 CIDR 判断，公网来源在握手前丢弃；`--landesk-allow-network` / `landesk.json` 的 `allowed_networks` 替换默认值（`ipnet` 已在依赖树里）
- [x] **监听监督循环**（`src/transport.rs::supervise`）：绑定失败每秒重试并说明原因；`landesk.json` 的端口/白名单改了 1 秒内重绑，**已有会话不受影响**
- [x] **监听状态对外可见**：exe 同目录的 `landesk-status.json`（`starting`/`listening`/`failed` + 端口 + 原因 + 时间），`--landesk-doctor` 直接读它
- [x] **配置优先级**：命令行 > `landesk.json`（监督循环只在“监听参数来自文件”时才重读文件）
- [x] **配置读取顺序修复**：`public.key` 自动读取曾排在 `landesk.json` 之前，导致目录里有这个文件时整份配置被跳过（自定义端口静默失效）

### 地址簿与配置

- [x] 地址簿 `addressbook.json`：组/子组两级、条目（显示名 + `地址:端口`）、指纹内嵌同一份文件
- [x] CSV 导入/导出（中英文表头、引号转义、按 `地址:端口` 覆盖式合并、全量校验后才落盘）
- [x] `--landesk-book-list` / `--landesk-book-verify`（TOFU 的“人工确认”那一步）
- [x] `landesk.json` + `--landesk-save-config`：让上游 `--install-service` 能在不额外改安装流程的情况下带参数自启

### 会话（能看到、能操作）

- [x] 会话桥：已认证连接交给上游 `Remote`/`Session`，画面经 `InvokeUiSession::on_rgba` 交付（带递增序号，避免每帧 8MB 空拷；GUI 用 `Arc` 共享，不再每帧克隆）
- [x] 图形界面 `landesk-client`：地址簿树、连接/只看、远端画面、断开、会话日志、显示器切换、画质/FPS/编码器选择
- [x] 鼠标（移动/按下/抬起/滚轮，含右键）、可打印字符、修饰键、控制键与 F1–F12/Insert/Space
- [x] **端到端测试** `tests/session_smoke.rs`：起真 Service、收真帧、持续推流、输入不报错、文件会话全操作、双服务扇出
- [x] 关键修复：会话入口改用 `ui_session_interface::io_loop`（此前用内层 `Remote`，`Session::sender` 从未接上，**所有输入被静默丢弃**）

### 剪贴板与键盘（真机验证）

- [x] **文本剪贴板双向通**：两机实测。客户端侧自建轮询（上游 `clipboard_listener` 在非 Flutter UI 里零事件），服务端读回经 CM
- [x] **文件剪贴板双向通**：**Win11 实测通过**（右键复制粘贴 + Ctrl+C/V）
- [x] **无界面连接管理器**（`--landesk-cm`）：用户令牌进程、只听 `_cm`、不画窗口、不参与授权
- [x] 四处“与上游不一致”导致的静默失效，均按“读上游 → 逐文件 diff → 只改差异”修掉：
  1. `Session::default()` 里三个服务端权限标志是 false，而上游两个 UI 外壳构造时全置 true（`is_file_clipboard_required` 因此恒 false）
  2. landesk 分支自己授权后没有 `try_start_cm(..., authorized)`，CM 侧 `is_authorized`/`file_transfer_enabled` 恒 false，丢弃每条 `ClipboardFile`
  3. 编码器默认值写在未 initialize 的 session 配置上，登录消息 `prefer` 恒为 Auto（服务端选最慢的 AV1）
  4. `codec_thread_num` 在 CPU 占用读不到时退化成单线程
- [x] **egui 的 Ctrl+C/X/V**：它把三个组合键转成 `Event::Copy/Cut/Paste` 并 `return`；剪贴板里是文件时 `Paste` 连事件都不产生。改为：合成 down+up 键对；文件场景用 `GetAsyncKeyState` 轮询 Ctrl+V 兜底（仅在 `CF_HDROP` 存在且无 `CF_UNICODETEXT` 时触发，不重复发送）
- [x] 架构笔记：`docs/upstream-architecture.md`（链路 + 日志位置 + fps 螺旋 + 编码线程闸）、`docs/file-clipboard-chain.md`（完整前置条件表 + 差异审计 + egui 按键矩阵）

### 文件分发与安装

- [x] **文件分发（扇出）**：同一批文件同时推给多台 Service（每台一个独立会话并行 + 逐台进度），文件操作支持删除/改名（同目录移动）/新建目录，下载也可用。“启动文件”按决定砍掉
  实现要点：无界面 Service 用上游 `handle_fs` 在本进程执行；上传约定 `to`=目标目录+文件名、`file_num=0`；不启动查询 IPC，因此同机可并存多个 Service
- [x] **`--install-service` 实测**：Windows VM 上装成服务 `lanDesk`、开机自启、读 exe 同目录的 `landesk.json`，多轮升级部署都走它

### 打包

- [x] `scripts/dist.sh` → `dist/lanDesk-<日期>.zip`：两个 exe + 一页说明，静态 CRT + 静态 vcpkg，不依赖 DLL
- [x] 干净目录实测发布包：genkey → save-config（含白名单）→ 仅配置启动 → 监听 + 白名单生效 → doctor 显示正确 → CLI 拨号 → GUI 启动

## 未完成

按建议顺序排列；每条都写了“为什么还没做”与“从哪开始”。

### 高优先

- [ ] **Win10 那台恢复后补验**
  现状：文件剪贴板与快捷键只在 Win11 验过。Win10 整机掉线（ARP 表里没有、所有端口不通），需要在 VirtualBox 控制台把它拉起来，然后部署同一版并复验两条：远端复制文件 → 本机粘贴、本机复制文件 → 远端粘贴
- [ ] **低配被控机上的画面策略**
  现状：1080p + VP9 软编在 4 vCPU 的 VM 上约 10–20 fps；`image_quality=low` 只降码率不降分辨率。你说过“虚拟机画面不纠结”，所以这条是“要做的话怎么做”
  从哪开始：评估把“分辨率降档（1280×720）”做成界面开关——它比调画质对帧率有效得多，但会改变被控机显示设置

### 中优先（补齐能力）

- [ ] 音频、终端（上游会发，界面回调是空实现）
- [ ] 输入法/死键/IME 组合输入（现在只有可打印字符 + 控制键）
- [ ] 画质/FPS/编码策略的界面开关做完整（编码器与画质已有，FPS 与自定义码率还没暴露）
- [ ] 地址簿的图形化编辑（现在增删改靠 CSV 与命令行）
- [ ] 会话内“文件推送”页与地址簿的联动（现在要先选目标再选文件）

### 低优先（工程与发布）

- [ ] GitHub Release 发布 `dist/*.zip`（`dist/` 是 gitignore 的，需要 token）
- [ ] 代码签名（未签名会被 SmartScreen 拦）
- [ ] 把主 crate 的完整构建纳入 CI（需要 vcpkg + libclang，成本较高；目前主 crate 只在本地验证）
- [ ] 上游公网模块的物理删除（`rendezvous_mediator`、`hbbs_http`、`updater*` 等）
  现状：landesk 模式**已不启动**它们，但代码仍在树里；删除点集中在会话与鉴权核心，需要能编译验证时才动
- [ ] 用 Flutter 重做 Master 界面（SDK 已下载在 `D:\dev_ai\_tools\flutter.zip`；当前是纯 Rust 的 eframe 窗口）
- [ ] mDNS 局域网发现——**按决定不做**

## 已知边界与风险

- **验证必须用 release 构建**：这一轮最大的弯路是把 debug 客户端当性能基准（同机同分辨率 3.5 fps vs 26.1 fps）。以后报“卡”先确认是不是 release。
- **被控机算力决定帧率上限**：编码线程数取 `min(核数/2, 可用内存GB/2)` 再按 CPU 占用折算；1 vCPU 上 1080p 软编就是个位数帧率，参数绕不过去。
- **来源白名单默认只收私有网段**：跨公网访问必须显式配 `--landesk-allow-network`，否则连接在握手前被丢弃。
- **上游把会话初始状态放在 UI 外壳里，而不是 `Default` 里**：任何新的调用方（我们）都必须自己复制那几个字段。已经栽过三次（`server_file_transfer_enabled`、`server_keyboard_enabled`、CM 的 `Data::Login`）。排查方法是：找到功能读的每个字段 → 反向找上游谁把它设成 true → 逐字段对比。
- **egui 会吞掉 Ctrl+C/X/V**：文本场景靠 `Event::Copy/Cut/Paste`，文件场景必须自己轮询键状态；任何新的组合键都要确认 egui 是否拦截。
- **日志位置**：客户端 `%APPDATA%\lanDesk\log\client\`；服务端（SYSTEM）`C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\lanDesk\log\server\`；CM `%APPDATA%\lanDesk\log\landesk-cm\`。
- **一把全局私钥全网通用**：泄漏就要给所有机器换公钥，这是明确接受的取舍。
- **服务端不做并发仲裁**：多个 Master 同时操作会抢鼠标，与上游同形。
- **私钥保护只有文件系统 ACL**（与 Veyon 同路），DPAPI 未实现。
- **主机指纹首次信任（TOFU）**：首次连接时操作者需自己核对并 `--landesk-book-verify`，否则一直是“未验证”。
- **测试环境**：两台 Windows VM 的桥接网络偶尔整机掉线（本轮的 Win10 就是），遇到“连不上”先查 ARP 与端口，再怀疑代码。
