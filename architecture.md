---
id: landesk-architecture
type: architecture-design
title: landesk 架构：直连传输、双挑战身份、与上游同形的并发
status: active
parent: landesk-goal
---

# landesk 架构

两个程序，一条直连，没有第三方在场。Master 拨号，Service 应答；两者之间的一切都发生在这一条 TCP 连接上。

## 拓扑

```
Master (最小自有窗口 + Rust core)  ──TCP 直连──►  Service (无界面, Windows 服务)
   地址簿 / 全局私钥 / 指纹库                      绑定公钥 / 主机密钥 / 端口
```

没有 hbbs、hbbr、云账号、控制台。Service 不主动连接任何地方——它只监听与应答。这是可以被验收的一条线：landesk 进程在运行期间没有出站连接。

## 分层：握手是前置层，会话协议照用

landesk 不重写 RustDesk 的会话协议。视频、输入、剪贴板、文件这些消息格式一律沿用，`libs/hbb_common` 的 framed stream 与 protobuf 定义不动。landesk 加的是**握手前置层**：

1. Master 按地址簿里的 `主机:端口` 直连 Service（默认端口 `21118`，安装时可改）。监听只接受**私有网段来源**（默认 RFC1918 + 100.64/10 + 链路本地 + IPv6 私有段），公网来源在握手前就被丢弃；需要放开时用 `--landesk-allow-network` / `landesk.json` 的 `allowed_networks` 显式配。

   监听由**监督循环**维护（`src/transport.rs::supervise`）：绑定失败每秒重试并在日志里说明原因（端口被占不再等于这台机器失联到下次重启）；`landesk.json` 里的端口或白名单变了，1 秒内重绑，已有会话不受影响。运行状态写到 exe 同目录的 `landesk-status.json`（`starting` / `listening` / `failed` + 端口 + 原因 + 时间），`--landesk-doctor` 直接读它。
2. Service 下发随机挑战；Master 用**全局私钥**签名并回送，同时声明本次请求的角色（控制 / 观看）。
3. Service 用安装时绑定的**公钥**验签；失败立即断开。
4. Service 用**自己的主机密钥**签回 Master 的挑战；Master 拿结果与自己地址簿里记的指纹比对。
   - 该地址首次连接 → TOFU：把指纹写进地址簿，向操作者显示一次，此后视为已知。
   - 已有指纹且不匹配 → 拒绝连接并报警（可能是重装机器，也可能是冒充）。
5. 两侧各有一次签名，握手 transcript 由此确定。
6. 双方用一对**临时 X25519** 在 transcript 上派生会话密钥，之后所有报文走 AEAD。静态密钥只用来签名挑战，不参与数据加密。
7. 握手完成后再进入 RustDesk 原有的消息流；原有 `LoginRequest` 里的密码/2FA 字段在 landesk 中不再使用。

为什么这样：握手之外的部分（编码、输入注入、剪贴板、文件传输、Windows 会话桥）是 RustDesk 十余年堆出来的资产，landesk 的差异全在"怎么找到对方、怎么证明身份"，所以差异只放在握手层。

## 身份：两把钥匙各管一头

| 密钥 | 谁持有 | 证明什么 | 泄露后果 |
|---|---|---|---|
| Master 密钥（每台一对） | 每台 Master 持有自己的私钥；Service 持有允许接入的**公钥集合** | "来连接的人持有集合里某一把对应的私钥" | 只丢这一台 Master 的身份；在那几台 Service 上撤销一条即可 |
| 主机密钥（每台一对） | Service 持有私钥；在 `AuthNonce` 里带上公钥并对挑战签名 | "对面这台确实是地址簿里那台机器" | 只影响该机器的指纹，可逐台重置 |

两层各管一头：Master 密钥决定**谁能进来**，主机密钥决定**我在跟谁说话**。两者都不需要人工分发密钥文件——主机密钥靠首次信任（TOFU 钉指纹），Master 公钥靠 `--landesk-allow-client` 加入服务端。

允许集合每次连接重读（`reload_auth_keys`），所以允许/撤销立即生效；旧的单值 `auth_key` 仍可读，视为集合里的一个元素。多台 Master 共用一把私钥仍然可行（直接把它们配成同一个公钥），但那会让泄露后的轮换成本回到全网级。

