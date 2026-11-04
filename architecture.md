---
id: landesk-architecture
type: architecture-design
title: landesk 架构：直连传输、双挑战身份、单控制令牌
status: active
parent: landesk-goal
---

# landesk 架构

两个程序，一条直连，没有第三方在场。Master 拨号，Service 应答；两者之间的一切都发生在这一条 TCP 连接上。

## 拓扑

```
Master (Flutter UI + Rust core)  ──TCP 直连──►  Service (无界面, Windows 服务)
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
| 主机密钥（每台一对） | Service 持有私钥；Master 地址簿记其指纹 | "对面这台确实是地址簿里那台机器" | 只影响该机器的指纹，可逐台重置 |

主机密钥不经过 CSV、不需要人工分发：属于首次信任，而不是清单的一部分。

## 会话仲裁：一个控制令牌，多个观察者

仲裁在 **Service 端强制**，Master 界面隐藏按钮不算数。

- 每台 Service 同时只有一个连接持有**控制令牌**：视频照发，输入、剪贴板、文件可用。
- 其他连接以**观察者**身份接入：只收视频。
- 观察者请求控制 → 令牌原子转移，原持有者立即降为观察者（画面不断）。
- 观察者的输入、剪贴板、文件消息在 Service 侧一律丢弃，不走"先收下再忽略"的路径。

## Windows 落地

Service 以 SYSTEM 身份作为 Windows 服务运行，沿用 RustDesk 既有的 IPC 机制：由服务在活动会话中拉起进程完成采集与输入注入。这条桥同时带来两个结果，都是需求：

- 目标机停在登录界面时仍可连接并操作——操作者自己在会话里敲 Windows 用户名密码，landesk 不保存、不代填、不感知任何系统凭据。
- 本地有人登录时直接接管当前桌面，不做提示。

## 存储

**Master**（配置目录内）：地址簿（组/子组两级、条目及其「地址:端口」、指纹）、全局私钥。私钥用 Windows DPAPI 静态加密，不写日志、不进口 CSV、不随清单导出。

**Service**：绑定的公钥、自己的主机密钥对、监听端口（安装时设置，每台可不同）。无图形界面，全部经安装参数或配置文件写入。

## 模块取舍

保留并按 landesk 的需要裁剪：

- `client.rs` + `client/`（io_loop、文件、截图）、`server.rs` + `server/`（video / input / clipboard / display / connection）——会话两侧的本体。
- `ipc/`、`service.rs`、`platform/windows`——Windows 服务与活动会话进程之间的桥，含 SYSTEM 桌面访问。
- `libs/scrap`、`libs/enigo`、`libs/clipboard`、`libs/hbb_common`——采集、注入、剪贴板、协议与密码学原语。
- `flutter/` + `flutter_ffi.rs` + `ui_interface.rs`——Master 界面，裁掉以 ID 为中心的连接页。

移除：

- `rendezvous_mediator.rs`、`hbbs_http/`、`custom_server.rs`、`auth_2fa.rs`、`trusted_devices`——ID 注册、打洞、服务器 API、2FA、旧信任机制。
- `updater.rs`、`updater/`、`update_metadata*`、`update_hash.rs`、`manual_update.rs`——任何自动更新路径。
- `ui_cm_interface.rs`——主界面里的 ID 连接框，由地址簿取代。
- `lan.rs`、`kcp_stream.rs`、`port_forward*`、`privacy_mode`、`virtual_display_manager`、`whiteboard`——发现、KCP、端口转发、隐私模式、虚拟屏、白板，均不在能力清单内。

新增（只依赖 `hbb_common`）：

| 模块 | 职责 | 位置 |
|---|---|---|
| `keyauth` | 挑战—签名—验签、TOFU 指纹与指纹库、transcript 摘要、拒绝原因与日志行 | `libs/hbb_common/src/keyauth.rs` |
| `arbiter` | 控制令牌、观察者列表、抢占降级、能力矩阵。Service 侧唯一的权限判断点。 | 未开始 |
| `transport` | 直连监听：固定端口接受连接，交给会话层 | `src/transport.rs` |
| `landesk` | Service 侧配置（监听端口、绑定公钥）与鉴权执行点 | `src/landesk.rs` |
| `addressbook` | 两级组、条目（地址:端口）、指纹、CSV 导入（按「地址:端口」upsert，一个事务内）。 | 未开始 |
| `svcsetup` | 安装期：注册 Windows 服务、写入绑定公钥与端口、启动。 | 未开始 |

### 落地形态（Service 侧）

- 开关：`--landesk-listen [port]`（缺省 `21118`）与 `--landesk-auth-key <hex>`。两者都不给时是空操作，上游 rendezvous 路径不受影响；给了就进入 landesk 模式。
- 公钥的文本形式是**小写十六进制**，与指纹表示同一套字符；不用 base64。
- 握手顺序：先走上游 Kx（Service 用自己的主机密钥签 `KeyExchange`），成功后 Service 在 AEAD 通道上发 `AuthNonce`，Master 回 `AuthProof`（签名覆盖 `AUTH_DOMAIN ‖ transcript_digest ‖ nonce`）。证明不过即在任何会话消息前断开。
- 被拒时只留一行：`landesk: reject <no-bound-key|bad-signature|host-fingerprint-mismatch|no-proof-yet> from <地址>`。
- 未绑定公钥时**连 Kx 都不做**，直接拒。


`libs/hbb_common` 从上游的 git 子模块改为**内置目录**：landesk 要在它的 protobuf 定义里加握手消息，若仍跟随上游子模块，任何上游改动都会与本地的协议扩展相撞。这也是唯一一处 landesk 必须改动共享协议库的地方。

## 不变量

- **无出站**：Service 除应答外不发起任何连接；任何 hbbs/hbbr/updater 调用都是缺陷。
- **证明先于会话**：`AuthProof` 验证通过之前，Service 不接受任何会话消息；拒绝日志恰好一行。
- **服务端强制**：观察者的输入、剪贴板、文件消息在 Service 侧被丢弃；权限判断只有 `arbiter` 一处。
- **私钥不出 Master**：不写日志、不进 CSV、不出现在任何网络报文中；静态加密走 DPAPI（**尚未实现，私钥目前只靠文件 ACL**——已知遗留）。
- **单控制令牌**：同一台 Service 任何时刻至多一个控制者，抢占是原子的降级而非并行。
- **分组无权限**：组/子组只影响 Master 侧怎么找机器，不进入任何授权判断。
- **凭据不入库**：landesk 不持有任何 Windows 账号；登录界面上的输入是人打的。

## 约束

- 仅 Windows（Master 与 Service），继承 RustDesk 的 AGPL-3.0。
- 目标网络前提：两端本来就能互相访问；landesk 不做穿透，也不做发现。
