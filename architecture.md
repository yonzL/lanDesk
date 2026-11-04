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

1. Master 按地址簿里的 `主机:端口` 直连 Service（默认端口 `21118`，安装时可改）。
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
| 全局私钥 / 绑定公钥 | Master 持有私钥；每台 Service 绑定同一把公钥 | "来连接的人持有那把私钥" | 全网通行证：需要给所有机器换公钥 |
| 主机密钥（每台一对） | Service 持有私钥；在 `AuthNonce` 里带上公钥并对挑战签名 | "对面这台确实是地址簿里那台机器" | 只影响该机器的指纹，可逐台重置 |

主机密钥不经过 CSV、不需要人工分发：属于首次信任（Master 验完签名后把公钥钉进地址簿，标记为未验证）。因为 `SignedId` 里没有验签所需的公钥，主机身份必须由握手自己带出来——这也是不用 hbbs 后唯一可行的位置。

## 会话：接线，不重写

Master 的会话本体全部用上游的 `Remote`/`Session`（登录、视频解码、输入编码、剪贴板、文件、终端都在那里），landesk 只做三处接线：

1. **取连接**：`Client::_start` 里加一条 landesk 分支——目标地址在本地登记过，就用我们已认证的那条流（`transport::connect_authenticated`），不走 rendezvous。
2. **服务端放行**：landesk 模式下密码不参与（身份已在密钥阶段证明）、没有设备 ID 可校（不做 `lr.username` 比对）、不启动连接管理器（无界面 Service 没有 UI 进程，等它只会把连接拖死）。
3. **画面与输入**：`InvokeUiSession::on_rgba` 把帧交给界面（带递增序号，界面据以免除每帧 8MB 的无谓拷贝）；输入走 `Session::send_mouse`/`send_key_event`。

可验证性不靠手点：`tests/session_smoke.rs` 起一个 Service、用同一份二进制当 Master，断言真的有 RGBA 帧到达，而且指纹被钉住了。

已知边界：

- 多显示器已支持切换，但只渲染当前屏；`on_texture`（vram 纹理直通）不走。
- 键盘：可打印字符 + 常见控制键与 F1–F12；输入法、死键等未覆盖。
- **文件传输未接，且不是“接线”能解决的**：实测确认服务端只在**专门的文件传输会话**（`LoginRequest::FileTransfer`，`ConnType::FILE_TRANSFER`）里处理 `FileAction`，同一会话里发 `Data::SendFiles` 会被对端直接忽略（客户端日志会诚实地说 “New job … read …”，但对端什么都不做）。要做就得另起一个会话类型 + 一套传输界面（列表、进度、覆盖确认），属于单独一刀。
- 剪贴板：上游的客户端剪贴板循环已随会话启动，理论上双向可用；但本机同一台机器上无法区分“真的同步了”与“本来就是一个剪贴板”，所以**未进测试**，需要两台机器试用确认。
- 音频与终端的 UI 回调是空实现。

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

**Master**（配置目录内）：地址簿（组/子组两级、条目及其「地址:端口」、指纹）、全局私钥。私钥不写日志、不进 CSV、不随清单导出；保护靠文件系统 ACL（DPAPI 为可选加固，尚未实现）。

**Service**：绑定的公钥、自己的主机密钥对、监听端口（安装时设置，每台可不同）。无图形界面，全部经安装参数或配置文件写入。

## 模块取舍

保留并按 landesk 的需要裁剪：

- `client.rs` + `client/`（io_loop、文件、截图）、`server.rs` + `server/`（video / input / clipboard / display / connection）——会话两侧的本体。
- `ipc/`、`service.rs`、`platform/windows`——Windows 服务与活动会话进程之间的桥，含 SYSTEM 桌面访问。
- `libs/scrap`、`libs/enigo`、`libs/clipboard`、`libs/hbb_common`——采集、注入、剪贴板、协议与密码学原语。
- `flutter/` + `flutter_ffi.rs` + `ui_interface.rs`——上游的 Master 界面。**landesk 不用它**：见下方“Master 界面”一节。

移除：

