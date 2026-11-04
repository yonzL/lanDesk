# lanDesk

局域网、私有网段或 VPN 内的远程桌面：**按地址直连，按密钥鉴权**。没有设备 ID、没有信令、没有中继、没有云账号，也没有用户名。

- **Master（控制端）**：图形界面，持有地址簿与全局私钥，发起连接并操作远端桌面。地址簿按「组 / 子组」两级组织，可用 CSV 批量导入。
- **Service（被控端）**：无图形界面的 Windows 服务，随开机自启，安装时绑定一个公钥，只接受持有对应私钥的 Master。

## 文档

设计先于代码。仓库当前只有设计，没有实现。

| 文档 | 内容 |
|---|---|
| [`goal-and-requirements.md`](goal-and-requirements.md) | 产品目标、能力清单、非目标、与 RustDesk / SubnetDesk / Veyon 的差别 |
| [`architecture.md`](architecture.md) | 直连传输、双挑战身份与 TOFU 指纹、单控制令牌仲裁、RustDesk 模块取舍 |

两份文档都是活的：能力变了、边界动了，就在同一次改动里更新它们。

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

早期。已有：设计文档、RustDesk 基线、`libs/hbb_common` 里的 `keyauth`（挑战—验签 + 主机指纹，含 12 个单测，已接真实 socket 往返），以及一个 `hbb_common` 的 CI 闸门。

未有：主 crate 里的 Service 接线（`--landesk-listen` / `--landesk-auth-key` 与鉴权调用点）、安装器与服务自启、Master 界面。公网路径（`rendezvous_mediator`、`hbbs_http`、自动更新等）仍在树里，随对应的功能替换逐块移除，而不是一次性删掉——它们的删除点集中在会话与鉴权核心（`server/connection.rs`、`ipc.rs`、`flutter_ffi.rs`），先行删除只会得到一个既不能编译也不能运行的树。

## 构建

仅目标 Windows。完整构建需要 Rust 工具链、Flutter（Master 界面）、`libclang`（部分构建脚本用 bindgen 生成绑定）与 vcpkg 提供的原生依赖。

```bash
cargo check -p hbb_common --all-targets   # landesk 唯一改动的共享协议库
cargo check -p rustdesk --lib             # 主 crate
```

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

## 许可

GNU Affero General Public License v3.0。
