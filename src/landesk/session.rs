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

/// 给即将开始的一次直连登记目标（显式指定地址簿/密钥的调用方用：CLI 的
/// `--landesk-book`/`--landesk-key`、`--landesk-push`与冒烟脚本）。
pub(crate) fn register_connection(peer: &str, book: PathBuf, key: PathBuf, control: bool) {
    register_target(peer, Target { book, key, control });
}

/// 这次直连用哪份地址簿、哪把私钥、是否申请控制权。
///
/// 显式登记过的目标优先（调用方指定的路径）；否则**像 SubnetDesk 的 `Client::start` 那样自己解析**：
/// `peer` 就是 `host:port`，身份取地址簿里该地址钉的那把，没有就用默认私钥。
/// 这样 Flutter 的会话窗口（上游 `session_add`/`session_start` 驱动、独立进程）不需要任何预备调用。
pub(crate) fn connection_inputs(peer: &str) -> ResultType<(PathBuf, PathBuf)> {
    if let Some(target) = take_target(peer) {
        return Ok((target.book, target.key));
    }
    let config = crate::landesk::config();
    let book = config
        .as_ref()
        .map(|config| config.book_path())
        .unwrap_or_else(|| crate::landesk::exe_dir().join("addressbook.json"));
    let key_from_book = crate::addressbook::AddressBook::load(&book)
        .ok()
        .and_then(|book| book.get(peer).and_then(|entry| entry.key.clone()))
        .and_then(|name| crate::landesk::resolve_identity(&name));
    let key = key_from_book
        .or_else(crate::landesk::default_identity)
        .ok_or_else(|| {
            hbb_common::anyhow::anyhow!(
                "landesk: no client identity (import or generate a private key first)"
            )
        })?;
    Ok((book, key))
}

/// 取走并移除：一次连接只用一次。
pub(crate) fn take_target(peer: &str) -> Option<Target> {
    targets().lock().unwrap().remove(peer)
}

/// 交给上游 `Session` 的 UI 回调。除画面外一律不做事——landesk 的这一版只要“看到”。
#[derive(Clone, Default)]
pub struct LandeskUi {
    frames: Arc<Mutex<Option<Arc<Frame>>>>,
    keys_dir: Arc<Mutex<Option<String>>>,
    seq: Arc<std::sync::atomic::AtomicU64>,
    messages: Arc<Mutex<Vec<String>>>,
    displays: Arc<Mutex<Vec<(i32, i32, i32)>>>,
    last_progress: Arc<Mutex<u32>>,
    listings: Arc<Mutex<Vec<FileListing>>>,
    /// 回指自己所在的会话：上游的若干回调（覆盖确认、删除确认）要求界面主动回应。
    session: Arc<Mutex<Option<Session<LandeskUi>>>>,
}