- `rendezvous_mediator.rs`、`hbbs_http/`、`custom_server.rs`、`auth_2fa.rs`、`trusted_devices`——ID 注册、打洞、服务器 API、2FA、旧信任机制。
- `updater.rs`、`updater/`、`update_metadata*`、`update_hash.rs`、`manual_update.rs`——任何自动更新路径。
- `ui_cm_interface.rs`——主界面里的 ID 连接框，由地址簿取代。
- `lan.rs`、`kcp_stream.rs`、`port_forward*`、`privacy_mode`、`virtual_display_manager`、`whiteboard`——发现、KCP、端口转发、隐私模式、虚拟屏、白板，均不在能力清单内。

新增（只依赖 `hbb_common`）：

| 模块 | 职责 | 位置 |
|---|---|---|
| `keyauth` | 挑战—签名—验签、TOFU 指纹与指纹库、transcript 摘要、拒绝原因与日志行 | `libs/hbb_common/src/keyauth.rs` |
| `arbiter` | **不采用**：并发时不做事后仲裁，与上游一致（见“并发连接”一节）。 | — |
| `transport` | 直连监听：固定端口接受连接，交给会话层 | `src/transport.rs` |
| `landesk` | Service 侧配置（监听端口、绑定公钥）与鉴权执行点 | `src/landesk.rs` |
| `addressbook` | 两级组、条目（地址:端口）、指纹、CSV 导入（按「地址:端口」upsert，一个事务内）。 | 未开始 |
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

`landesk-gui`：一个独立的小 crate（`eframe`/egui，glow 后端，纯 Rust），只做三件事——按组/子组展示地址簿、对选中的机器拨号、把人能看懂的结果写在底部一行日志。

它取代了早先“裁剪上游 Flutter 界面”的打算，原因有三：Flutter 的 Dart 改造量远大于这个窗口；这台开发机上没有 Flutter 工具链（SDK 在下载中，不阻塞）；一个 300 行的窗口能直接编译、直接跑，符合“简单、可知”。上游 `flutter/` 仍留在树里，将来若要做主控端界面或移动端，它仍在原处。

两个已知取舍：wgpu 后端与主 crate 锁定的旧 `windows` crate 撞版本，所以固定走 glow；`eframe` 固定 0.31（0.37 的 `App` trait 与面板 API 已变）。
- 地址簿：`addressbook.json` 在可执行文件同目录（`--landesk-book` 可改），条目为 `组/子组/显示名/地址:端口`，指纹库内嵌在同一份文件里；导入导出就是整份文件的进出。
- 私钥：`private.key` 在可执行文件同目录（`--landesk-key` 可改），保护靠文件系统 ACL——与 Veyon 同一路子（它用 `keys/public` 与 `keys/private` 两个目录、每个密钥一个子目录，就为了能分别设权限）；DPAPI 是可选的加固，不是缺口。


`libs/hbb_common` 从上游的 git 子模块改为**内置目录**：landesk 要在它的 protobuf 定义里加握手消息，若仍跟随上游子模块，任何上游改动都会与本地的协议扩展相撞。这也是唯一一处 landesk 必须改动共享协议库的地方。

## 不变量

- **无出站**：Service 除应答外不发起任何连接；任何 hbbs/hbbr/updater 调用都是缺陷。landesk 模式下以跳过 `RendezvousMediator` 落实，日志里应出现 `landesk: skipping the rendezvous mediator`。
- **证明先于会话**：`AuthProof` 验证通过之前，Service 不接受任何会话消息；拒绝日志恰好一行。
- **私钥不出 Master**：不写日志、不进 CSV、不出现在任何网络报文中；保护靠文件系统 ACL（DPAPI 为可选加固，尚未实现）。
- **并发不仲裁**：与上游一致，多个已认证连接都能操作，后写入者生效；“只看”是连接时的声明，服务端只记日志。
- **分组无权限**：组/子组只影响 Master 侧怎么找机器，不进入任何授权判断。
- **凭据不入库**：landesk 不持有任何 Windows 账号；登录界面上的输入是人打的。

## 约束

- 仅 Windows（Master 与 Service），继承 RustDesk 的 AGPL-3.0。
- 目标网络前提：两端本来就能互相访问；landesk 不做穿透，也不做发现。
