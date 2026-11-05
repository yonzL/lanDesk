# 第二台机器（Windows VM）验证用的脚本

跑这些脚本之前先确认两件事：

1. VM 里开了 OpenSSH Server，且当前主机的公钥装进了 `administrators_authorized_keys`。
2. 主机能 `ssh <user>@<ip>` 通。桥接网卡时 VM 的 IP 可能每次重连都变，`--` 先 `ipconfig` 看一眼。

| 脚本 | 作用 |
|---|---|
| `setup.ps1` | 把 `C:\lanDesk\landesk-server.exe` 与 `landesk.json` 复制到 `C:\Program Files\lanDesk`（安装脚本的拷贝步骤在本环境里会失败，用它补上） |
| `state.ps1` | 列出 `landesk-server` 进程及其 session、当前登录会话、SYSTEM 侧的日志目录 |
| `clip-probe.ps1` | **在交互式会话里**读剪贴板（文本 + FileDropList）并写 `C:\lanDesk\clip-report.txt` |
| `clip-probe-run.ps1` | 注册一个 `LogonType=Interactive` 的计划任务来执行探针——SSH 落在 session 0，和桌面剪贴板不是同一个 window station，所以必须借计划任务进到 session 1 |

用法（在主机上）：

```bash
scp -i ~/.ssh/landesk_vm scripts/vm/*.ps1 vboxuser@<vm-ip>:C:/lanDesk/
ssh -i ~/.ssh/landesk_vm vboxuser@<vm-ip> \
  'powershell -NoProfile -ExecutionPolicy Bypass -File C:\lanDesk\clip-probe-run.ps1'
```

`clip-probe-run.ps1` 会直接打印报告；`files=` 为空表示剪贴板里没有文件。
