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
    listings: Arc<Mutex<Vec<FileListing>>>,
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

    /// 画质：`low` / `balanced` / `best` / `custom`（上游的取值）。
    pub fn set_image_quality(&self, value: &str) {
        self.session.save_image_quality(value.to_owned());
    }

    pub fn image_quality(&self) -> String {
        self.session.get_image_quality()
    }

    /// 帧率上限；只在画质为 `custom` 时单独生效（上游的语义）。
    pub fn set_fps(&self, fps: i32) {
        self.session.set_custom_fps(fps);
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
struct Shared {
    frames: Arc<Mutex<Option<Frame>>>,
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
        seq: Default::default(),
        messages: shared.messages.clone(),
        displays: shared.displays.clone(),
        listings: shared.listings.clone(),
        last_progress: Default::default(),
    };
    session
}

fn spawn_session(session: Session<LandeskUi>, closed: Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(move || {
        // 用上游真正的会话入口：它会建好 Data 通道并把 `handler.sender` 接上。
        // （直接用内层 `Remote` 会让 `Session::send` 静默丢弃，输入与文件都发不出去。）
        crate::ui_session_interface::io_loop(session, 0);
        closed.store(true, std::sync::atomic::Ordering::Relaxed);
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
