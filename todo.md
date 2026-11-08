# lanDesk 待办与路线图

本文是**工作追踪**，不是规格。设计上的决定与边界在 `goal-and-requirements.md` 与 `architecture.md`；技术细节的链路图在 `docs/upstream-architecture.md`、`docs/file-clipboard-chain.md`、`docs/subnetdesk-notes.md`、`docs/flutter-path.md`。

最近更新：Flutter 会话窗口在真机出画面、鼠标注入生效之后（结构改为“路线图 + 已完成明细”）。

## 路线图

四个阶段，每个阶段有明确的**可验收结果**，不是按功能清单堆。

### 阶段 1 — 会话能用（已完成）

冷启动到“看到并操作远端桌面”。验收：两个进程之间直连、密钥证明、画面、鼠标键盘、剪贴板、音频、文件分发、装成服务、出包。

### 阶段 2 — 界面换成 Flutter（现在）

主界面已经是 Flutter（地址簿 + 双击开会话窗口）。这个阶段**只剩四件事**，做完才算收尾：

| # | 事项 | 验收方式 | 状态 |
|---|---|---|---|
| 1 | 会话窗口的**键盘注入** | 远端记事本里打字 → SSH 读文件内容对得上 | 未验证（鼠标已通过，键盘同一通道但 Alt+F4/Ctrl+Shift+Esc 无反应，需查） |
| 2 | **文本 / 文件剪贴板 + 音频**在 Flutter 窗口里 | 两台机器实测：本地复制 → 远端 `Get-Clipboard`；文件复制粘贴；声音 | 未验证（eframe 里都验过） |
| 3 | 修 **失焦后画面停更** | 会话窗口被盖住再切回来，远端新开的窗口要出现 | 已复现，未修 |
| 4 | **删掉 `landesk-client` crate** + 重打包 | 包里只剩 `landesk-server.exe` + `master/`（Flutter），CI/文档同步 | 未做（crate 仍在 workspace / CI / dist.sh） |

### 阶段 3 — 试用轮（下一阶段）

把成品交给真实用法：三台机器（本机 + 两台 Win10），照 `README` 的试用手册走一遍，收问题。

- 新机器加入（`--landesk-enroll`）、撤销一台 Master、指纹变更被拒 —— 都要真做一次
- 输入法 / CJK 在 Flutter 窗口里的落字（eframe 里验过）
- 会话内“文件推送”页与地址簿联动（现在要先选目标再选文件）
- 十几个窗口并存时的体验（同进程多引擎，见下方“已知边界”）

### 阶段 4 — 发布

- `v*` tag → CI 的 `release` 作业出包并 `gh release create`（流程已跑通一次）
- 不做：代码签名（无证书，文档说明 SmartScreen 即可）

## 未完成明细

按阶段排列，每条写“为什么还没做 / 从哪开始”。

### 阶段 2（现在）

- [ ] **会话窗口的键盘注入**：鼠标经 `send_mouse` 已确认生效（远端回收站被点开）；键盘用 `SendKeys` 发 Alt+F4 / Ctrl+Shift+Esc 远端无反应。先分清是“键没送到 Flutter”，还是“Flutter 送了但被上游的键盘模式挡掉”（`keyboard_mode` = Map/Legacy/Translate 三种路径，见 `docs/upstream-architecture.md` §输入）。
- [ ] **Flutter 会话窗口的剪贴板与音频实测**：eframe 里文本/文件剪贴板与音频都过了真机；Flutter 走的是上游 `FlutterHandler`，客户端剪贴板监听在非 Flutter UI 里曾零事件，这里要重新确认。
- [ ] **失焦后画面停更**：会话窗口被别的窗口盖住期间，远端新开的界面没有出现在流里（仍显示旧帧）。怀疑与失焦节流/事件流暂停有关；注意 `landesk::priority` 的降级只作用于 eframe 会话，Flutter 会话走上游逻辑。
- [ ] **删除 `landesk-client` crate**：用户已决定“直接删掉，只发 Flutter”。牵连点：`Cargo.toml` 成员、CI 的 `cargo check -p landesk-client` 与 release 作业、`scripts/dist.sh` 的拷贝与说明、`README`/`architecture.md`/`docs/flutter-path.md` 的引用。
- [ ] **重打包**：阶段 2 的 1–3 修完之后再出包，避免包里带一个已知会停更的会话窗口。

