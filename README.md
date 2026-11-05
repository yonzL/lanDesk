# lanDesk

局域网、私有网段或 VPN 内的远程桌面：**按地址直连，按密钥鉴权**。没有设备 ID、没有信令、没有中继、没有云账号，也没有用户名。

- **Master（控制端）**：图形界面，持有地址簿与全局私钥，发起连接并操作远端桌面。地址簿按「组 / 子组」两级组织，可用 CSV 批量导入。
- **Service（被控端）**：无图形界面的 Windows 服务，随开机自启，安装时绑定一个公钥，只接受持有对应私钥的 Master。

## 文档

| 文档 | 内容 |
|---|---|
| [`goal-and-requirements.md`](goal-and-requirements.md) | 产品目标、能力清单、非目标、与 RustDesk / SubnetDesk / Veyon 的差别 |
| [`architecture.md`](architecture.md) | 直连传输、双挑战身份与 TOFU 指纹、并发模型、会话接线、RustDesk 模块取舍、已知边界 |
| [`todo.md`](todo.md) | 已完成 / 未完成的工作清单（工作追踪，不是规格） |

前两份是 durable spec，都是活的：能力变了、边界动了，就在同一次改动里更新它们。`todo.md` 只回答“做到哪了、还差什么”。

## 与上游的关系