impl LandeskUi {
    /// 记下对端上报的公钥目录（`--landesk-enroll` 用它决定把公钥推到哪里）。
    fn note_platform_additions(&self, data: &str) {
        if data.trim().is_empty() {
            return;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(dir) = value.get("landesk_keys_dir").and_then(|v| v.as_str()) {
                if let Ok(mut slot) = self.keys_dir.lock() {
                    *slot = Some(dir.to_owned());
                }
            }
        }
    }

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
    fn set_peer_info(&self, peer_info: &PeerInfo) {
        // 完整的那份 PeerInfo（含 platform_additions）随登录响应下发，走这里而不是单独那条消息。
        self.note_platform_additions(&peer_info.platform_additions);
        // 一条 info 级别的“我在跟谁说话”：主机名/登录用户/平台/分辨率，便于事后核对。
        let displays = peer_info
            .displays
            .iter()
            .map(|display| format!("{}x{}", display.width, display.height))
            .collect::<Vec<_>>()
            .join(",");
        log::info!(
            "landesk: connected to host={} user={} platform={} version={} displays=[{}]",
            peer_info.hostname,
            peer_info.username,
            peer_info.platform,
            peer_info.version,
            displays
        );
    }
    fn set_displays(&self, displays: &Vec<DisplayInfo>) {
        let mut slots = self.displays.lock().unwrap();
        slots.clear();
        for (index, display) in displays.iter().enumerate() {
            slots.push((index as i32, display.width, display.height));
        }
    }
    fn set_platform_additions(&self, data: &str) {
        self.note_platform_additions(data);
    }
    fn on_connected(&self, _conn_type: ConnType) {}
    fn update_privacy_mode(&self) {}
    fn set_permission(&self, name: &str, value: bool) {
        // 对端下发的权限变化：声音/剪贴板/文件这些直接决定能不能用，记一条便于排查。
        log::info!("landesk: peer permission {name} = {value}");
    }
    #[cfg(feature = "flutter")]
    fn is_multi_ui_session(&self) -> bool {
        // 一个进程只开一个会话窗口（会话多开是“每会话一个进程”），所以不存在多 UI 共用。
        false
    }
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
        entries: &Vec<FileEntry>,
        path: String,
        is_local: bool,
        only_count: bool,
    ) {
        if only_count {
            return;
        }
        self.listings.lock().unwrap().push(FileListing {
            path,
            is_remote: !is_local,
            entries: entries.clone(),
        });
    }
    fn confirm_delete_files(&self, id: i32, _i: i32, name: String) {
        // 无人值守的文件会话：删除前不再弹窗，记一行日志即可。
        self.note(format!("删除已确认：{name}"));
        // `FileManager::confirm_delete_files` 在 flutter/移动端不存在（那边的文件管理器在 UI 层），
        // 所以这里按同样的条件门控，避免 `--features flutter` 下编译失败。
        #[cfg(not(any(target_os = "android", target_os = "ios", feature = "flutter")))]
        {
            use crate::client::file_trait::FileManager;
            if let Some(session) = self.session.lock().unwrap().as_ref() {
                session.confirm_delete_files(id, 0);
            }
        }
    }
    fn override_file_confirm(
        &self,
        id: i32,
        file_num: i32,
        to: String,
        is_upload: bool,
        _is_identical: bool,
    ) {
        // 重部署时目标目录里通常已经有同名文件；无人值守的会话直接覆盖（并记住这个选择）。
        // `set_confirm_override_file` 在 `file_trait.rs` 里**没有** cfg 门控（只有 `confirm_delete_files` 有），
        // 所以这里也不能门控：否则 `--features flutter` 下覆盖确认发不出去，推送会卡在对端等确认。
        self.note(format!("覆盖已存在的文件：{to}"));
        {
            use crate::client::file_trait::FileManager;
            if let Some(session) = self.session.lock().unwrap().as_ref() {
                session.set_confirm_override_file(id, file_num, true, true, is_upload);
            }
        }
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
        // 上游解码出来的不是 RGBA：Windows 上是 ARGB（字节序 B,G,R,A），只有 Flutter 构建才是 ABGR。
        // 这里统一成界面能直接用的 RGBA，并且把 alpha 强制为不透明。
        let (width, height) = (rgba.w, rgba.h);
        let Some(pixels) = to_rgba(rgba, width, height) else {
            return;
        };
        let frame = Frame {
            seq,
            display,
            width,
            height,
            rgba: pixels,
        };
        if seq == 1 {
            log::info!(
                "landesk: first frame {}x{} (source format {:?})",
                frame.width,
                frame.height,
                rgba.fmt
            );
        }
        *self.frames.lock().unwrap() = Some(Arc::new(frame));
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

#[derive(Clone)]
/// 一次 landesk 会话的把手：界面从这里取最新画面、发输入。
pub struct SessionHandle {
    frames: Arc<Mutex<Option<Arc<Frame>>>>,
    keys_dir: Arc<Mutex<Option<String>>>,
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

    /// 最新一帧（共享引用，界面不解一份拷贝；1080p 一份就是 8MB）。
    pub fn latest_frame(&self) -> Option<Arc<Frame>> {
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

    /// 对端上报的 client 公钥目录（`--landesk-enroll` 用它决定把公钥推到哪里）。
    pub fn remote_keys_dir(&self) -> Option<String> {
        self.keys_dir.lock().unwrap().clone()
    }

    /// 对端报告过的显示器（序号、宽、高）。多于一台时界面给出切换。
    pub fn displays(&self) -> Vec<(i32, i32, i32)> {
        self.displays.lock().unwrap().clone()
    }

    pub fn switch_display(&self, display: i32) {
        self.session.switch_display(display);
    }

    /// 剪贴板传文件（Windows 双向，上游叫 file copy-paste）。
    ///
    /// 开了之后在资源管理器里复制文件、到远端粘贴就能直接落盘——这就是“剪贴板传文件”，
    /// 不需要专设的文件传输会话。
    pub fn file_clipboard_enabled(&self) -> bool {
        self.session
            .get_toggle_option("enable-file-copy-paste".to_owned())
    }

    pub fn set_file_clipboard(&self, on: bool) {
        if self.file_clipboard_enabled() != on {
            self.session.toggle_option("enable-file-copy-paste".to_owned());
        }
    }

    /// 编码器偏好：`auto` / `vp8` / `vp9` / `av1`（上游选项，下一次连接生效）。
    pub fn codec_preference(&self) -> String {
        self.session.get_option("codec-preference".to_owned())
    }

    pub fn set_codec_preference(&self, value: &str) {
        self.session
            .set_option("codec-preference".to_owned(), value.to_owned());
        // 上游在 Flutter 里接着调这个，否则只改了配置、本次会话不变。
        self.session.update_supported_decodings();
    }

    /// 画质：`low` / `balanced` / `best` / `custom`（上游的取值）。
    pub fn set_image_quality(&self, value: &str) {
        self.session.save_image_quality(value.to_owned());
    }

    /// 声音（对端来的音频）；`get_toggle_option("disable-audio")` 为真表示已关。
    pub fn audio_enabled(&self) -> bool {
        !self.session.get_toggle_option("disable-audio".to_owned())
    }

    pub fn set_audio_enabled(&self, on: bool) {
        if self.audio_enabled() != on {
            self.session.toggle_option("disable-audio".to_owned());
        }
    }

    /// 文本剪贴板同步。
    pub fn clipboard_enabled(&self) -> bool {
        !self.session.get_toggle_option("disable-clipboard".to_owned())
    }

    pub fn set_clipboard_enabled(&self, on: bool) {
        if self.clipboard_enabled() != on {
            self.session.toggle_option("disable-clipboard".to_owned());
        }
    }

    pub fn image_quality(&self) -> String {
        self.session.get_image_quality()
    }

    /// 帧率上限；只在画质为 `custom` 时单独生效（上游的语义）。
    pub fn set_fps(&self, fps: i32) {
        self.session.set_custom_fps(fps);
    }

    /// 把会话切到“后台”状态：降低本进程优先级，并把请求帧率压到后台值。
    ///
    /// 一个 Master 可能同时开十几个窗口，后台窗口不该和前台争 CPU。恢复时把帧率交还给
    /// `foreground_fps`（界面当前的帧率上限），上游的自适应 `fps_control` 会从那里自己爬回去。
    pub fn set_background(&self, background: bool, foreground_fps: i32) {
        crate::landesk::priority::set_background(background);
        let fps = if background {
            crate::landesk::priority::BACKGROUND_FPS
        } else {
            foreground_fps.max(crate::landesk::priority::BACKGROUND_FPS)
        };
        self.session.set_custom_fps(fps);
    }

    /// 自定义画质的百分比（上游 `custom_image_quality`，10–100）。
    pub fn custom_image_quality(&self) -> i32 {
        self.session
            .get_custom_image_quality()
            .first()
            .copied()
            .unwrap_or(50)
    }

    pub fn set_custom_image_quality(&self, value: i32) {
        self.session.save_custom_image_quality(value);
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
    let shared = Shared::new();
    let session = build_session(&peer, &book, &key, control, &shared);
    session.lc.write().unwrap().initialize(
        peer.clone(),
        ConnType::DEFAULT_CONN,
        None,
        false,
        None,
        None,
        None,
    );
    let handle = SessionHandle {
        frames: shared.frames.clone(),
        keys_dir: shared.keys_dir.clone(),
        messages: shared.messages.clone(),
        closed: shared.closed.clone(),
        displays: shared.displays.clone(),
        session: session.clone(),
    };
    spawn_session(session, shared.closed.clone());
    Ok(handle)
}

/// 一次文件会话的结果快照。
#[derive(Debug, Clone)]
pub struct FileListing {
    pub path: String,
    pub is_remote: bool,
    pub entries: Vec<FileEntry>,
}

/// 文件会话的把手：浏览远端目录，并往里推文件。
pub struct FileSessionHandle {
    messages: Arc<Mutex<Vec<String>>>,
    listings: Arc<Mutex<Vec<FileListing>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    session: Session<LandeskUi>,
}

impl FileSessionHandle {
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn drain_messages(&self) -> Vec<String> {
        std::mem::take(&mut *self.messages.lock().unwrap())
    }

    pub fn take_listing(&self) -> Option<FileListing> {
        self.listings.lock().unwrap().pop()
    }

    /// 列举远端目录（结果通过 `take_listing` 取）。
    pub fn list(&self, path: &str) {
        use crate::client::file_trait::FileManager;
        self.session
            .set_option("remote_dir".to_owned(), path.to_owned());
        self.session.read_remote_dir(path.to_owned(), false);
    }

    /// 把一个本机路径推到远端的**完整目标路径**（含文件名）。
    ///
    /// 参数约定与上游界面一致：`to` = 目标目录 + 名字，`file_num = 0`。
    pub fn upload(&self, local_path: &str, remote_dest: &str) {
        use crate::client::file_trait::FileManager;
        let job_id = base::fs::get_next_job_id();
        self.session.send_files(
            job_id,
            base::fs::JobType::Generic as i32,
            local_path.to_owned(),
            remote_dest.to_owned(),
            0,
            false,
            false,
        );
    }

    /// 把远端路径拉到本机目录（`to` 同样带名字）。
    pub fn download(&self, remote_path: &str, local_dest: &str) {
        use crate::client::file_trait::FileManager;
        let job_id = base::fs::get_next_job_id();
        self.session.send_files(
            job_id,
            base::fs::JobType::Generic as i32,
            remote_path.to_owned(),
            local_dest.to_owned(),
            0,
            false,
            true,
        );
    }

    pub fn delete(&self, remote_path: &str, is_dir: bool) {
        use crate::client::file_trait::FileManager;
        let job_id = base::fs::get_next_job_id();
        if is_dir {
            self.session.remove_dir(job_id, remote_path.to_owned(), true);
        } else {
            self.session
                .remove_file(job_id, remote_path.to_owned(), 0, true);
        }
    }

    /// 改名或移动（`new_name` 为相对当前目录的新名字）。
    pub fn rename(&self, remote_path: &str, new_name: &str) {
        use crate::client::file_trait::FileManager;
        let act_id = base::fs::get_next_job_id();
        self.session
            .rename_file(act_id, remote_path.to_owned(), new_name.to_owned(), true);
    }

    pub fn create_dir(&self, remote_path: &str) {
        use crate::client::file_trait::FileManager;
        let job_id = base::fs::get_next_job_id();
        self.session
            .create_dir(job_id, remote_path.to_owned(), true);
    }

    pub fn close(&self) {
        self.session.close();
    }
}

/// 起一次文件会话：同一个 peer、同一条已认证的路，只是登录成 `FILE_TRANSFER`。
pub fn start_file_transfer(
    peer: String,
    book: PathBuf,
    key: PathBuf,
    control: bool,
    remote_dir: String,
) -> ResultType<FileSessionHandle> {
    let shared = Shared::new();
    let session = build_session(&peer, &book, &key, control, &shared);
    {
        let mut lc = session.lc.write().unwrap();
        lc.initialize(
            peer.clone(),
            ConnType::FILE_TRANSFER,
            None,
            false,
            None,
            None,
            None,
        );
        lc.set_option("remote_dir".to_owned(), remote_dir);
    }
    let handle = FileSessionHandle {
        messages: shared.messages.clone(),
        listings: shared.listings.clone(),
        closed: shared.closed.clone(),
        session: session.clone(),
    };
    spawn_session(session, shared.closed.clone());
    Ok(handle)
}

/// 一次会话里被界面读走的共享数据。
/// 把解码器给的像素换成界面直接能用的 RGBA（每像素 4 字节、alpha 不透明）。
///
/// `ImageFormat::ARGB` 的字节序是 B,G,R,A；`ABGR` 才是 R,G,B,A；`Raw` 是 3 字节且行可能被对齐填充。
fn to_rgba(rgba: &scrap::ImageRgb, width: usize, height: usize) -> Option<Vec<u8>> {
    use scrap::ImageFormat;
    if width == 0 || height == 0 {
        return None;
    }
    let bytes_per_row = rgba.raw.len() / height;
    let (bpp, order) = match rgba.fmt {
        ImageFormat::Raw => (3usize, None),
        ImageFormat::ABGR => (4, Some(false)),
        ImageFormat::ARGB => (4, Some(true)),
    };
    if bytes_per_row < width * bpp {
        return None;
    }
    let mut out = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let start = row * bytes_per_row;
        let line = &rgba.raw[start..start + width * bpp];
        match bpp {
            4 => {
                let swap = order == Some(true);
                for px in line.chunks_exact(4) {
                    if swap {
                        out.extend_from_slice(&[px[2], px[1], px[0], 0xff]);
                    } else {
                        out.extend_from_slice(&[px[0], px[1], px[2], 0xff]);
                    }
                }
            }
            _ => {
                for px in line.chunks_exact(3) {
                    out.extend_from_slice(&[px[0], px[1], px[2], 0xff]);
                }
            }
        }
    }
    Some(out)
}

struct Shared {
    frames: Arc<Mutex<Option<Arc<Frame>>>>,
    /// 对端上报的 client 公钥目录（landesk 模式；用于 `--landesk-enroll` 把公钥推过去）。
    keys_dir: Arc<Mutex<Option<String>>>,
    messages: Arc<Mutex<Vec<String>>>,
    displays: Arc<Mutex<Vec<(i32, i32, i32)>>>,
    listings: Arc<Mutex<Vec<FileListing>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl Shared {
    fn new() -> Self {
        Self {
            frames: Arc::new(Mutex::new(None)),
            messages: Arc::new(Mutex::new(Vec::new())),
            displays: Arc::new(Mutex::new(Vec::new())),
            keys_dir: Arc::new(Mutex::new(None)),
            listings: Arc::new(Mutex::new(Vec::new())),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

fn build_session(
    peer: &str,
    book: &PathBuf,
    key: &PathBuf,
    control: bool,
    shared: &Shared,
) -> Session<LandeskUi> {
    register_target(
        peer,
        Target {
            book: book.clone(),
            key: key.clone(),
            control,
        },
    );
    let mut session = Session::<LandeskUi>::default();
    session.ui_handler = LandeskUi {
        frames: shared.frames.clone(),
        keys_dir: shared.keys_dir.clone(),
        seq: Default::default(),
        messages: shared.messages.clone(),
        displays: shared.displays.clone(),
        listings: shared.listings.clone(),
        session: Default::default(),
        last_progress: Default::default(),
    };
    *session.ui_handler.session.lock().unwrap() = Some(session.clone());
    // 上游的 Flutter / Sciter 在构造会话时把三个服务端权限标志显式置为 true
    // （`flutter.rs:1358-1360`、`ui/remote.rs:600-603`），`Session::default()` 里它们是 false。
    // `is_file_clipboard_required()` 同时读 keyboard、file 与 `enable_file_copy_paste`，
    // 任何一个为 false 都会让客户端 `stop` 掉文件剪贴板。
    *session.server_keyboard_enabled.write().unwrap() = true;
    *session.server_file_transfer_enabled.write().unwrap() = true;
    *session.server_clipboard_enabled.write().unwrap() = true;
    // 编码器偏好**不在这里动**：界面上的 `auto` 就是上游的 `auto`，由服务端按它的规则挑。
    // 我们要解决的只是“auto 会选 AV1”——那在服务端侧关掉（`landesk::apply_service_defaults`
    // 把 `av1-test` 置为 N），于是 auto 落到 VP9，而总内存 ≤4GB 时上游仍会改选 VP8。
    session
}

fn spawn_session(session: Session<LandeskUi>, closed: Arc<std::sync::atomic::AtomicBool>) {
    // 文件剪贴板这条链的客户端侧前置条件，开一条日志，免得再靠猜。
    {
        let lc = session.lc.read().unwrap();
        log::info!(
            "landesk: file clipboard state — server_file_transfer_enabled: {}, enable_file_copy_paste: {}, is_default: {}",
            *session.server_file_transfer_enabled.read().unwrap(),
            lc.enable_file_copy_paste.v,
            session.is_default(),
        );
    }
    spawn_clipboard_poller(session.clone(), closed.clone());
    std::thread::spawn(move || {
        // 用上游真正的会话入口：它会建好 Data 通道并把 `handler.sender` 接上。
        // （直接用内层 `Remote` 会让 `Session::send` 静默丢弃，输入与文件都发不出去。）
        crate::ui_session_interface::io_loop(session, 0);
        closed.store(true, std::sync::atomic::Ordering::Relaxed);
    });
}

/// 轮询本机剪贴板并把变化发给对端。
///
/// 上游的 `clipboard_listener` 在非 Flutter UI 里不报变更（实测：线程启动了但一条事件也没有），
/// 所以这里自己读、自己去重、自己发；`ClipboardSide::Client` 表示“这是控制端本机的内容”。
fn spawn_clipboard_poller(session: Session<LandeskUi>, closed: Arc<std::sync::atomic::AtomicBool>) {
    use crate::client::Interface;
    std::thread::spawn(move || {
        let mut ctx = None;
        // 首次读到的是“连之前就在剪贴板里的东西”（可能是密码），只当基线不发送：
        // 上游同样把首次同步门控在 `sync_init_clipboard` 之后。
        let mut primed = false;
        let mut last_sent: Vec<u8> = Vec::new();
        while !closed.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(400));
            // 会话里关了剪贴板就别再往对端发（与上游 `disable-clipboard` 语义一致）。
            if session.get_toggle_option("disable-clipboard".to_owned()) {
                continue;
            }
            let Some(msg) = crate::clipboard::check_clipboard(
                &mut ctx,
                crate::clipboard::ClipboardSide::Client,
                false,
            ) else {
                continue;
            };
            // 上游读出来的是一条 `MultiClipboards`，内容相同就不重复发（也避免回声互相触发）。
            let Some(base::message_proto::message::Union::MultiClipboards(mcb)) =
                msg.union.as_ref()
            else {
                continue;
            };
            let mut sig = Vec::new();
            for cb in &mcb.clipboards {
                sig.extend_from_slice(&cb.content);
            }
            if sig.is_empty() || sig == last_sent {
                continue;
            }
            if !primed {
                primed = true;
                last_sent = sig;
                log::debug!("landesk: clipboard baseline taken, not sent");
                continue;
            }
            log::debug!(
                "landesk: sending local clipboard ({} clipboards, {} bytes)",
                mcb.clipboards.len(),
                sig.len()
            );
            last_sent = sig;
            session.send(crate::client::Data::Message(msg));
        }
    });
}

/// 一台目标机器的分发进度。
#[derive(Debug, Clone)]
pub struct TargetReport {
    pub peer: String,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    /// 最近一条失败原因（有就说明这台出过问题）。
    pub last_error: Option<String>,
}

/// 一次扇出分发：同一批文件同时推给多台 Service。
pub struct Distribution {
    reports: Arc<Mutex<Vec<TargetReport>>>,
    finished: Arc<std::sync::atomic::AtomicBool>,
}

impl Distribution {
    /// 当前的逐台进度快照。
    pub fn snapshot(&self) -> Vec<TargetReport> {
        self.reports.lock().unwrap().clone()
    }

    pub fn finished(&self) -> bool {
        self.finished.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// 把 `files`（本机路径）逐个推到每台 `peers` 上的 `remote_dir`。
///
/// 每台机器一个独立的文件会话，彼此并行；这条调用立即返回，进度从 `Distribution` 里读。
pub fn distribute(
    peers: Vec<String>,
    book: PathBuf,
    key: PathBuf,
    files: Vec<String>,
    remote_dir: String,
) -> Distribution {
    let reports = Arc::new(Mutex::new(
        peers
            .iter()
            .map(|peer| TargetReport {
                peer: peer.clone(),
                total: files.len(),
                done: 0,
                failed: 0,
                last_error: None,
            })
            .collect::<Vec<_>>(),
    ));
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(peers.is_empty()));
    let mut handles = Vec::new();
    for (index, peer) in peers.into_iter().enumerate() {
        let reports = reports.clone();
        let book = book.clone();
        let key = key.clone();
        let files = files.clone();
        let remote_dir = remote_dir.clone();
        let handle = std::thread::spawn(move || {
            let session =
                match start_file_transfer(peer.clone(), book, key, true, remote_dir.clone()) {
                    Ok(session) => session,
                    Err(err) => {
                        let mut reports = reports.lock().unwrap();
                        reports[index].failed = files.len();
                        reports[index].last_error = Some(format!("{err:#}"));
                        return;
                    }
                };
            // 等登录完成：列一次目录，拿到结果就说明文件会话已就绪。
            let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut ready = false;
            while std::time::Instant::now() < ready_deadline {
                session.list(&remote_dir);
                std::thread::sleep(std::time::Duration::from_millis(200));
                if session.take_listing().is_some() {
                    ready = true;
                    break;
                }
            }
            if !ready {
                let mut reports = reports.lock().unwrap();
                reports[index].failed = files.len();
                reports[index].last_error = Some("文件会话没有就绪".to_owned());
                return;
            }
            for file in &files {
                let name = std::path::Path::new(file)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let dest = format!("{}\\{}", remote_dir, name);
                session.upload(file, &dest);
            }
            // 等每个 job 的结果：完成/失败都会作为消息进来。
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            let mut settled = 0;
            while settled < files.len() && std::time::Instant::now() < deadline {
                for message in session.drain_messages() {
                    if message.contains("文件传输完成") {
                        settled += 1;
                        let mut reports = reports.lock().unwrap();
                        reports[index].done += 1;
                    } else if message.contains("文件传输失败") {
                        settled += 1;
                        let mut reports = reports.lock().unwrap();
                        reports[index].failed += 1;
                        reports[index].last_error = Some(message);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if settled < files.len() {
                let mut reports = reports.lock().unwrap();
                reports[index].failed += files.len() - settled;
                reports[index].last_error.get_or_insert_with(|| "等待传输结果超时".to_owned());
            }
            session.close();
        });
        handles.push(handle);
    }
    {
        let finished = finished.clone();
        std::thread::spawn(move || {
            for handle in handles {
                handle.join().ok();
            }
            finished.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }
    Distribution { reports, finished }
}
