# SubnetDesk 与 RustDesk 的差异（可借鉴点）

SubnetDesk（`zibo-chen/SubnetDesk`）是 **RustDesk 2.0.0** 的分支，定位和我们几乎一样：LAN / 私有网络 / VPN 内的直连远控，去掉公共 ID、rendezvous、relay、云账号。它比我们的基线（1.5.1）新一个大版本，所以"差异"里混了两类东西：**上游 2.0 的演进** 和 **它自己的取舍**。下面分开记。

## 1. 它的自研部分（我们可借鉴）

### 1.1 握手：`src/lan_protocol.rs`（318 行）

```
ClientHello{protocol_version, client_nonce, client_capabilities}
   ↓
ServerHello{protocol_version, device_public_key, server_nonce, ephemeral_public_key,
            signature = sign(transcript)}   ← 用设备长期签名密钥签
   ↓  客户端验签 + 校验 transcript(client_nonce, server_nonce, ephemeral_pk)
PublicKey{asymmetric_value, symmetric_value}   ← 复用上游的对称密钥协商
   ↓  stream.set_key(key)
```

要点：

- **只证明被控端身份**（客户端验主机），控制端身份靠登录框里的用户名/密码（Argon2id）。我们的做法更强：单把全局私钥做**双向**证明，不需要密码。
- 它把自定义消息加进 `message.proto`（`LanClientHello` / `LanServerHello`），用 `Message` 承载——和我们在 `rendezvous.proto` 加 `AuthNonce`/`AuthProof` 是同一手法。
- `PROTOCOL_VERSION` 显式比对，不匹配直接 bail。我们已经有同一件事：`AUTH_VERSION` + `AuthNonce.version`，不匹配返回 `RejectReason::VersionMismatch`（日志里是 `version-mismatch`）。这条我们已经做完了。
- 指纹 = `device_fingerprint(device_public_key)`，首次连接交给用户确认并记住（TOFU）。我们已做同一件事（`PinnedHosts`），并且额外支持"标记已验证"。

### 1.2 来源网络白名单：`src/lan_server.rs`

```rust
// 默认只允许私有地址段
vec!["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10",
     "127.0.0.0/8", "169.254.0.0/16", "fc00::/7", "fe80::/10", "::1/128"]
```

- accept 之后立刻判来源 IP（`source_allowed`），不在白名单就 `continue`，连握手都不走。
- 配置项 `lan-allowed-networks`（逗号分隔 CIDR），IPv4-mapped IPv6 会被归一化。
- 依赖 `cidr_utils`；实现 40 行，带 6 个单测。

**这是我们目前缺的一条便宜且有效的防线**：我们的监听端口对任何来源都开门，只靠密钥把门。

### 1.3 mDNS 发现：`src/lan_mdns.rs`（563 行）

- 服务类型固定 `_subnetdesk._tcp.local.`，依赖 `mdns-sd`。
- 广播"在网、可被连"这件事与"是否允许连入"解耦（`is_discoverable()` vs `service_ready()`）。
- 我们只有地址簿（手工/CSV），没有发现能力。

### 1.4 监听运行时：`src/lan_server.rs` + `src/desktop_runtime.rs`

- `LanServer::start()` 是个**监督循环**：监听参数（端口、绑定地址、白名单…）组成 `listener_signature()`，签名一变就重启监听；服务不可用/桌面正在退出就关连接、等待、重来。
- 有显式状态机 `stopped/starting/listening/failed` + `last_error`，界面直接读它。
- 我们的监听是"起一次、失败就报错退出"，配置改动需要重启进程。

### 1.5 2.0 的 user-session runtime

README 提到 DRM 包因为"根服务要适配新的 user-session runtime"而暂停。`desktop_runtime.rs` 是他们在 2.0 引入的"桌面会话生命周期"抽象——对我们正在做的 CM（用户令牌进程）是一个**方向验证**：用户会话进程是这类产品的标配。

## 2. 上游 2.0 相对我们 1.5.1 的相关变化

| 位置 | 2.0 的写法 | 对我们的意义 |
|---|---|---|
| `libs/scrap/src/common/codec.rs:983` `codec_thread_num` | **仍是** `cpu usage` 读不到就退化成 1 线程 | 我们那个 `None → max/2` 的修复在上游仍未修，属于真 bug |
| `src/client/io_loop.rs:1231` `fps_control` | 默认 `custom_fps.unwrap_or(60)`（我们是 30）；结构改成按 display 的 `video_threads` | 2.0 的默认上限更高；我们的"逐级减半、下限 1"在弱机器上会滚到很低 |
| mDNS/白名单/LAN 设置 | 2.0 分支自带 | 见 §1 |

## 3. 结论：我们该借什么、不该借什么

**建议借（按性价比排序）**

1. **来源 CIDR 白名单，默认私有网段**（≈40 行 + 一个已在依赖树里的 `ipnet`，带单测）。与 goal 里"只在私有网络里用"完全一致，且把"端口暴露在公网"从"靠密钥"升级成"默认拒绝"。**已实现**（`src/landesk/net.rs`）。
2. **监听监督循环 + 状态机**（配置变了自动重启监听，界面能显示 failed/last_error）。我们现在改端口必须重启进程。
3. **mDNS 发现**（`mdns-sd`）。收益明显但工作量最大，且要防"被发现在公网端口上"的隐私问题。

**不建议借**

- **用户名 + 密码（Argon2id）**：我们的密钥模型更强也更简单，密码只是多一个可泄露的口令面。
- **把自定义消息全部塞进 `message.proto`**：我们已经用 `rendezvous.proto` 的握手前置消息，改动更小。
- **直接跟到 2.0**：跨大版本合并成本高，且 2.0 的 session/runtime 重构会影响我们已跑通的 CM 与文件分发。