### 阶段 3（试用轮）

- [ ] 输入法 / CJK 在 Flutter 窗口里的落字：组合文本已按上游形状以 `Event::Text` → `chr` 的 down+up 键对转发，最终能不能落字取决于服务端 `process_chr` 的 `key_sequence` 路径
- [ ] 会话内“文件推送”页与地址簿的联动
- [ ] 多窗口并存的体验：同进程多引擎，一个会话卡住会影响同一进程的其它窗口；先量一量再决定要不要做隔离

### 按决定不做

- 分辨率降档：会改变被控机**现场**的显示设置（操作者就在那台机器前），代价不对等
- mDNS 局域网发现、终端（ConPTY）、代码签名

## 已完成

### 仓库与设计

- [x] 两份 durable spec 随实现演化：`goal-and-requirements.md`（能力、非目标、替代方案）、`architecture.md`（拓扑、身份、会话、并发、模块取舍、已知边界、不变量）
- [x] RustDesk 基线导入：上游 `23e5bb4`，`libs/hbb_common` 由子模块改为内置（握手协议要改它的 protobuf）
- [x] AGPL-3.0 许可、README（状态 / 构建前置 / 两台机器试用指南 / 拒绝原因对照表）
- [x] **产品命名统一**：交付物 `landesk-server.exe`（被控端 + 命令行）与 Flutter 的 `master/landesk-master.exe`（控制端界面）；app name 改为 `lanDesk`（配置目录、日志、服务名、IPC 管道都跟着它，因此自动关掉上游更新检查）
- [x] CI：`hbb_common` 作业（check + `keyauth` 测试）、主 crate 作业（vcpkg + libclang + `cargo check`/`key_proof`）、`v*` tag 的 `release` 作业
- [x] 大陆网络下的完整构建链路摸通并写进 README（USTC crates 镜像、GitHub git 源经代理 + openssl 后端、vcpkg 的 `opus/libvpx/aom/libyuv`、`libclang`）；MSVC 下的两个坑（vcvars 覆写 `VCPKG_ROOT`、Git Bash 的 `link.exe` 抢位）记在 `docs/flutter-path.md` 第 9 节

### 身份（landesk 的差异所在）

- [x] `keyauth`（`libs/hbb_common`）：ed25519 双向证明、TOFU 指纹库、`AuthNonce`/`AuthProof`/`AuthOk`、`AUTH_VERSION` 版本比对、拒绝原因与日志行
- [x] 服务端直连监听（`--landesk-listen`，默认 21118）与强制执行点；未绑定公钥时连 Kx 都不做
- [x] landesk 模式的服务端放行：密码不参与、不比对设备 ID、跳过上游 rendezvous，由监听监督循环接管进程存活
- [x] Master 侧拨号（`--landesk-connect` / `--landesk-view`）：密钥交换 → 主机身份 → 回证明 → 等 `AuthOk`
- [x] **一把 Master 一把私钥、Service 持有公钥集合**：`auth_keys` 数组 + 公钥目录，改完下次连接即生效（不需要重启）
- [x] `--landesk-allow-client` / `--landesk-revoke-client` / `--landesk-client-list` / `--landesk-enroll`（一步把公钥推到多台）
- [x] 双进程冒烟脚本 `scripts/landesk-smoke.sh`：持钥两连成功、错钥被拒并留一行日志、指纹变更被拒、断言无出站

### 监听与运维能力

- [x] **来源网络白名单（默认只收私有网段）**：`src/landesk/net.rs`，accept 后按 CIDR 判断，公网来源在握手前丢弃
- [x] **监听监督循环**（`src/transport.rs::supervise`）：绑定失败每秒重试并说明原因；`landesk.json` 的端口/白名单改了 1 秒内重绑，**已有会话不受影响**；带集成测试（改白名单后下一次握手必须被拒）
- [x] **绑定前探测端口**：`SO_REUSEADDR` 会让第二个实例也"绑定成功"，两个进程都报 listening 而连接全落到先绑的那个——现在探测到有人在听就走失败/重试
- [x] **监听状态对外可见**：`landesk-status.json`（`starting`/`listening`/`failed` + 端口 + 原因 + 起始时间），状态没变不重写；`--landesk-doctor` 读它并现场探一次端口
- [x] **配置优先级**：命令行 > `landesk.json`（监督循环只在“监听参数来自文件”时才重读文件）

