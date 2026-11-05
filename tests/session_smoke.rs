//! 会话冒烟：在本机起一个 Service，把同一份二进制当 Master 连上去，断言真的收到了画面帧。
//!
//! 这是"能不能远程看到画面"的可重复证据——不需要人手点界面。
//! 需要先构建二进制；测试会用 `CARGO_BIN_EXE_landesk-server` 找到它。

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hbb_common::keyauth::GlobalKeyPair;
use librustdesk::landesk::public_key_hex;

struct Service {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Service {
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 测试进程里把日志打到 stderr——库里 `log::` 的输出没有初始化器时是静默的。
fn init_logging() {
    struct Stderr;
    impl hbb_common::log::Log for Stderr {
        fn enabled(&self, _: &hbb_common::log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &hbb_common::log::Record) {
            if record.level() <= hbb_common::log::Level::Debug {
                eprintln!("[{}] {}", record.level(), record.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Stderr = Stderr;
    let _ = hbb_common::log::set_logger(&LOGGER);
    let level = match std::env::var("LANDESK_LOG").unwrap_or_else(|_| "info".into()).as_str() {
        "debug" => hbb_common::log::LevelFilter::Debug,
        "trace" => hbb_common::log::LevelFilter::Trace,
        _ => hbb_common::log::LevelFilter::Info,
    };
    hbb_common::log::set_max_level(level);
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            dump(&self.log);
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("landesk-session-{tag}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn start_service(port: u16, pub_key: &str, log: PathBuf) -> Service {
    let bin = env!("CARGO_BIN_EXE_landesk-server");
    let out = std::fs::File::create(&log).expect("service log");
    let err = out.try_clone().expect("clone log handle");
    let child = Command::new(bin)
        .args([
            "--server",
            "--landesk-listen",
            &port.to_string(),
            "--landesk-auth-key",
            pub_key,
        ])
        .env("RUST_LOG", "debug")
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .expect("spawn service");
    let service = Service { child, port, log };
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return service;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("service did not listen on port {port} within 30s");
}

/// 一个进程里同一时刻只能跑一个 rustdesk `--server`（IPC 管道名全局唯一），测试串行。
fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn unique_port() -> u16 {
    static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    31300 + (NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 400)
}

fn dump(path: &std::path::Path) {
    if let Ok(bytes) = std::fs::read(path) {
        eprintln!("--- {} ---\n{}", path.display(), String::from_utf8_lossy(&bytes));
    }
}

#[test]
fn a_landesk_session_receives_a_frame() {
    let _guard = exclusive();
    let _ = hbb_common::sodiumoxide::init();
    init_logging();
    let dir = temp_dir("frame");
    let key_path = dir.join("private.key");
    let book_path = dir.join("addressbook.json");
    let keypair = GlobalKeyPair::generate();
    keypair.save(&key_path).expect("save key");
    let pub_key = public_key_hex(&keypair.pk);

    // 端口按进程号偏移，避免与家目录里跑着的 Service 撞车。
    let port = unique_port();
    let mut service = start_service(port, &pub_key, dir.join("service.log"));
    let addr = format!("127.0.0.1:{}", service.port);

    let session = librustdesk::landesk::session::start(
        addr.clone(),
        book_path.clone(),
        key_path.clone(),
        true,
    )
    .expect("start session");

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut reported_pin = false;
    let mut first_frame_seq = 0;
    while Instant::now() < deadline {
        let seq = session.frame_seq();
        if seq > 0 && first_frame_seq == 0 {
            let frame = session.latest_frame().expect("frame");
            assert!(frame.width > 0 && frame.height > 0, "frame has no size");
            assert_eq!(
                frame.rgba.len(),
                frame.width * frame.height * 4,
                "frame is not RGBA of the announced size"
            );
            let book = librustdesk::addressbook::AddressBook::load(&book_path).expect("book");
            assert!(
                book.pins().get(&addr).is_some(),
                "the host fingerprint was not pinned"
            );
            first_frame_seq = seq;
            // 输入路径：发一次鼠标移动与点击，不应该让会话出错。
            session.send_mouse(0, 10, 10, false, false, false, false);
            session.send_mouse(1, 10, 10, false, false, false, false);
            session.send_mouse(2, 10, 10, false, false, false, false);
        } else if first_frame_seq > 0 && seq > first_frame_seq {
            // 画面在持续更新，不是只有首帧。
            assert!(!session.is_closed(), "session closed while streaming");
            let expected = book_path.clone();
            drop(expected);
            // 对端被杀掉之后，会话应当自行结束。
            service.kill();
            let close_deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < close_deadline {
                if session.is_closed() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            assert!(
                session.is_closed(),
                "the session did not notice that the service went away"
            );
            return;
        }
        if !reported_pin {
            let book = librustdesk::addressbook::AddressBook::load(&book_path).expect("book");
            if book.pins().get(&addr).is_some() {
                eprintln!("身份已通过（指纹已钉住），但还没有画面");
                reported_pin = true;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(first_frame_seq > 0, "no frame arrived within 60s");
    session.close();
    dump(&service.log);
}

#[test]
fn the_peer_accepts_the_file_clipboard_option() {
    let _guard = exclusive();
    let _ = hbb_common::sodiumoxide::init();
    init_logging();
    let dir = temp_dir("clipboard-file");
    let key_path = dir.join("private.key");
    let book_path = dir.join("addressbook.json");
    let keypair = GlobalKeyPair::generate();
    keypair.save(&key_path).expect("save key");
    let pub_key = public_key_hex(&keypair.pk);
    let log = dir.join("service.log");

    let port = unique_port();
    let service = start_service(port, &pub_key, log.clone());
    let addr = format!("127.0.0.1:{}", service.port);
    let session = librustdesk::landesk::session::start(
        addr.clone(),
        book_path,
        key_path,
        true,
    )
    .expect("start session");

    // 等会话真的立起来（首帧）再切开关。
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && session.frame_seq() == 0 {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(session.frame_seq() > 0, "no frame, session never started");

    session.set_file_clipboard(true);
    assert!(
        session.file_clipboard_enabled(),
        "the local toggle did not flip"
    );

    // 对端收到这个选项后会在日志里留下 `enable_file_transfer: Yes`。
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = false;
    let mut last_read_error = None;
    while Instant::now() < deadline {
        match std::fs::read(&log) {
            Ok(bytes) => {
                // 服务进程的控制台输出里混着非 UTF-8 字节（中文 Windows 的 GBK 提示），不能直接当字符串读。
                if String::from_utf8_lossy(&bytes).contains("enable_file_transfer: Yes") {
                    seen = true;
                    break;
                }
            }
            Err(err) => last_read_error = Some(err.to_string()),
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    if !seen {
        eprintln!("读取日志的最后一次错误：{last_read_error:?}");
    }
    session.close();
    assert!(
        seen,
        "the peer never acknowledged the file clipboard option"
    );
}

/// 文件会话：列举、上传、改名、新建目录、删除、下载。
#[test]
fn the_file_session_lists_and_manages_files() {
    let _guard = exclusive();
    let _ = hbb_common::sodiumoxide::init();
    init_logging();
    let dir = temp_dir("file-session");
    let key_path = dir.join("private.key");
    let book_path = dir.join("addressbook.json");
    let keypair = GlobalKeyPair::generate();
    keypair.save(&key_path).expect("save key");
    let pub_key = public_key_hex(&keypair.pk);

    let remote_dir = dir.join("remote");
    std::fs::create_dir_all(&remote_dir).expect("mkdir remote");
    std::fs::write(remote_dir.join("hello.txt"), b"remote file\n").expect("write remote file");
    let local_dir = dir.join("local");
    std::fs::create_dir_all(&local_dir).expect("mkdir local");
    let local_file = local_dir.join("upload.txt");
    std::fs::write(&local_file, b"uploaded\n").expect("write local file");
    let remote_dir_s = remote_dir.to_string_lossy().to_string();
    let local_dir_s = local_dir.to_string_lossy().to_string();

    let port = unique_port();
    let service = start_service(port, &pub_key, dir.join("service.log"));
    let addr = format!("127.0.0.1:{}", service.port);
    let file = librustdesk::landesk::session::start_file_transfer(
        addr.clone(),
        book_path,
        key_path,
        true,
        remote_dir_s.clone(),
    )
    .expect("start file session");

    let wait_for = |what: &str, check: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("timed out waiting for {what}");
    };

    // 1) 列目录
    let mut names: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        file.list(&remote_dir_s);
        std::thread::sleep(Duration::from_millis(200));
        if let Some(listing) = file.take_listing() {
            names = listing.entries.iter().map(|e| e.name.clone()).collect();
            if names.iter().any(|n| n == "hello.txt") {
                break;
            }
        }
    }
    assert!(
        names.iter().any(|n| n == "hello.txt"),
        "remote listing did not include hello.txt: {names:?}"
    );

    // 2) 上传：to 带文件名（上游约定）
    let uploaded = remote_dir.join("upload.txt");
    file.upload(
        &local_file.to_string_lossy(),
        &uploaded.to_string_lossy(),
    );
    wait_for("upload", &|| uploaded.exists());
    assert_eq!(std::fs::read(&uploaded).expect("read upload"), b"uploaded\n");

    // 3) 改名
    let renamed = remote_dir.join("upload-renamed.txt");
    file.rename(&uploaded.to_string_lossy(), "upload-renamed.txt");
    wait_for("rename", &|| renamed.exists() && !uploaded.exists());

    // 4) 删除
    file.delete(&renamed.to_string_lossy(), false);
    wait_for("delete", &|| !renamed.exists());

    // 5) 新建目录
    let new_dir = remote_dir.join("sub");
    file.create_dir(&new_dir.to_string_lossy());
    wait_for("create dir", &|| new_dir.exists());

    // 6) 下载：to 带文件名
    let downloaded = local_dir.join("hello.txt");
    file.download(
        &remote_dir.join("hello.txt").to_string_lossy(),
        &downloaded.to_string_lossy(),
    );
    wait_for("download", &|| downloaded.exists());

    file.close();
}

/// 扇出：同一批文件同时推给两台 Service，两台都要落地。
#[test]
fn the_same_files_reach_two_services() {
    let _guard = exclusive();
    let _ = hbb_common::sodiumoxide::init();
    init_logging();
    let dir = temp_dir("fanout");
    let key_path = dir.join("private.key");
    let book_path = dir.join("addressbook.json");
    let keypair = GlobalKeyPair::generate();
    keypair.save(&key_path).expect("save key");
    let pub_key = public_key_hex(&keypair.pk);

    // 两台 Service 共用一把公钥，各自一个远端目录。
    let mut services = Vec::new();
    let mut peers = Vec::new();
    let mut remote_dirs = Vec::new();
    for index in 0..2 {
        let remote = dir.join(format!("remote-{index}"));
        std::fs::create_dir_all(&remote).expect("mkdir remote");
        let port = unique_port();
        services.push(start_service(port, &pub_key, dir.join(format!("service-{index}.log"))));
        peers.push(format!("127.0.0.1:{}", port));
        remote_dirs.push(remote);
    }

    // 同一批文件（两个）。
    let mut files = Vec::new();
    for name in ["one.txt", "two.txt"] {
        let path = dir.join(name);
        std::fs::write(&path, format!("content of {name}\n")).expect("write source");
        files.push(path.to_string_lossy().to_string());
    }

    // 两台的目标目录不同，所以分两次分发（同一批文件、各自的目标目录）。
    for (peer, remote) in peers.iter().zip(remote_dirs.iter()) {
        let distribution = librustdesk::landesk::session::distribute(
            vec![peer.clone()],
            book_path.clone(),
            key_path.clone(),
            files.clone(),
            remote.to_string_lossy().to_string(),
        );
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline && !distribution.finished() {
            std::thread::sleep(Duration::from_millis(200));
        }
        let report = distribution.snapshot().remove(0);
        assert_eq!(
            report.failed, 0,
            "{} 失败：{:?}",
            report.peer, report.last_error
        );
        assert_eq!(report.done, files.len(), "{} 完成数不对", report.peer);
    }

    // 单次调用推多台（共用同一个目标目录）——这才是扇出的真实形状。
    let shared = dir.join("shared-incoming");
    std::fs::create_dir_all(&shared).expect("mkdir shared");
    let distribution = librustdesk::landesk::session::distribute(
        peers.clone(),
        book_path.clone(),
        key_path.clone(),
        files.clone(),
        shared.to_string_lossy().to_string(),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline && !distribution.finished() {
        std::thread::sleep(Duration::from_millis(200));
    }
    for report in distribution.snapshot() {
        assert_eq!(report.failed, 0, "{} 失败：{:?}", report.peer, report.last_error);
    }

    for remote in &remote_dirs {
        for name in ["one.txt", "two.txt"] {
            let landed = remote.join(name);
            assert!(landed.exists(), "{} 没落地", landed.display());
            assert_eq!(
                std::fs::read(&landed).expect("read"),
                format!("content of {name}\n").as_bytes()
            );
        }
    }
}
