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

公网路径**已从代码里删除**（见 `architecture.md` 的“移除（已落地）”）：`rendezvous_mediator`、`hbbs_http`、`updater` + `manual_update`、客户端回退拨号、启动时的版本检查。剩下不再被启动的只有 `custom_server.rs`、`auth_2fa.rs`、`trusted_devices` 与若干上游模块，按功能替换逐块清理。

## 构建

仅目标 Windows。完整构建需要 Rust 工具链、`libclang`（`kcp-sys` 用 bindgen）与 vcpkg 的 `opus`/`libvpx`/`aom`/`libyuv`。Flutter 只有想做上游那套界面时才需要；landesk 自己的界面是纯 Rust。

```bash
cargo check -p hbb_common --all-targets          # landesk 唯一改动的共享协议库
cargo build --bin landesk-server -p landesk-client
cargo test  -p rustdesk --test session_smoke     # 端到端：真的收到一帧画面
bash scripts/dist.sh                            # 打一个可拷给别人的包（需先 --release）
```

静态 CRT + 静态 vcpkg，所以打包出来就是两个 exe，不依赖 DLL。`landesk-server.exe` 的双击启动会去拉起上游的 sciter 界面，需要 `sciter.dll`——landesk 模式下不要双击它，只用 `--server` / `--landesk-*` 这些参数。

CI（`.github/workflows/ci.yml`）：`hbb-common` 作业跑共享协议库的 check 与 `keyauth` 测试；`main-crate` 作业准备 vcpkg/libclang 后跑 `cargo check -p rustdesk --lib` 与 `cargo check -p landesk-client`；打 `v*` tag 时会额外跑 `release` 作业出包并发布。端到端会话测试不在 CI 里跑（需要一个可截屏的桌面会话）。

### Windows 构建工具（比网络更容易踩）

三个与代码无关、但会让人卡一天的环境事实：

1. **需要 Visual Studio 的「使用 C++ 的桌面开发」工作负载，外加 Windows SDK 本体。**
   只有 `VC\Tools\MSVC` 目录不够：`vcvars64.bat` 与 CMake 来自工作负载，而 `windows.h` /
   `kernel32.lib` / `stddef.h` 来自 SDK。
   - 缺工作负载：`cc-rs` 报 `Failed to find tool. Is cl.exe installed?`。
   - 缺 SDK（更隐蔽，`cl.exe` 在、`vcvars` 也能跑）：编译期报
     `fatal error C1083: Cannot open include file: 'windows.h'`，链接期报
     `LNK1181: cannot open input file 'kernel32.lib'`。
   - SDK 是否就位的判定：`Test-Path "C:\Program Files (x86)\Windows Kits\10\Include\*\um\windows.h"`。
   - `flutter build windows` 在同样状态下报 `Unable to find suitable Visual Studio toolchain`
     或 `Unable to locate a Windows 10 SDK`。
2. **Git Bash 的 `link.exe` 会抢在 MSVC 的 linker 前面。**
   `C:\Program Files\Git\usr\bin\link.exe` 是 coreutils 的 `link`，被 rustc 当成链接器调用时报
   `link: extra operand`。做法：在 vcvars 环境里构建，或把 MSVC 的
   `…\VC\Tools\MSVC\<ver>\bin\Hostx64\x64` 放到 PATH 最前面。
3. **主 crate 的首次构建需要 `VCPKG_ROOT` 与 `LIBCLANG_PATH`**（见下面的网络与依赖说明）。
### 大陆网络下的前置条件

依赖图里有三类在大陆直连不通的来源：crates.io、GitHub 上的 git 源、以及 `*.googlesource.com`
（vcpkg 里 `aom` / `libyuv` 两个 port 的 `vcpkg_from_git` 源）。下面都是**个人环境**的配置，不入库。

