//! landesk 的会话桥：把已认证的连接交给上游的客户端主循环（登录、解码、输入都在那里），
//! 再把画面帧交给我们自己的界面。
//!
//! 这里刻意只做“接线”：不重写登录、视频解码、输入编码——那些在上游 `Remote`/`Session` 里。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base::message_proto::*;
use hbb_common::rendezvous_proto::ConnType;
use hbb_common::{log, ResultType};

use crate::client::helper::QualityStatus;
use crate::ui_session_interface::{InvokeUiSession, Session};


/// 一帧画面。`rgba` 是上游解码后的原始像素，界面自己上传纹理。
#[derive(Debug, Clone)]
pub struct Frame {
    /// 递增序号，界面据此判断是不是新帧（避免每秒重传同一张纹理）。
    pub seq: u64,
    pub display: usize,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// 这个进程里某次 landesk 连接的目标信息，供 `Client::_start` 判断“这次不是 rendezvous 连接”。
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub book: PathBuf,
    pub key: PathBuf,
    pub control: bool,
}

fn targets() -> &'static Mutex<HashMap<String, Target>> {
    use std::sync::OnceLock;
    static TARGETS: OnceLock<Mutex<HashMap<String, Target>>> = OnceLock::new();
    TARGETS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn register_target(peer: &str, target: Target) {
    targets().lock().unwrap().insert(peer.to_owned(), target);
}

/// 取走并移除：一次连接只用一次。
pub(crate) fn take_target(peer: &str) -> Option<Target> {
    targets().lock().unwrap().remove(peer)
}

/// 交给上游 `Session` 的 UI 回调。除画面外一律不做事——landesk 的这一版只要“看到”。
#[derive(Clone, Default)]
pub struct LandeskUi {
    frames: Arc<Mutex<Option<Frame>>>,
    seq: Arc<std::sync::atomic::AtomicU64>,
    messages: Arc<Mutex<Vec<String>>>,
    displays: Arc<Mutex<Vec<(i32, i32, i32)>>>,
    last_progress: Arc<Mutex<u32>>,
}

impl LandeskUi {
    fn note(&self, message: String) {
        log::info!("landesk: {message}");
        self.messages.lock().unwrap().push(message);
    }
}

impl InvokeUiSession for LandeskUi {
    fn set_cursor_data(&self, _cd: CursorData) {}
    fn set_cursor_id(&self, _id: String) {}
    fn set_cursor_position(&self, _cp: CursorPosition) {}
    fn set_display(&self, _x: i32, _y: i32, _w: i32, _h: i32, _cursor_embedded: bool, _scale: f64) {}
    fn switch_display(&self, _display: &SwitchDisplay) {}
    fn set_peer_info(&self, _peer_info: &PeerInfo) {}
    fn set_displays(&self, displays: &Vec<DisplayInfo>) {
        let mut slots = self.displays.lock().unwrap();
        slots.clear();
        for (index, display) in displays.iter().enumerate() {
            slots.push((index as i32, display.width, display.height));
        }
    }
    fn set_platform_additions(&self, _data: &str) {}
    fn on_connected(&self, _conn_type: ConnType) {}
    fn update_privacy_mode(&self) {}
    fn set_permission(&self, _name: &str, _value: bool) {}
    fn close_success(&self) {}
    fn update_quality_status(&self, _qs: QualityStatus) {}
    fn set_connection_type(&self, _is_secured: bool, _direct: bool, _stream_type: &str) {}
    fn set_fingerprint(&self, _fingerprint: String) {}
    fn job_error(&self, _id: i32, err: String, _file_num: i32) {
        self.note(format!("文件传输失败：{err}"));
    }
    fn job_done(&self, _id: i32, file_num: i32) {
        self.note(format!("文件传输完成（{file_num} 个文件）"));
    }
    fn clear_all_jobs(&self) {}
    fn new_message(&self, _msg: String) {}
    fn update_transfer_list(&self) {}
    fn load_last_job(&self, _cnt: i32, _job_json: &str, _auto_start: bool) {}
    fn update_folder_files(
        &self,
        _id: i32,
        _entries: &Vec<FileEntry>,
        _path: String,
        _is_local: bool,
        _only_count: bool,
    ) {
    }
    fn confirm_delete_files(&self, _id: i32, _i: i32, _name: String) {}
    fn override_file_confirm(
        &self,
        _id: i32,
        _file_num: i32,
        _to: String,
        _is_upload: bool,
        _is_identical: bool,
    ) {
    }
    fn update_block_input_state(&self, _on: bool) {}
    fn job_progress(&self, _id: i32, _file_num: i32, speed: f64, finished_size: f64) {
        // 只按 10% 台阶报进度，不然日志会被刷满。
        let percent = (finished_size / 10_000.0) as u32;
        let mut last = self.last_progress.lock().unwrap();
        if percent >= *last + 10 {
            *last = percent;
            self.note(format!("传输中：约 {percent}%（{:.1} KB/s）", speed));
        }
    }
    fn adapt_size(&self) {}
    fn on_rgba(&self, display: usize, rgba: &mut scrap::ImageRgb) {
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let frame = Frame {
            seq,
            display,
            width: rgba.w,
            height: rgba.h,
            rgba: rgba.raw.clone(),
        };
        if seq == 1 {
            log::info!("landesk: first frame {}x{}", frame.width, frame.height);
        }
        *self.frames.lock().unwrap() = Some(frame);
    }
    fn msgbox(&self, msgtype: &str, _title: &str, text: &str, _link: &str, _retry: bool) {
        log::warn!("landesk: peer message ({msgtype}): {text}");
        // 界面把这条显示出来：连不上时用户在窗口里就能看到原因，不必去翻日志文件。
        if !text.trim().is_empty() {
            self.messages
                .lock()
                .unwrap()
                .push(format!("{msgtype}: {text}"));
        }
    }
    fn cancel_msgbox(&self, _tag: &str) {}
    fn switch_back(&self, _id: &str) {}
    fn portable_service_running(&self, _running: bool) {}
    fn on_voice_call_started(&self) {}
    fn on_voice_call_closed(&self, _reason: &str) {}
    fn on_voice_call_waiting(&self) {}
    fn on_voice_call_incoming(&self) {}
    fn get_rgba(&self, _display: usize) -> *const u8 {
        std::ptr::null()
    }
    fn next_rgba(&self, _display: usize) {}
    fn set_multiple_windows_session(&self, _sessions: Vec<WindowsSession>) {}
    fn set_current_display(&self, _disp_idx: i32) {}
    fn update_record_status(&self, _start: bool) {}
    fn printer_request(&self, _id: i32, _path: String) {}
    fn handle_screenshot_resp(&self, _sid: String, _msg: String) {}
    fn handle_terminal_response(&self, _response: TerminalResponse) {}
}

