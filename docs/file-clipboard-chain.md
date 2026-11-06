# 文件剪贴板（Windows）完整链路与差异分析

排障方法固定为三步：**读上游这条链 → 逐文件 diff 我们的树 → 只按差异下结论**。下面是这条链的完整前置条件表，以及我们与上游的实际差异。

## 1. 实现与上游的一致性（先确认没有自造轮子）

| 文件 | 与上游 `23e5bb4` 的差异 |
|---|---|
| `src/clipboard_file.rs`（`Cliprdr` 编解码、`ContextSend` 桥） | **0 行** |
| `src/clipboard.rs`（文本/文件剪贴板读写、owner marker） | **0 行** |
| `src/client/io_loop.rs`（`handle_cliprdr_msg`、`check_clipboard_file_context`） | **0 行** |
| `libs/clipboard/`（Windows OLE 桥、`ContextSend`） | **完全一致** |
| `src/ui_cm_interface.rs` | 1 处：`handle_fs` 由 `async fn` 改为 `pub(crate) async fn`（我们本地执行 FS 需要） |
| `src/client.rs` | 只多一个 landesk 分支：用已认证的流替代 rendezvous（`Client::_start` 开头） |

**结论**：这条链的实现层我们没改过，出问题只可能出在**构造会话/配置**上。这也是后面只查"取值"的原因。

## 2. 链路（客户端复制文件 → 被控端粘贴）

```
控制端(Windows)                                  被控端(Windows)
──────────────                                   ──────────────
ContextSend(本机 OLE)                             ContextSend(用户令牌的 --landesk-cm 里)
   │ 本机剪贴板出现文件                                ▲
   │                                                  │ OLE 写虚拟文件
   ├─ Cliprdr → 服务端 ───────────────────────────→ Data::ClipboardFile → CM
   │                                                  │
   │ ←────────────── Cliprdr(文件内容请求) ────────────┤ 被控端应用粘贴时向 CM 要 file contents
   └─ 回文件内容 ─────────────────────────────────────→ CM 写进剪贴板
```

## 3. 前置条件表（每一格都要成立）

| # | 条件 | 上游取值 / 位置 | 我们的取值 | 状态 |
|---|---|---|---|---|
| 1 | 控制端 cliprdr 上下文被启用 | `io_loop.rs:170` `ContextSend::enable(true)`（`handler.is_default()`） | 同（我们的会话是 `DEFAULT_CONN`） | ✅ |
| 2 | **控制端认为对端允许文件传输** | Flutter/Sciter 构造会话时 `server_file_transfer_enabled = true`（`flutter.rs:1359`、`ui/remote.rs:602`） | **`Session::default()` = false** | ❌→已修（`ff7ea5d`） |
| 3 | 控制端本机开关 | `lc.enable_file_copy_paste.v`，默认来自 `UserDefaultConfig`，键值默认 **"Y"**（`config.rs:2491`） | 同（未改动） | ✅（待日志确认） |
| 4 | 登录消息带上该权限 | `client.rs:3321` `option.enable_file_transfer = config.enable_file_copy_paste.v` | 同 | ✅ |
| 5 | 服务端放行文件传输 | `connection.rs:604` `file = permission(OPTION_ENABLE_FILE_TRANSFER, control_permissions)`；客户端不发 `control_permissions` → 回落全局 `enable-file-transfer`（默认 Y） | 同 | ✅ |
| 6 | 服务端收到对端意愿 | `self.enable_file_transfer` ← 登录 `option.enable_file_transfer` | 同 | ✅（依赖 3/4） |
| 7 | 服务端把 `Cliprdr` 交给 CM | `connection.rs:3516` `send_to_cm(Data::ClipboardFile(clip))` | 同 | ✅ |
| 8 | CM 的 OLE 上下文启用 | `ui_cm_interface.rs:851` `ContextSend::enable(is_permission_enabled_locally(OPTION_ENABLE_FILE_TRANSFER))` | 同（我们的 `--landesk-cm` 走同一入口） | ✅ |

**唯一差异是第 2 项**：上游两个 UI 外壳在构造会话时显式把 `server_file_transfer_enabled` 初始化成 `true`，而我们用 `Session::<LandeskUi>::default()`，`Arc<RwLock<bool>>` 的默认值是 `false`，于是：

- `check_clipboard_file_context()`（`io_loop.rs:2796`）算出 `enabled = false` → `ContextSend::enable(false)`；
- 日志里就是反复出现的 `Succeeded to uninit cliprdr`；
- 服务端也不会收到任何 `Cliprdr`，所以两个方向同时死（文本剪贴板不走这条路，因此一直正常）。