### 地址簿与配置

- [x] 地址簿 `addressbook.json`：组/子组两级、条目（显示名 + `地址:端口` + 身份）、指纹内嵌同一份文件
- [x] CSV 导入/导出（中英文表头、引号转义、按 `地址:端口` 覆盖式合并、全量校验后才落盘、导出带 BOM、`#` 开头的组名是数据不是注释）
- [x] `--landesk-book-list` / `--landesk-book-verify`（TOFU 的“人工确认”那一步）
- [x] `landesk.json` + `--landesk-save-config`：让上游 `--install-service` 能在不额外改安装流程的情况下带参数自启
- [x] **Flutter 主界面读本机地址簿**：`bind.landeskBookList()`，组/子组树 + 状态圆点（灰=从未连接 / 黄=已钉未核对 / 绿=已核对）+ 双击连接 + 每行菜单（连接 / 标记已验证 / 看指纹 / 删除），2 秒轮询文件；云端那四个 tab 在模型里禁掉

### 会话（能看到、能操作）

- [x] 会话桥：已认证连接交给上游 `Remote`/`Session`，画面经 `InvokeUiSession::on_rgba` 交付
- [x] **Flutter 会话窗口**：地址簿双击 → `rustDeskWinManager.newRemoteDesktop(addr)` → 同进程第二个 Flutter 引擎 → `FFI.start()` → `client::_start` 自己从地址簿解析身份并走 keyauth（**不需要任何预备调用**）。真机验证：画面、鼠标注入生效、工具栏齐全
- [x] 鼠标（移动/按下/抬起/滚轮，含右键）、可打印字符、修饰键、控制键与 F1–F12/Insert/Space（eframe 时代验证；Flutter 里鼠标已复验）
- [x] **端到端测试** `tests/session_smoke.rs`：起真 Service、收真帧、持续推流、输入不报错、文件会话全操作、双服务扇出、白名单改动即时生效
- [x] 关键修复：会话入口改用 `ui_session_interface::io_loop`（此前用内层 `Remote`，`Session::sender` 从未接上，**所有输入被静默丢弃**）

### 画质、音频与地址簿（真机验证，eframe 时代）

- [x] **音频有声音**（用户实测）：播放全在上游 `AudioHandler`（opus 解码 + cpal），服务端默认就订阅 `audio_service`
- [x] **自定义画质 = 百分比 + 帧率上限**（上游语义）：`custom` 时出现 10–100% 滑块与 15 / 30 / 60 帧率上限
- [x] **编码器**：不覆盖客户端偏好，靠服务端 `av1-test=N` 让 `auto` 落到 VP9（≤4GB 内存仍由上游改选 VP8）
- [x] **地址簿图形编辑**：Add entry / Edit / Delete（删除同时丢弃指纹），保存前校验 `addr:port`

### 剪贴板与键盘（eframe 时代真机验证）

- [x] **文本剪贴板双向通**、**文件剪贴板双向通**（右键复制粘贴 + Ctrl+C/V）
- [x] **无界面连接管理器**（`--landesk-cm`）：用户令牌进程、只听 `_cm`、不画窗口、不参与授权
- [x] 四处“与上游不一致”导致的静默失效，均按“读上游 → 逐文件 diff → 只改差异”修掉：
  1. `Session::default()` 里三个服务端权限标志是 false，而上游两个 UI 外壳构造时全置 true（`is_file_clipboard_required` 因此恒 false）
  2. landesk 分支自己授权后没有 `try_start_cm(..., authorized)`，CM 侧 `is_authorized`/`file_transfer_enabled` 恒 false，丢弃每条 `ClipboardFile`
  3. 编码器默认值写在未 initialize 的 session 配置上，登录消息 `prefer` 恒为 Auto
  4. `codec_thread_num` 在 CPU 占用读不到时退化成单线程
- [x] **egui 的 Ctrl+C/X/V**：它把三个组合键转成 `Event::Copy/Cut/Paste` 并 `return`；改为合成 down+up 键对 + 文件场景轮询 `GetAsyncKeyState` 兜底

### 文件分发与安装

- [x] **文件分发（扇出）**：同一批文件同时推给多台 Service（每台一个独立会话并行 + 逐台进度），支持删除/改名/新建目录，下载也可用
- [x] **`--install-service` 实测**：Windows VM 上装成服务 `lanDesk`、开机自启、读 exe 同目录的 `landesk.json`

