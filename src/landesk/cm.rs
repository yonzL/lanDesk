//! 无界面的连接管理器（CM）进程。
//!
//! 上游把"需要用户身份才能做的事"（读本机剪贴板、文件剪贴板、UAC、聊天/语音转发）交给一个以
//! 用户令牌运行、监听 `_cm` 管道的进程。我们的二进制没有 UI 外壳，`--cm` 起不来，所以这里用
//! 同一套业务逻辑（`ui_cm_interface::start_ipc`）配一个不画界面的 `InvokeUiCM` 实现。
//!
//! 只做转发与用户身份操作；是否允许连接仍由密钥握手决定（`connection.rs` 里自授权），本进程不参与。

use crate::ui_cm_interface::{Client, ConnectionManager, InvokeUiCM};
use hbb_common::log;

/// 只记日志的 CM 界面实现：`InvokeUiCM` 的 8 个回调全是上游 UI 的通知点。
#[derive(Clone)]
pub struct HeadlessCm;

impl InvokeUiCM for HeadlessCm {
    fn add_connection(&self, client: &Client) {
        log::info!(
            "landesk cm: connection {} from {} (authorized: {})",
            client.id,
            client.peer_id,
            client.authorized
        );
    }

    fn remove_connection(&self, id: i32, close: bool) {
        log::debug!("landesk cm: connection {} removed (close: {})", id, close);
    }

    fn new_message(&self, id: i32, _text: String) {
        // 聊天文本由对端控制频率、而且是用户内容：只记“有消息”，不写正文，级别压到 trace。
        log::trace!("landesk cm: chat message for {id}");
    }

    fn change_theme(&self, _dark: String) {}

    fn change_language(&self) {}

    fn show_elevation(&self, _show: bool) {}

    fn update_voice_call_state(&self, _client: &Client) {}

    fn file_transfer_log(&self, action: &str, _log: &str) {
        // 每个 Data::FS 块都会调到这里，且带着整份 transfer-jobs 序列化文本：
        // debug 级会把 16MB 的日志轮转冲满，所以只留 trace 与动作名。
        log::trace!("landesk cm: {} update", action);
    }
}

/// `--landesk-cm` 入口：常驻监听 `_cm`，直到进程结束。
pub fn run() {
    let cm = ConnectionManager {
        ui_handler: HeadlessCm {},
    };
    log::info!("landesk cm: listening on _cm");
    crate::ui_cm_interface::start_ipc(cm);
}