## 3b. 第二处差异：CM 从来没收到过 `Data::Login`

上游有一条不变量：**每个授权成功的登录分支，在 `send_logon_response_and_keep_alive()` 之后都要 `self.try_start_cm(lr.my_id, lr.my_name, self.authorized)`**（`connection.rs:3129`、`:3149`）。我们那条 landesk 分支（自己授权、不等 CM）把函数直接返回了，所以：

| CM 侧状态 | 来源 | 我们的实际值 |
|---|---|---|
| `is_authorized`（`ipc_task` 里） | `Data::Login.authorized` | **false**（从未收到 Login） |
| `self.file_transfer_enabled` | `Data::Login.file` | **false**（同上） |

CM 因此丢弃每一条 `Data::ClipboardFile`：日志里就是

```
Process clipboard message from client peer, stop: false, is_clipboard_enabled: true, file_transfer_enabled: false
Clipboard message from client peer, but not authorized
```

修复：在 landesk 分支的授权之后补上同一个 `try_start_cm(..., self.authorized)` 调用。

## 3c. 日志位置（别再找错）

- 客户端（release）：`%APPDATA%\RustDesk\log\client\landesk-client_rCURRENT.log`
  —— 因为 GUI 在 `init_log` 之后才把 `APP_NAME` 改成 `lanDesk`，所以日志跟着旧名走。
- 服务端（SYSTEM 服务）：`C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\lanDesk\log\server\`
- CM（用户令牌）：`%APPDATA%\lanDesk\log\landesk-cm\`

## 4. 下次怎么一次定位（不要试错）

控制端会话开始时打印一行（`spawn_session` 里）：

```
landesk: file clipboard state — server_file_transfer_enabled: ?, enable_file_copy_paste: ?, is_default: ?
```

三者都应是 `true` / `true` / `true`。服务端侧看：

- CM 日志 `libs\clipboard\src\platform\windows.rs` 的 `client_format_list called ... FileGroupDescriptorW, FileContents`：说明客户端已经把"我这边有文件"通告到被控端剪贴板；
- CM 日志 `Process clipboard message from clip, stop: ... is_clipboard_enabled: ... file_transfer_enabled(peer): ...`：`stop` 必须是 `false`，否则就是第 5/6 项里的某个开关没打开；
- 服务端日志的 `Option update: ... enable_file_transfer: Yes`。

## 5. 已知与上游不同的地方（与文件剪贴板无关，但会一起被看到）

- 服务端 `send_fs` 在本进程执行（不依赖 CM 的文件操作通道），与 `Data::ClipboardFile` 是两条独立通道，互不影响。
- `--landesk-cm` 无界面：只实现 `InvokeUiCM` 的 9 个通知方法，OLE/`ContextSend` 逻辑完全复用上游。

## 6. 差异审计（`connection.rs` / `server.rs` 里我们对上游做的每一处改动）

排障要查的是"我们与上游的差异"，所以把这两份文件的差异逐条列出并判定。**新增的日志不算差异**。

### `src/server/connection.rs`

| 差异 | 判定 |
|---|---|
| 新增字段 `local_reply` / `local_write_jobs` | 刻意：无界面时本地执行 FS |
| `CmErr` 在 landesk 模式只告警 | 刻意：CM 只提供用户身份能力，不该带走会话 |
| `send_fs` 在 landesk 模式本地执行 `handle_fs` | 刻意：已验证可用 |
| 跳过密码校验 | 刻意：身份已在密钥阶段证明 |
| 跳过设备 ID 比对 | 刻意：没有 ID 这回事 |
| `try_start_cm_ipc` 在非 root 时提前返回 | 刻意：非 root 本来就是用户身份 |
| CM 启动参数改 `--landesk-cm` | 刻意：无界面 CM |
| **授权分支缺少 `try_start_cm(..., authorized)`** | **漏掉的上游副作用 —— 已修（本次 bug）** |

### `src/server.rs`

| 差异 | 判定 |
|---|---|
| 密钥交换后插入 `landesk::enforce(...)`，并把 transcript 摘要交出来 | 刻意：我们的鉴权层 |
| 跳过查询用 IPC（`ipc::start("")`） | 刻意：无托盘/无 CM 查询，且允许同机并存多个 Service |
| 跳过 `RendezvousMediator::start_all()`，改为 await 自己的监听 | 刻意：无出站 |

**结论**：截至本轮，真正的"漏掉上游副作用"只有 `try_start_cm` 一处。以后在 landesk 模式里砍掉上游某段流程时，必须检查它除了"启动某服务"之外还**给其他组件发过什么状态**（CM 的 `Data::Login` 就是这类）。
