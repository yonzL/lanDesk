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
- `PROTOCOL_VERSION` 显式比对，不匹配直接 bail。我们目前没有版本号，两端版本不一致时的行为是"协议解析失败"，报错不友好——值得加一个。
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

1. **来源 CIDR 白名单，默认私有网段**（≈40 行 + 一个依赖，带单测）。与 goal 里"只在私有网络里用"完全一致，且把"端口暴露在公网"从"靠密钥"升级成"默认拒绝"。
2. **协议版本号**（握手时比对，不匹配给明确错误）。我们已经在握手层加了自定义消息，加一个 `u32` 很便宜。
3. **监听监督循环 + 状态机**（配置变了自动重启监听，界面能显示 failed/last_error）。我们现在改端口必须重启进程。
4. **mDNS 发现**（`mdns-sd`）。收益明显但工作量最大，且要防"被发现在公网端口上"的隐私问题。

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