**生成与绑定分开**：生成密钥只是产生文件，绑定才是“认可身份”。所以两端都能生成——Master 生成自己的身份（私钥），Service 生成主机密钥（私钥不出本机）。方向保持不对称：**Service 只导入公钥，Master 只导入私钥**。

**一台 Master 可以持有多把身份**：私钥放在 `<exe>/identities/`（`--landesk-import-key` 导入、`--landesk-key-forget` 卸载），文件名带角色-主机-时间-IP 便于区分；地址簿条目可以用 `key` 指定“连这台用哪把”，留空则用默认身份（`private.key` 优先，否则身份目录里最新的一把）。这把“逐目标绑定”比全局单身份更贴近实际：不同机器可能只认不同身份。

**密钥命名只是标签**：`<role>-<host>-<yyyymmdd-HHMM>-<ip>` 用于区分多把，不是轮换语义。Master 身份轮换 = 新公钥 enroll 到各 Service 后再 revoke 旧的；Service 主机密钥轮换会让所有 Master 的 TOFU 指纹失配，必须逐台重新核验。

主机密钥不经过 CSV、不需要人工分发：属于首次信任（Master 验完签名后把公钥钉进地址簿，标记为未验证）。因为 `SignedId` 里没有验签所需的公钥，主机身份必须由握手自己带出来——这也是不用 hbbs 后唯一可行的位置。

## 会话：接线，不重写

Master 的会话本体全部用上游的 `Remote`/`Session`（登录、视频解码、输入编码、剪贴板、文件、终端都在那里），landesk 只做三处接线：

1. **取连接**：`Client::_start` 里加一条 landesk 分支——目标地址在本地登记过，就用我们已认证的那条流（`transport::connect_authenticated`），不走 rendezvous。
2. **服务端放行**：landesk 模式下密码不参与（身份已在密钥阶段证明）、没有设备 ID 可校（不做 `lr.username` 比对）。连接管理器照旧启动——但跑的是我们自己无界面的那份（见下）。
3. **画面与输入**：`InvokeUiSession::on_rgba` 把帧交给界面（带递增序号，界面据以免除每帧 8MB 的无谓拷贝）；输入走 `Session::send_mouse`/`send_key_event`。

可验证性不靠手点：`tests/session_smoke.rs` 起一个 Service、用同一份二进制当 Master，断言真的有 RGBA 帧到达，而且指纹被钉住了。

已知边界：

- 多显示器已支持切换，但只渲染当前屏；`on_texture`（vram 纹理直通）不走。
- 键盘：可打印字符 + 常见控制键与 F1–F12；输入法、死键等未覆盖。
- **剪贴板（含传文件）已接上，两机实测双向通过**：会话里自动开启上游的 `enable-file-copy-paste`，在资源管理器里复制文件到远端粘贴能直接落盘（Windows 双向，对端会回 `has_file_clipboard: true`）；`tests/session_smoke.rs` 断言对端确实收到并确认了这个选项。文本剪贴板由客户端的轮询器发送（见 CM 那一条），粘贴往返在两台 Windows 机器上都验过。
- **连接管理器（CM）以无界面形态存在**：上游把“需要用户身份才能做的事”（读本机剪贴板、Windows 文件剪贴板、UAC、聊天/语音转发）交给一个以用户令牌运行、监听 `_cm` 的进程。我们的二进制没有 UI 外壳，`--cm` 起不来，所以新增 `--landesk-cm`：同一个 exe、同一套 `ui_cm_interface` 业务逻辑，配一个不画界面的 `InvokeUiCM` 实现（8 个回调只记日志，且只在 trace 级记录对端可控的日志）。
  - 服务端在 landesk 模式拉起它（`start_ipc` 的启动参数切换），CM 出错只告警、不断开会话——画面与输入不依赖它，降级仍可用。
  - 它不参与授权：连接是否允许仍由密钥握手决定，所以远端**不会出现任何“接受连接”弹窗**。
  - 文本剪贴板两个方向走的路不同：**写**在本机桌面由 `--server` 进程直接做（上游同样如此），**读**必须经 CM（`--server` 是 SYSTEM，自己读拿到的不是用户桌面的剪贴板）。
  - 控制端的“本机 → 对端”发送由 `src/landesk/session.rs` 的轮询器负责：上游 `clipboard_listener` 在非 Flutter UI 里不报变更（实测线程起来了但零事件），所以自己每 400ms 读一次、按内容去重后发 `MultiClipboards`。去重与回声靠上游的 owner marker（`clipboard.rs:399` 的 `get_formats_filter` 会跳过「自己写的那一侧」）。