| 阻碍 | 做法 |
|---|---|
| crates.io 约 25KB/s | `~/.cargo/config.toml` 里把 `source.crates-io` 换成 `sparse+https://mirrors.ustc.edu.cn/crates.io-index/`（实测约 220KB/s） |
| GitHub 的 git 源握手失败（schannel） | `GIT_CONFIG_*` 注入 `http.sslBackend=openssl`；然后让 git 走 HTTPS 代理（`HTTPS_PROXY` / `http.proxy`），**或者**用 `url.<你的镜像>/https://github.com/.insteadOf=https://github.com/` 改走镜像 |
| vcpkg 的 `aom` / `libyuv` | 这两个 port 自己 `vcpkg_from_git`：`https://aomedia.googlesource.com/aom`、`https://chromium.googlesource.com/libyuv/libyuv`。要么给 git 配代理，要么对这两个前缀同样用 `insteadOf` 指向带对应提交的镜像 |
| `VCPKG_ROOT` 未设置 | `libs/scrap/build.rs` 与 `magnum-opus` 直接读它，并按 `<VCPKG_ROOT>/installed/<triplet>` 找库。安装**必须在仓库根目录**用清单模式跑（根目录有 `vcpkg.json`，带 `res/vcpkg` 的 overlay 版本钉）：`VCPKG_DEFAULT_HOST_TRIPLET=x64-windows-static $VCPKG_ROOT/vcpkg install --triplet x64-windows-static --x-install-root="$VCPKG_ROOT/installed"`。注意：在根目录跑 `vcpkg install opus:x64-windows-static …` 这种逐包写法会被清单模式直接拒绝 |
| bindgen 找不到 libclang | 安装 LLVM 并设 `LIBCLANG_PATH` 到其 `bin` 目录 |

**关于镜像**：`<你的镜像>` 换成你信任的（自建、付费或组织内部）。公共免费加速镜像意味着这几条
fetch 的 TLS 在镜像处终止；只在你能接受这一点、或事后核对了提交哈希时使用，不要当成默认值。

一条命令的骨架（代理端口与镜像按自己的环境改）：

```bash
export HTTPS_PROXY=http://127.0.0.1:7890 HTTP_PROXY=http://127.0.0.1:7890
export VCPKG_ROOT=/path/to/vcpkg \
  GIT_CONFIG_COUNT=2 \
  GIT_CONFIG_KEY_0=http.sslBackend GIT_CONFIG_VALUE_0=openssl \
  GIT_CONFIG_KEY_1=http.proxy          GIT_CONFIG_VALUE_1="$HTTPS_PROXY" \
  LIBCLANG_PATH="/c/Program Files/LLVM/bin"
cargo check -p rustdesk --lib
```

（`HTTPS_PROXY` 必须在**单独一行**先 export：同一条 `export A=1 B=$A` 里 `$A` 还没生效，
`http.proxy` 会变成空串。）

如果代理对 `googlesource.com` 无效，就把上表的 `insteadOf` 逐条追加成 `GIT_CONFIG_KEY_2/3…`。

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

**先自检，再排障：**

```bash
landesk-server.exe --landesk-doctor
```

只读，不改任何东西；它会一行行告诉你：配置文件在不在、监听端口、绑没绑公钥、本机主机指纹、端口能不能绑，以及还需要人工做的两件事（防火墙、装成服务）。

**被拒时看什么：** Service 日志里一行（带节流），Master 侧会拿到同样的原因。

Service 侧长这样，直接给出“对方是谁、本机允许谁”：

```
landesk: reject key-not-allowed from 192.168.99.23:51834 — presented key B651-2E03-D638-84FA-3271-9151-C65-8AF9; bound 1: 5162-339A-F41B-64E5-85A2-2A1E-20E7-78B2
```

| 日志里的原因 | 含义 |
|---|---|
| `no-bound-key` | Service 还没绑定任何公钥——先 `--landesk-allow-client` 或用公钥目录 |
| `key-not-allowed` | 对方声明的身份不在这台 Service 的允许集合里（`presented key` 就是它） |
| `bad-signature` | 公钥在集合里，但签名对不上（私钥不对 / 挑战被改 / 老客户端没自报公钥且全部验签失败） |
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