### 打包与发布链路

- [x] `scripts/dist.sh` → `dist/lanDesk-<日期>.zip`：静态 CRT + 静态 vcpkg，不依赖 DLL
- [x] **Flutter 发布构建**：`flutter build windows --release`（75MB bundle，整包 69MB），包里 `master/landesk-master.exe` + `librustdesk.dll` + `landesk-server.exe`
- [x] 干净目录实测发布包：genkey → save-config（含白名单）→ 仅配置启动 → 监听 + 白名单生效 → doctor 显示正确 → CLI 拨号 → 界面启动 → 双击 → 真会话

### 安全

- [x] **`master.key` 与 `.av1e/k.key` 曾进公开仓库**（`84f6448` / `5af568c`）→ 已从树中移除并 `*.key` 进 gitignore
- [x] **主控密钥轮换**：新私钥 `run-client/private.key`（公钥 `0d42a5b6…`）；Win10 上实测新钥通过、泄露的旧钥 `reject key-not-allowed`
- [x] **`--landesk-revoke-client` 静默失效修复**：旧 `landesk.json` 用单值 `auth_key`，只清 `auth_keys` 会让撤销"成功"但连接照样放行；现在两处都清、删公钥目录同名文件、撤不干净直接报错
- [x] **克隆机的主机身份重复**：从 Win10 克隆出来的新机器曾与源机共用同一把主机密钥（指纹相同），已换成自己的身份

## 已知边界与风险

- **同进程多引擎，没有崩溃隔离**：Flutter 的会话窗口是同一进程里的第二个 Flutter 引擎（`desktop_multi_window` 没有 `CreateProcess`），一个会话硬崩会带走所有窗口和主界面。“一个会话一个进程”是 eframe 时代的结论，已不成立。
- **`landesk::priority` 只能按进程降级**：`SetPriorityClass(GetCurrentProcess())` 降的是整个进程；在 Flutter 界面里接焦点降级会把主窗口一起降下去。要按会话降级得改线程级或按会话限帧（待办）。
- **验证必须用 release 构建**：这一轮最大的弯路是把 debug 客户端当性能基准（同机同分辨率 3.5 fps vs 26.1 fps）。以后报“卡”先确认是不是 release。
- **`cargo test` 会覆盖 `librustdesk.dll`**：默认特性构建会重写 `target/debug/librustdesk.dll`（78MB → 17MB），随后 `flutter build` 装进去的 DLL 缺 `rustdesk_core_main`，界面表现为一闪而退。顺序固定为：先 `cargo build --lib --features flutter`，再 `flutter build`。
- **被控机算力决定帧率上限**：编码线程数取 `min(核数/2, 可用内存GB/2)` 再按 CPU 占用折算；1 vCPU 上 1080p 软编就是个位数帧率，参数绕不过去。
- **来源白名单默认只收私有网段**：跨公网访问必须显式配 `--landesk-allow-network`，否则连接在握手前被丢弃。
- **上游把会话初始状态放在 UI 外壳里，而不是 `Default` 里**：任何新的调用方都必须自己复制那几个字段。已经栽过三次（`server_file_transfer_enabled`、`server_keyboard_enabled`、CM 的 `Data::Login`）。排查方法：找到功能读的每个字段 → 反向找上游谁把它设成 true → 逐字段对比。
- **一把全局私钥全网通用**：泄漏就要给所有机器换公钥，这是明确接受的取舍。
- **服务端不做并发仲裁**：多个 Master 同时操作会抢鼠标，与上游同形。
- **私钥保护只有文件系统 ACL**（与 Veyon 同路），DPAPI 未实现。
- **主机指纹首次信任（TOFU）**：首次连接时操作者需自己核对并标记已验证，否则一直是“未验证”。
- **测试环境**：本机 + 两台 Win10（`192.168.99.191`、`192.168.99.230`）。VM 的桥接网络会整机掉线，遇到“连不上”先查 ARP 与端口，再怀疑代码。

## 日志位置

- 客户端（Flutter）：`%APPDATA%\lanDesk\log\`（`*_rCURRENT.log`）
- 服务端（SYSTEM）：`C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\lanDesk\log\server\`
- CM（用户令牌）：`%APPDATA%\lanDesk\log\landesk-cm\`