- **文件分发（扇出）已做**：同一批文件可以同时推给多台 Service，每台一个独立的文件会话并行跑，界面逐台给 `完成数/总数` 或失败原因；文件操作（删除、改名/同目录移动、新建目录）与下载也能用。
  - 关键点：Windows 上文件操作本来整套委托给 CM（`ipc::FS::*`）。做法是在 `send_fs` 里把请求交给上游的 `handle_fs`（原本就是 CM 用的自由函数）**在本进程执行**，结果通过 `local_reply` 当作 CM 回包投回连接循环；写 job 由 Connection 自己的 `local_write_jobs` 持有。CM 起来之后这条路径**保持不变**：它已验证可用，没必要为了统一而改回委托。
  - 另一条必须遵守的上游约定：上传时 `to` 是**目标目录 + 文件名**、`file_num = 0`（从 Flutter 的 `file_model.dart` 查到）。
  - landesk 模式不启动查询用的 IPC 管道（没托盘），因此**同机可以并存多个 Service**——扇出测试就是这么跑的。但这与 CM 用的 `_cm` 无关：同一个用户会话里 CM 只有一个，多个 Service 会复用它。
  - “启动文件”按决定**砍掉**（需要新协议动作 + 服务端执行开关，安全边界不同）。
- **音频播放不需要界面回调**：解码与播放全在 `client.rs` 的 `AudioHandler`（opus 解码 + cpal 输出），由 `start_audio_thread` 驱动。界面只负责 `disable-audio` 开关与 `Permission::Audio` 的展示；服务端默认就会把已授权的连接订阅到 `audio_service`（`add_monitor_connection` 的 `noperms` 里只排除被关掉的服务）。
- 终端（ConPTY）仍是空实现：它需要单独的终端交互界面，不在本产品当前范围。

## 并发连接：与上游一致

多台 Master 可以持有同一把私钥、同时连一台 Service。这里刻意不发明仲裁：

- 每个已认证连接都收画面，也都**可以发输入**；同时操作时后写入者生效。
- “只看”是**连接时的声明**：Master 自己决定不发送输入（上游 `view_only` 是同一回事），Service 把 `AuthProof.control = false` 记进日志，不做事后拦截。
- 为什么不用服务端令牌：持有私钥的人本来就是全权的，想乱来的客户端直接声明自己是控制者即可；服务端丢弃“观察者”的输入只防误触、不防对手，却要多一套状态与降级路径。代价是两个操作者会抢鼠标，这一点写进 goal 的能力条目里，不藏着。

## Windows 落地

Service 以 SYSTEM 身份作为 Windows 服务运行，沿用 RustDesk 既有的 IPC 机制：由服务在活动会话中拉起进程完成采集与输入注入。这条桥同时带来两个结果，都是需求：

- 目标机停在登录界面时仍可连接并操作——操作者自己在会话里敲 Windows 用户名密码，landesk 不保存、不代填、不感知任何系统凭据。
- 本地有人登录时直接接管当前桌面，不做提示。

## 存储

**Master**（配置目录内）：地址簿（组/子组两级、条目及其「地址:端口」、指纹）、全局私钥。私钥不写日志、不进 CSV、不随清单导出；保护靠文件系统 ACL（非 Windows 上以 0600 创建；DPAPI 未实现，是可选的加固方向）。

**Service**：绑定的公钥、自己的主机密钥对、监听端口（安装时设置，每台可不同）。无图形界面，全部经安装参数或配置文件写入。

## 模块取舍

保留并按 landesk 的需要裁剪：