服务的 `--server` 子进程启动时会读 `landesk.json`，所以不需要把参数写进服务命令行；不带任何参数的 `landesk-server.exe --server` 也会读它。`landesk.json` 里是 `listen_port`、`auth_keys`（允许接入的 Master 公钥列表）与可选的 `allowed_networks`。

**多台 Master，每台一把密钥：** 每台 Master 用自己的身份（私钥）生成公钥，把公钥分发到各台 Service，然后在 Service 上加入（不需要重启服务，下一次连接就生效）。命名带角色-主机-时间-IP 便于区分：

```bash
landesk-server.exe --landesk-genkey                      # 生成 master-<主机>-<时间>-<IP>.key + .pub
landesk-server.exe --landesk-key-list                    # 看本机有哪些身份
landesk-server.exe --landesk-import-key other.key        # 导入另一把身份
landesk-server.exe --landesk-key-forget master-...       # 卸载一把
```

服务端侧（**只导入公钥**，私钥永远不出 Master 本机）：

```bash
landesk-server.exe --landesk-allow-client <公钥十六进制或 public.key 路径>
landesk-server.exe --landesk-revoke-client <公钥十六进制或 public.key 路径>   # 某台 Master 失窃时
landesk-server.exe --landesk-client-list                                    # 看当前允许哪些
```

旧的单值写法 `auth_key: "<hex>"` 仍然可读，视为列表里的一个元素（那就是“一把钥匙开全网”的形态）。

**服务端的主机身份：** 服务端自己有一对主机密钥（上游管理，私钥不出本机），Master 首次连接时钉住它、用来判断“对面确实是那台机器”。打印或导出公钥：

```bash
landesk-server.exe --landesk-host-key                    # 打印主机公钥与指纹
landesk-server.exe --landesk-host-key host.pub           # 同时导出公钥文件
```

注意：重新生成主机密钥会让所有 Master 已钉的指纹失配，必须逐台重新核对——所以它应该是安装期一次性的动作。

**一步把新 Master 加进多台 Service：** 服务端会自动建好公钥目录 `<exe 目录>/landesk.d/clients/`，里面一个文件一把公钥。在 Master 上执行一次即可（它会先问对端这个目录在哪，再用文件会话把公钥推过去）：

```bash
landesk-server.exe --landesk-enroll 192.168.99.191:21118,192.168.99.192:21118 --landesk-key master.key
```

撤销某台 Master：删掉它那个 `.pub` 文件，或 `--landesk-revoke-client`。两种方式都从下一次连接起生效。

**来源网络白名单（默认只收私有网段）：** 监听端口默认只接受私有地址段的连接（`10/8`、`172.16/12`、`192.168/16`、`100.64/10`、`127/8`、`169.254/16`、`fc00::/7`、`fe80::/10`、`::1/128`），公网来源在握手前就被丢弃并记一行 `rejected connection from disallowed source`。要跨公网用，就显式配一份列表（配了就是**替换**默认值）：

```bash
landesk-server.exe --landesk-save-config --landesk-listen 21118 \
  --landesk-auth-key <公钥十六进制> --landesk-allow-network 203.0.113.0/24,198.51.100.7/32
```

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

**文件分发（扇出）：** 窗口顶部切到「文件分发」标签，在左侧地址簿里勾选多台目标，填远端目录和要推的文件（一行一个本机路径），点「开始分发」。每台一个独立的文件会话并行推同一批文件，进度逐台显示；失败会给出原因。同一页面也支持下载与文件操作（删除/改名/新建目录），目前通过 `landesk-server.exe` 的文件会话 API 驱动（界面暂只做了分发）。

**打包给别人试：** `cargo build --release …` 后跑 `bash scripts/dist.sh`，得到 `dist/lanDesk-<日期>.zip`：里面只有两个 exe 与一页 `使用说明.txt`（静态 CRT + 静态 vcpkg，不依赖 DLL）。

## 许可

GNU Affero General Public License v3.0。