lanDesk 是 [RustDesk](https://github.com/rustdesk/rustdesk) 的精简分支：保留其会话内核（采集、编码、输入注入、剪贴板、文件传输、Windows 服务与活动会话之间的桥），移除全部公网路径（`rendezvous_mediator`、`hbbs_http`、中继、账号、自动更新）。

由此继承 RustDesk 的 **AGPL-3.0** 许可，见 [`LICENCE`](LICENCE)。

### 基线

| 项 | 值 |
|---|---|
| 上游提交 | `23e5bb471a040d62f0a6ad52591fb30784265b73`（2026-11-01，`rustdesk/rustdesk` master） |
| 获取方式 | 经 `gitee.com/mirrors/rustdesk` 镜像（镜像保留上游 SHA）；直连 GitHub 在当前网络下拉完整树不可行 |
| 导入形式 | 单次基线提交，未携带上游提交历史；后续裁剪以普通提交叠加 |
| `libs/hbb_common` | 由上游子模块改为内置目录，固定在上游 `fae47e024dd83cef6aaecde0aa7d11731df11649`，`.gitmodules` 已移除 |

需要重新比对上游时，用同 SHA 从镜像取一份源码即可，不必修改本仓库的历史。

## 状态

可以上手试用了。已有：双向 `keyauth`（主机身份 + 全局私钥证明 + TOFU 指纹）、Service 监听、地址簿（CSV 导入/导出/标记验证）、`landesk.json` 与开机自启路径、图形界面（画面、鼠标、可打印字符、修饰键与 F1–F12、多屏切换、断线回退），以及**真正的远端会话**——同机自动化测试把整条链跑通：密钥交换 → 身份证明 → 登录 → 视频解码 → 收到 2560x1600 的真实帧（`cargo test -p rustdesk --test session_smoke`，约 2 秒），并覆盖“持续推流”与“对端消失后会话自行结束”。

还不够“成品”的地方：

- **批量文件传输没做**。剪贴板传文件已可用（会话里自动开启 `enable-file-copy-paste`，在资源管理器复制文件、到远端粘贴就落盘），但真正的批量推送/拉取 + 文件操作（删除、移动、重命名、启动）需要专设的文件传输会话，是单独一刀；详见 `architecture.md` 的“已知边界”。
- **文本剪贴板未进测试**（同机两进程共享一个剪贴板，无法区分是否真的同步）；文件通道已测到“对端确认开关”这一步。
- 音频、终端未接；输入法/死键未覆盖。
- `--install-service` 需要管理员，实际安装尚未验证；无代码签名。

另：公网路径（`rendezvous_mediator`、`hbbs_http`、自动更新等）仍在树里，但 landesk 模式已不再启动它们；彻底删除随对应功能替换逐块进行——删除点集中在会话与鉴权核心（`server/connection.rs`、`ipc.rs`、`flutter_ffi.rs`），先行删除只会得到一个既不能编译也不能运行的树。

## 构建

仅目标 Windows。完整构建需要 Rust 工具链、`libclang`（`kcp-sys` 用 bindgen）与 vcpkg 的 `opus`/`libvpx`/`aom`/`libyuv`。Flutter 只有想做上游那套界面时才需要；landesk 自己的界面是纯 Rust。

```bash
cargo check -p hbb_common --all-targets          # landesk 唯一改动的共享协议库
cargo build --bin landesk-server -p landesk-client
cargo test  -p rustdesk --test session_smoke     # 端到端：真的收到一帧画面
bash scripts/dist.sh                            # 打一个可拷给别人的包（需先 --release）
```

静态 CRT + 静态 vcpkg，所以打包出来就是两个 exe，不依赖 DLL。`landesk-server.exe` 的双击启动会去拉起上游的 sciter 界面，需要 `sciter.dll`——landesk 模式下不要双击它，只用 `--server` / `--landesk-*` 这些参数。

CI（`.github/workflows/ci.yml`）在 `windows-latest` 上只跑第一条；主 crate 的闸门要等裁剪到仅剩 Service 目标后再加。

### 大陆网络下的前置条件

上游的依赖图里有一批只存在于 GitHub 的 git 源，还有一个挂在 `chromium.googlesource.com` 上的子模块。直连环境下这些都不通，以下配置已在本机验证可用（**不入库**，属于个人环境）：

| 阻碍 | 做法 |
|---|---|
| crates.io 约 25KB/s | `~/.cargo/config.toml` 里把 `source.crates-io` 换成 `sparse+https://mirrors.ustc.edu.cn/crates.io-index/`（实测约 220KB/s） |
| GitHub 的 git 源握手失败（schannel） | 环境变量 `GIT_CONFIG_*` 注入 `http.sslBackend=openssl`，并 `url.https://ghfast.top/https://github.com/.insteadOf=https://github.com/` 走可用代理 |
| `chromium.googlesource.com/webm/libwebm` 子模块 | `url.https://github.com/webmproject/libwebm.insteadOf=https://chromium.googlesource.com/webm/libwebm`；官方 GitHub 镜像包含该提交 |
| bindgen 找不到 libclang | 安装 LLVM 并设 `LIBCLANG_PATH` 到其 `bin` 目录 |

完整的一条命令（示例值按自己的代理端口改）：

```bash
export HTTPS_PROXY=http://127.0.0.1:7890 HTTP_PROXY=http://127.0.0.1:7890 \
  GIT_CONFIG_COUNT=3 \
  GIT_CONFIG_KEY_0=http.sslBackend GIT_CONFIG_VALUE_0=openssl \
  GIT_CONFIG_KEY_1=url.https://ghfast.top/https://github.com/.insteadOf GIT_CONFIG_VALUE_1=https://github.com/ \
  GIT_CONFIG_KEY_2=url.https://github.com/webmproject/libwebm.insteadOf GIT_CONFIG_VALUE_2=https://chromium.googlesource.com/webm/libwebm \
  LIBCLANG_PATH="/c/Program Files/LLVM/bin"
cargo check -p rustdesk --lib
```

## 试用

两台 Windows 机器，同一网段或 VPN 内。先在两台机器上都放一份 `landesk-server.exe`（`target/debug/landesk-server.exe`，或 `cargo build --release --bin landesk-server` 后的 `target/release/`）。

**在被控机（Service）上做的前置：** 先在控制机上生成密钥对，把公钥拷过来。

```bash
# 控制机（Master）上：生成全局密钥对，记下打印出的 public key
landesk-server.exe --landesk-genkey master.key

# 被控机（Service）上：用那个公钥启动，监听 21118
landesk-server.exe --server --landesk-listen 21118 --landesk-auth-key <公钥十六进制>
```

控制机需要能连到被控机的 21118（Windows 防火墙要先放行）。

**在控制机上建档并连接：**

```bash
# 1. 一份清单：组, 子组, 显示名, 地址:端口
cat > hosts.csv <<'EOF'
组,子组,显示名,地址
办公室,开发组,工位-01,10.0.0.5:21118
EOF

landesk-server.exe --landesk-book-import hosts.csv     # 导入（按地址:端口覆盖式合并）
landesk-server.exe --landesk-book-list                 # 看清单与每台的指纹状态
landesk-server.exe --landesk-connect 10.0.0.5:21118    # 连过去
landesk-server.exe --landesk-book-verify 10.0.0.5:21118 # 首次连过、核过指纹后标为已验证
```

默认地址簿是 exe 同目录的 `addressbook.json`，私钥是 `private.key`；两者都能用 `--landesk-book` / `--landesk-key` 换位置。

**被拒时看什么：** Service 日志里恰好一行，Master 侧同一句话回显。

| 日志里的原因 | 含义 |
|---|---|
| `no-bound-key` | Service 启动时没给 `--landesk-auth-key` |
| `bad-signature` | 你的私钥与 Service 绑定的公钥不配对 |
| `bad-host-proof` | 对面无法证明它的主机密钥（通常说明它不是 landesk） |
| `host-fingerprint-mismatch` | 这台机器的主机密钥变了：查地址簿里的指纹，确认后删掉该条重连 |
| `no-proof-yet` | 握手没走完（超时或对端不听 landesk） |

**装成开机自启的服务（被控机，管理员权限）：**

```bash
# 1. 先把参数落成配置：写到 exe 同目录的 landesk.json
landesk-server.exe --landesk-save-config --landesk-listen 21118 --landesk-auth-key <公钥十六进制>
# 2. 装成 Windows 服务（上游自带，无需额外安装包）
landesk-server.exe --install-service
```

服务的 `--server` 子进程启动时会读 `landesk.json`，所以不需要把参数写进服务命令行；不带任何参数的 `landesk-server.exe --server` 也会读它。`landesk.json` 里就是两项：`listen_port` 与 `auth_key`。

**本机自测（不用两台机器）：**

```bash
bash scripts/landesk-smoke.sh          # 默认端口 21118
PORT=31218 bash scripts/landesk-smoke.sh   # 本机已有 Service 时换端口
```

它会起一个 Service、用同一个二进制当 Master，把四条路径跑一遍并断言“没有启动 rendezvous”。

**图形界面（Master）：**

```bash
cargo build -p landesk-client      # 纯 Rust 窗口，不需要 Flutter
./target/debug/landesk-client.exe
```

左边是地址簿的组/子组树，选中一台后右側显示地址与指纹状态；“连接（申请操作权）”与“只看”两个按钮做的是同一件事——验证对面主机、用本机私钥证明自己——区别只在后者声明自己不发送输入。连上之后窗口里就是远端画面，鼠标、滚轮、可打印字符、Ctrl/Shift/Alt 组合键与 F1–F12 会发过去；顶部有“断开”，对端有多块屏时还能选看哪一块。会话里会自动开启**剪贴板传文件**（在资源管理器复制文件，到远端粘贴就落盘）。会话断开后窗口自动回到可重连的状态，底部日志里能看到原因。多台 Master 可以同时连一台 Service（同一把私钥），同时操作时后写入者生效。

这个窗口默认读写 exe 同目录的 `addressbook.json` 与 `private.key`——和命令行版一致（把两个文件放到 `landesk-client.exe` 旁边即可）；CSV 导入仍然用命令行的 `--landesk-book-import`。

**打包给别人试：** `cargo build --release …` 后跑 `bash scripts/dist.sh`，得到 `dist/lanDesk-<日期>.zip`：里面只有两个 exe 与一页 `使用说明.txt`（静态 CRT + 静态 vcpkg，不依赖 DLL）。

## 许可

GNU Affero General Public License v3.0。
