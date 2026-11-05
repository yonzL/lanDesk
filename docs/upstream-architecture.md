# RustDesk 上游架构地图（landesk 排障用）

这份文档不是我们的设计，而是**上游代码怎么运转**的速查表。landesk 的问题大多落在这些链路里；排查时先在这里定位，再去读对应的 `file:line`。

约定：行号对应基线 `23e5bb4`。我们的分支改动都标了 `landesk`。

## 1. 两端与进程模型（Windows）

```
SCM ──► rustdesk.exe --service         (session 0, SYSTEM)
          └─► landesk-server.exe --server   (用户 session，仍是 SYSTEM 令牌)
                └─► --cm (连接管理器)          (用户 session，用户令牌)   ← landesk 没有
```

- `--service`：`src/platform/windows.rs:659 run_service` → `:694` 取活动 session → `:721/:739 launch_server`。
- `--server`：`windows.rs:814 launch_server` → `:826 launch_privileged_process` → `:834` 以 `as_user=FALSE` 在目标 session 里启动（所以是 SYSTEM 身份、用户 session）。
- `--cm`：由 `--server` 反向拉起，`src/server/connection.rs:6466 start_ipc` → `:6482 run_as_user(vec!["--cm"])`；CM 监听 `_cm` 管道 `src/ui_cm_interface.rs:847`。
- **授权只能由 CM 完成**：`connection.rs:767 Data::Authorize` → `:1922 authorized=true`；无 CM 时 `LOGIN_GRACE=180s`（`connection.rs:89`）后断连。landesk 在密钥鉴权通过后直接授权（`connection.rs:3095/3044`），绕过 CM。

### 只有 CM 能做的事（landesk 必须旁路或放弃）

文件操作 `ipc::FS::*`（`connection.rs:2452`）、**Windows 文件剪贴板**（`connection.rs:3516` 无条件 `send_to_cm`）、SYSTEM 身份读剪贴板（`clipboard_service.rs:241`）、接受/拒绝连接、聊天、语音、权限切换、切换方向、UAC 提权、隐私模式回显。

landesk 已有的旁路（改动集中在四处）：`common.rs:231 need_fs_cm_send_files`、`connection.rs:2433 send_fs`、`connection.rs:2907 try_start_cm_ipc`、`clipboard_service.rs:202 get_clipboard_msg`、`server.rs:698/741`。

## 2. 视频链路

```
服务端 video_service 采集→编码 ──VideoFrame──► 客户端 io_loop ──► 解码线程 ──► on_rgba ──► UI
```

- 客户端入口：`ui_session_interface.rs:1974 io_loop` → `:2069 Remote::new` → `client/io_loop.rs:164`；收包 `:296` → `:1699 Union::VideoFrame`。
- 每 display 一条解码线程：`client/io_loop.rs:2889 new_video_thread`（队列 `VIDEO_QUEUE_SIZE=120`，`client.rs:112`）→ `client.rs:4002 start_video_thread` → `client.rs:4070` 回调 → `on_rgba`（trait `ui_session_interface.rs:1746`）。
- 丢帧与限速：关键帧绕过队列（`io_loop.rs:1716`）；差量帧入 `ArrayQueue`，满则 `force_push` 挤掉并发 `refresh_video`（`:1723-1725`）；`fps_control`（`:1543`）按解码帧率回推 `OptionMessage.custom_fps`（`:1624`）；队列长期超阈值会 `Refresh display to reduce delay`（`:1652`）。
- 编码器选择：客户端 `codec-preference` 选项 → `libs/scrap/src/common/codec.rs:819 preference()` → `SupportedDecoding.prefer` 进登录消息（`client.rs:3460/3877`）→ 服务端 `Encoder::update`（`connection.rs:5013` → `codec.rs:171`）取多数派写全局 `ENCODE_CODEC_FORMAT`（`codec.rs:327`）。`auto` 的优先级是 **h265 > h264 > AV1 > VP9**（`codec.rs:284-327`）。
- 画质/帧率：`client.rs:3395-3419` 把 `image_quality`/`custom_fps` 写进 `OptionMessage`（**`custom-fps` 只在 `image_quality=="custom"` 时随登录发送**）；服务端 `video_qos.rs:435-456` 换算 quality ratio（0.5/0.67/1.5，`codec.rs:893-926`）与码率。
- 服务端发送节奏：`video_service.rs:597 run`，每轮 `spf = 1/fps`（`video_qos.rs:344`）+ 每帧等所有订阅者确认（`video_service.rs:1010-1020`，最多 3s）。
- 「画面没变」：不跳过，而是用上次 yuv 重编码（`video_service.rs:726-729/941-975`、`static_refresh.rs:181`）；不录制时 VPX **关闭关键帧**（`vpxcodec.rs:90-95`）。

## 3. 输入链路

```
UI 事件 ──► KeyEvent{chr|control_key, modifiers, mode} ──► 服务端 input_service ──► enigo/rdev
```

- 上游客户端的转换在 `src/keyboard.rs:998 event_to_key_events` → `legacy_keyboard_mode`（`:1076`）+ `legacy_modifiers`（`:425`，把按住的修饰键**追加**进 `KeyEvent.modifiers`）。
- `chr` 一字段两义：Map 模式是扫描码（`keyboard.rs:1414`，服务端 `input_service.rs:1734`）；Legacy/Translate 模式是 Unicode 标量（服务端 `process_chr` `input_service.rs:2236`）。
- 服务端分水岭：`input_service.rs:1850 process_chr(..., has_hotkey_modifiers(evt))`。**没有修饰键 → 当文本输入（`en.key_sequence`），有 → 真按键注入（带 Ctrl）**。`has_hotkey_modifiers`（`:1771`）只读 `KeyEvent.modifiers`，不读主机键盘状态。
- 鼠标 mask：低 3 位类型、`按钮 << 3`（`common.rs:68-91`，用法 `client.rs:4399`）；服务端 `input_service.rs:1287-1288` 解析，DOWN/UP 只认单键精确匹配（`:1361-1390`）。