- `client.rs` + `client/`（io_loop、文件、截图）、`server.rs` + `server/`（video / input / clipboard / display / connection）——会话两侧的本体。
- `ipc/`、`service.rs`、`platform/windows`——Windows 服务与活动会话进程之间的桥，含 SYSTEM 桌面访问。
- `libs/scrap`、`libs/enigo`、`libs/clipboard`、`libs/hbb_common`——采集、注入、剪贴板、协议与密码学原语。
- `flutter/` + `flutter_ffi.rs` + `ui_interface.rs`——上游的 Master 界面。**正式版的界面方向就是它**（先决条件已在 `docs/flutter-path.md` 中实测打通）；当前仍由 eframe 的 `landesk-client` 交付，见下方“Master 界面”。

移除（**已落地**，2026-11）：

- `rendezvous_mediator.rs`（1844 行）、`hbbs_http/`（1844 行，其中有 114 行的通用 HTTP 客户端搬到 `src/http_client.rs` 保留）、`updater.rs` + `updater/` + `manual_update.rs`（846 行）——ID 注册、打洞、中继、服务器 API、自动更新，全部已从树中删除。
- 客户端的直连 IP / 域名 / rendezvous 回退路径：`Client::start` 现在只接受会话层登记过的目标，其余直接报错。
- 启动时的 `api.rustdesk.com` 版本检查（`check_software_update` 及响应解析）。
- flutter-only 的云钩子也一并删了：`flutter_ffi.rs` 的下载器/更新 FFI 与 `ui_interface.rs` 的 OIDC 登录（`account_auth*`）。`--features flutter` 现在不会再引用已删模块。

拟删（未做）：

- `custom_server.rs`、`auth_2fa.rs`、`trusted_devices`——服务器 API、2FA、旧信任机制。
- `ui_cm_interface.rs`——主界面里的 ID 连接框，由地址簿取代。
- `lan.rs`、`kcp_stream.rs`、`port_forward*`、`privacy_mode`、`virtual_display_manager`、`whiteboard`——发现、KCP、端口转发、隐私模式、虚拟屏、白板，均不在能力清单内。

新增（只依赖 `hbb_common`）：

| 模块 | 职责 | 位置 |
|---|---|---|
| `keyauth` | 挑战—签名—验签、TOFU 指纹与指纹库、transcript 摘要、拒绝原因与日志行 | `libs/hbb_common/src/keyauth.rs` |
| `arbiter` | **不采用**：并发时不做事后仲裁，与上游一致（见“并发连接”一节）。 | — |
| `transport` | 直连监听：固定端口接受连接，交给会话层 | `src/transport.rs` |
| `landesk` | Service 侧配置（监听端口、绑定公钥）与鉴权执行点 | `src/landesk.rs` |
| `addressbook` | 两级组、条目（地址:端口）、指纹、CSV 导入导出（按「地址:端口」覆盖式合并；解析全成功才落盘）。 | `src/addressbook.rs` |
| `svcsetup` | 安装期：注册 Windows 服务、写入绑定公钥与端口、启动。 | 未开始 |

### 落地形态（Service 侧）

- 开关：`--landesk-listen [port]`（缺省 `21118`）与 `--landesk-auth-key <hex>`。两者都不给时是空操作，上游 rendezvous 路径不受影响；给了就进入 landesk 模式。
- 公钥的文本形式是**小写十六进制**，与指纹表示同一套字符；不用 base64。
- 握手顺序：先走上游 Kx（Service 用自己的主机密钥签 `KeyExchange`），成功后 Service 在 AEAD 通道上发 `AuthNonce`（携带自己的主机公钥 + 主机签名），Master 验主机证明并按指纹库判定，再回 `AuthProof`；Service 验签通过后回 `AuthOk`，Master 收到回执才算连成。证明不过即在任何会话消息前断开。
- 被拒时只留一行：`landesk: reject <no-bound-key|bad-signature|host-fingerprint-mismatch|bad-host-proof|version-mismatch|no-proof-yet> from <地址>`。
- 未绑定公钥时**连 Kx 都不做**，直接拒。
- 配置了监听端口时**不启动上游 rendezvous**，并由监听本身维持进程存活（上游原本靠那条永不返回的循环活着）。

### 落地形态（Master 侧）

