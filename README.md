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

## 构建

仅目标 Windows。需要 Rust 工具链与 Flutter（Master 界面）。

```bash
cargo build --release           # Service 与 Master 的 Rust 内核
flutter build windows --release # Master 界面
```

> 基线导入后的具体构建命令随实际裁剪结果调整。

## 许可

GNU Affero General Public License v3.0。