/// 一次 landesk 会话的把手：界面从这里取最新画面、发输入。
pub struct SessionHandle {
    frames: Arc<Mutex<Option<Frame>>>,
    messages: Arc<Mutex<Vec<String>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    displays: Arc<Mutex<Vec<(i32, i32, i32)>>>,
    session: Session<LandeskUi>,
}

impl SessionHandle {
    /// 会话是否已经结束（对端断开、认证被拒、登录失败等）。界面据此回到可重连的状态。
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 取走并清空会话期间的提示（“已连接”“密码不对”之类）。
    pub fn drain_messages(&self) -> Vec<String> {
        let mut messages = self.messages.lock().unwrap();
        std::mem::take(&mut *messages)
    }
    pub fn latest_frame(&self) -> Option<Frame> {
        self.frames.lock().unwrap().clone()
    }

    /// 只看序号：界面用它判断要不要去拷一帧（拷贝一帧 1080p 约 8MB）。
    pub fn frame_seq(&self) -> u64 {
        self.frames
            .lock()
            .unwrap()
            .as_ref()
            .map(|frame| frame.seq)
            .unwrap_or(0)
    }

    /// 对端报告过的显示器（序号、宽、高）。多于一台时界面给出切换。
    pub fn displays(&self) -> Vec<(i32, i32, i32)> {
        self.displays.lock().unwrap().clone()
    }

    pub fn switch_display(&self, display: i32) {
        self.session.switch_display(display);
    }

    pub fn send_mouse(
        &self,
        mask: i32,
        x: i32,
        y: i32,
        alt: bool,
        ctrl: bool,
        shift: bool,
        command: bool,
    ) {
        self.session.send_mouse(mask, x, y, alt, ctrl, shift, command);
    }

    pub fn send_key(&self, evt: &KeyEvent) {
        self.session.send_key_event(evt);
    }

    pub fn close(&self) {
        self.session.close();
    }
}

/// 起一次会话：登记目标 → 交给上游 `Remote::io_loop`。
///
/// 连接本身由 `Client::_start` 里的 landesk 分支取，走的是我们已认证的那条流。
pub fn start(peer: String, book: PathBuf, key: PathBuf, control: bool) -> ResultType<SessionHandle> {
    register_target(&peer, Target { book, key, control });

    let frames = Arc::new(Mutex::new(None));
    let messages = Arc::new(Mutex::new(Vec::new()));
    let displays = Arc::new(Mutex::new(Vec::new()));
    let mut session = Session::<LandeskUi>::default();
    session.ui_handler = LandeskUi {
        frames: frames.clone(),
        seq: Default::default(),
        messages: messages.clone(),
        displays: displays.clone(),
        last_progress: Default::default(),
    };
    session.lc.write().unwrap().initialize(
        peer.clone(),
        ConnType::DEFAULT_CONN,
        None,
        false,
        None,
        None,
        None,
    );
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handle = SessionHandle {
        frames: frames.clone(),
        messages,
        closed: closed.clone(),
        displays,
        session: session.clone(),
    };

    std::thread::spawn(move || {
        // 用上游真正的会话入口：它会建好 Data 通道并把 `handler.sender` 接上。
        // （直接用内层 `Remote` 会让 `Session::send` 静默丢弃，输入与文件都发不出去。）
        crate::ui_session_interface::io_loop(session, 0);
        closed.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    Ok(handle)
}