## 4. 剪贴板链路（两个方向不对称）

**客户端 → 服务端（不依赖 CM）**
`client.rs:1945 try_start_clipboard`（监听线程，需要 `ClientClipboardContext`：`io_loop.rs:1790` 构造）→ `client.rs:2085 check_clipboard` → `:2128 send_msg` → `connection.rs:3460/3490` → `clipboard.rs:259 do_update_clipboard_`（arboard 直接写对端桌面剪贴板）。

**服务端 → 客户端（SYSTEM 下依赖 CM）**
`server/clipboard_service.rs:194 get_clipboard_msg` → SYSTEM 时走 `read_clipboard_from_cm_ipc`（`:241/:253`，连 `_cm`），非 SYSTEM 直接 `clipboard::check_clipboard`（`:232`）→ 客户端 `io_loop.rs:1868/1888` → `update_clipboard(..., Client)`。

**文件剪贴板（资源管理器复制文件）**
协议 `Cliprdr`（`clipboard_file.rs:4/183`）。客户端侧在进程内（`io_loop.rs:170 ContextSend::enable`、`:494 handle_local_clipboard_msg`）；**服务端侧无条件 `send_to_cm`（`connection.rs:3516`）**，OLE 桥在 CM 里（`ui_cm_interface.rs:645`）。→ 无 CM 时双向全废。

## 5. 文件传输（非剪贴板）

- 会话类型 `ConnType::FILE_TRANSFER`（`client/io_loop.rs:189`，登录 `client.rs:3886`）；服务端只在该类型下处理 `FileAction`（`connection.rs:3571`）。
- 客户端上传 `io_loop.rs:852 Data::SendFiles`（`is_remote=false` → read job）；下载 `is_remote=true` → write job。
- 服务端所有文件动作都过 `send_fs`（`connection.rs:2429`）：landesk 在本进程跑 `ui_cm_interface::handle_fs`（`:2433-2449`），否则 `send_to_cm`（`:2452`，无接收者时 **静默丢弃**）。

## 6. 排障对照表
| 现象 | 先查这些点 |
|---|---|
| 画面滞后 | 服务端 `video_qos` 的 fps/ratio（trace `qos_trace`）、`switch to change quality`、客户端 `Refresh display … to reduce delay`（`:1652`）、`Set fps to {}`；landesk 侧额外看 `on_rgba` 的整帧转换与 GUI 拉帧拷贝 |
| Ctrl 组合无效 | egui 是否把 Ctrl+C/X/V 吞成 `Copy/Cut/Paste`（我们的 UI 要自己组包）；`KeyEvent.modifiers` 是否带上；`chr` 事件没有对应的 key-up |
| 本端复制、远端粘不出 | 客户端 `try_start_clipboard` 的监听是否在跑（`Start client clipboard loop`）、服务端是否收到 `Union::Clipboard`、`do_update_clipboard_` 是否报 `Failed to set clipboard` |
| 远端复制、本端粘贴不了 | 服务端 `get_clipboard_msg` 是否走到 CM 分支报错（`Failed to read clipboard from cm`） |
| 文件粘贴无效 | 服务端 `Union::Cliprdr` → `send_to_cm` 无人接收（本表第 1 节旁路未覆盖） |

## 7. landesk 自己引入的额外开销（上游没有）
- `src/landesk/session.rs:155 on_rgba`：每帧做一次 `ARGB→RGBA` 全量转换并换新 `Arc<Frame>`（1080p ≈ 8MB/帧）。
- 早期版本 GUI 还会 `clone()` 一帧（已改为 `Arc` 共享）。
- 1080p@30fps 下这两步是 GB/s 级的内存带宽，VM 里会直接吃满 → 表现为“画面不跟手”而服务端 fps 正常。

## 8. 日志在哪（找错地方会很浪费时间）

release 构建写文件日志，路径由 `Config::log_path()` 决定，进程名作子目录：`<log>/<name>/<name>_rCURRENT.log`。

- 普通用户进程：`%APPDATA%\lanDesk\log\<name>\`
- **SYSTEM 服务进程**：`Config::get_home()` 里的 `patch()`（`libs/hbb_common/src/config.rs:469`）把 `system32\config\systemprofile` 改写成 `ServiceProfiles\LocalService`，所以真实位置是
  `C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\lanDesk\log\server\`

`--server` 的日志里能直接看到：`new encoder: VPX(...)`、`cpu num: N, ... codec thread: M`、`Option update: ...custom_fps`、`encode fail: no valid frame`、`Failed to read clipboard from cm`、`clipboard updated on host`。这些是判断"编码太慢 / QoS 降档 / 剪贴板读写失败"的第一手证据。

## 9. 资源对帧率的影响（实测）

`codec_thread_num()` 的上限是 `min(cpu/2, 可用内存/2)`，再按 CPU 占用估算，最后落到 1/2/4/8/16/32/64。1 个 vCPU 的虚拟机上，1080p VP8 软件编码只能到个位数帧率——这时客户端 `Set fps to N`、`Refresh display to reduce delay` 都是症状，不是原因。