- `--landesk-genkey <path>`：生成全局密钥对，打印公钥供 Service 绑定。
- `--landesk-connect <addr:port>`：拨号、验主机身份、回证明、把指纹变化落盘，然后退出。

### Master 界面

**两个外壳，一套后端。**

* 现在交付的是 `landesk-client`（独立 crate、`eframe`/egui、glow 后端、纯 Rust）：按组/子组展示地址簿、双击开一个会话窗口、把人能看懂的结果写在底部日志。
* **正式版的方向是 Flutter**（上游 `flutter/`）：先决条件已打通——codegen（含 ffigen）可用、`cargo check --features flutter` 通过、`flutter analyze lib` 0 error。剩下的只是把 Dart 页面接到 landesk 的接口上，见 `docs/flutter-path.md`。

界面与后端的分工是稳定的：landesk 已经把界面需要的东西集中暴露在 `src/landesk/ffi.rs`（`book_list` / `book_upsert` / `book_remove` / `book_verify` / `identity_list` / `listen_status` / `open_remote_window`），`flutter_ffi.rs` 只有一层同名薄封装。**换 UI 不动后端。**

**一个会话一个进程**：界面不自己跑会话，调用 `open_remote_window(addr)` 起一个 `--landesk-remote <addr>` 子进程。崩溃隔离、输入法/剪贴板天然按窗口隔离，关窗口即断会话。十几个窗口并存时，后台窗口会被降级（`landesk::priority`：进程优先级 `BELOW_NORMAL` + 请求帧率 5 fps + 重绘 5Hz），前台保持 `NORMAL` 与界面帧率上限。

两个已知取舍：wgpu 后端与主 crate 锁定的旧 `windows` crate 撞版本，所以固定走 glow；`eframe` 固定 0.31（0.37 的 `App` trait 与面板 API 已变）。
- 地址簿：`addressbook.json` 在可执行文件同目录（`--landesk-book` 可改），条目为 `组/子组/显示名/地址:端口/身份`，指纹库内嵌在同一份文件里。CSV 不是“整份替换”：导入按 `地址:端口` **覆盖式合并**（同址覆盖、其余保留），且全部行解析成功才落盘；导出写 `组,子组,显示名,地址,身份`，与自己的解析器对称（`#` 开头的值加引号、换行转空格、表头去 BOM）。
- 私钥：`private.key` 在可执行文件同目录（`--landesk-key` 可改），保护靠文件系统 ACL——与 Veyon 同一路子（它用 `keys/public` 与 `keys/private` 两个目录、每个密钥一个子目录，就为了能分别设权限）。Unix 上 `GlobalKeyPair::save` 直接以 0600 创建文件；Windows 上没有这个模式位，靠目录 ACL。**DPAPI 尚未实现**（这是可选的加固方向，不是当前承诺）。


`libs/hbb_common` 从上游的 git 子模块改为**内置目录**：landesk 要在它的 protobuf 定义里加握手消息，若仍跟随上游子模块，任何上游改动都会与本地的协议扩展相撞。这也是唯一一处 landesk 必须改动共享协议库的地方。

## 不变量

- **无出站**：Service 除应答外不发起任何连接。已经**从代码上**落实：`rendezvous_mediator`、`hbbs_http`、`updater` 与启动时的版本检查都已删除；客户端也只连会话层登记过的目标（其余直接报错）。冒烟脚本里仍有“无出站”的断言作为第二道防线。
- **证明先于会话**：`AuthProof` 验证通过之前，Service 不接受任何会话消息；拒绝日志恰好一行。
- **私钥不出 Master**：不写日志、不进 CSV、不出现在任何网络报文中。
- **并发不仲裁**：与上游一致，多个已认证连接都能操作，后写入者生效；“只看”是连接时的声明，服务端只记日志。
- **分组无权限**：组/子组只影响 Master 侧怎么找机器，不进入任何授权判断。
- **凭据不入库**：landesk 不持有任何 Windows 账号；登录界面上的输入是人打的。

## 约束

- 仅 Windows（Master 与 Service），继承 RustDesk 的 AGPL-3.0。
- 目标网络前提：两端本来就能互相访问；landesk 不做穿透，也不做发现。