## 4. 与 landesk 现状对照

| 能力 | landesk | SubnetDesk |
|---|---|---|
| 直连 `IP:端口` | ✅ | ✅ |
| 无 rendezvous/relay/账号 | ✅ | ✅ |
| 被控端身份（指纹） | ✅ TOFU + 可标记已验证 | ✅ TOFU |
| 控制端身份 | ✅ 全局 ed25519 密钥双向证明 | ⚠️ 用户名 + 密码 |
| 默认端口 | 21118 | 21118 |
| 来源白名单 | ❌ | ✅ 默认私有网段 |
| 局域网发现 | ❌ | ✅ mDNS |
| 文件分发到多台 | ✅（同一批文件扇出） | ❌（仅普通文件传输） |
| 用户令牌的 CM | ✅ 无界面 `--landesk-cm` | ✅（2.0 的 user-session runtime） |

## 3. 远程会话窗口：两条链路到底差在哪（2026-11 调研）

结论先说：**窗口这一层完全相同，差别全在拨号层**。SubnetDesk 的 Flutter 会话窗口是上游原样，
没有任何局域网专用代码；我们之所以需要额外接线，是因为保留了一层「目标注册表」。

### 3.1 SubnetDesk（上游 2.0 形状）

```
地址簿双击 / --connect <host:port>
  → connect()                (flutter/lib/common.dart:2756)
  → connectMainDesktop()     (common.dart:2729)
  → rustDeskWinManager.newRemoteDesktop(id)   (multi_window_manager.dart:272)
  → DesktopMultiWindow.createWindow(msg)      ← 独立窗口，参数只有 {type, id, password}
新窗口 Dart（argv: multi_window <id> <json>）
  → runMultiWindow → DesktopRemoteScreen → ConnectionTabPage → RemotePage
  → FFI.start(id) → bind.sessionAddSync(...) 建 Session<FlutterHandler>
                  → bind.sessionStart(...)   → src/flutter.rs:1489 spawn io_loop
  → io_loop → Client::start(peer)  (src/client/io_loop.rs:208)
  → Client::start 自己就是 LAN 拨号器 (src/client.rs:251)：
      Endpoint::parse(peer) → 候选列表 → TCP → lan_protocol::client_handshake
```

要点：**peer id 就是 endpoint**。`Client::start` 里直接 `Endpoint::parse(peer)` 并拨号，
所以 UI 层（含 Flutter 多窗口）完全不需要知道「这次是局域网连接」——上游原代码即可工作。
它为此新增的 FFI 只有登录体验相关的 `session_login_lan` / `session_login_lan_identity`
（用户/密码或身份选择），与窗口链路无关。

### 3.2 我们（1.5.1 形状 + landesk 前置层）

窗口链路与它逐行相同（`connect` → `newRemoteDesktop` → `createWindow` → `FFI.start`），
差别全在 `Client::_start`：上游 1.5.1 的 `_start` 是「rendezvous → 直连 IP → relay」三段式，
我们把 landesk 直连插在最前面，**两条分支都走同一个 `connect_authenticated`**：

```rust
// ① 调用方登记过目标（landesk::session::start / 冒烟脚本的临时地址簿与密钥）
if let Some(target) = crate::landesk::session::take_target(peer) {
    connect_authenticated(peer, &target.book, &target.key, target.control).await?
}
// ② 没登记：像 SubnetDesk 的 Client::start 那样自己解析
if crate::landesk::is_landesk() {
    let (book, key) = crate::landesk::session::default_inputs(peer)?;   // 地址簿里该地址的身份，否则默认私钥
    connect_authenticated(peer, &book, &key, true).await?
}
```

两层都在的理由：① 给 CLI 用 `--landesk-book` / `--landesk-key` 指向临时文件的场景（冒烟脚本、
`--landesk-push`）保留自带路径的能力；② 让 Flutter 会话窗口——上游 `session_add`/`session_start`
驱动的一个新 Flutter 引擎（与主窗口**同一进程**，不是新进程）——不需要任何预备调用。

早先只有 ①，而 eframe 壳（`landesk::session::start` 自己构造 `Session<LandeskUi>`）正好需要它；
Flutter 窗口走进来时才发现它没有那个调用点，于是加上了 ②。

### 3.3 与 SubnetDesk 的差别清单（已按 ② 收敛）

收敛已完成：Flutter 会话窗口的 `session_start` 不需要任何预备调用，链路回到上游原样。
剩下的差别都是**有意保留**的：

差别清单（供后续比对）：

| 关注点 | SubnetDesk | 我们 |
|---|---|---|
| peer id 形态 | `hbb_common::lan::Endpoint`（共享类型，含校验） | `host:port` 字符串，解析分散在几处 |
| 拨号入口 | `Client::start` 即 LAN 拨号器（无注册表） | `_start` 里先看登记（CLI 临时路径），否则自己解析 |
| 身份来源 | 用户/密码 + 可记住的凭据（`lan_access_username`） | 全局密钥对（双向签名），服务端绑公钥集合 |
| 主机身份 | `lan_protocol::client_handshake` 验设备公钥 + TOFU | `AuthNonce.host_pk + host_proof` + `PinnedHosts` TOFU |
| 会话窗口接线 | 无（上游原样） | **也无**（已收敛，`FFI.start` 前无任何预备调用） |
| 候选/重试 | `connection_candidates` + mDNS 发现回退 | 单地址，不重试候选 |
