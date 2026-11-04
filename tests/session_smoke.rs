//! 会话冒烟：在本机起一个 Service，把同一份二进制当 Master 连上去，断言真的收到了画面帧。
//!
//! 这是"能不能远程看到画面"的可重复证据——不需要人手点界面。
//! 需要先构建二进制；测试会用 `CARGO_BIN_EXE_rustdesk` 找到它。

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

/// 测试进程里把日志打到 stderr——库里 `log::` 的输出没有初始化器时是静默的。
fn init_logging() {
    struct Stderr;
    impl hbb_common::log::Log for Stderr {
        fn enabled(&self, _: &hbb_common::log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &hbb_common::log::Record) {
            if record.level() <= hbb_common::log::Level::Info {
                eprintln!("[{}] {}", record.level(), record.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: Stderr = Stderr;
    let _ = hbb_common::log::set_logger(&LOGGER);
    hbb_common::log::set_max_level(hbb_common::log::LevelFilter::Info);
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
    let bin = env!("CARGO_BIN_EXE_rustdesk");
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

fn dump(path: &std::path::Path) {
    if let Ok(text) = std::fs::read_to_string(path) {
        eprintln!("--- {} ---\n{}", path.display(), text);
    }
}

#[test]
fn a_landesk_session_receives_a_frame() {
    let _ = hbb_common::sodiumoxide::init();
    init_logging();
    let dir = temp_dir("frame");
    let key_path = dir.join("private.key");
    let book_path = dir.join("addressbook.json");
    let keypair = GlobalKeyPair::generate();
    keypair.save(&key_path).expect("save key");
    let pub_key = public_key_hex(&keypair.pk);

    // 端口按进程号偏移，避免与家目录里跑着的 Service 撞车。
    let port = 31300 + (std::process::id() % 200) as u16;
    let service = start_service(port, &pub_key, dir.join("service.log"));
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
    while Instant::now() < deadline {
        if session.frame_seq() > 0 {
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
            session.close();
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
    session.close();
    dump(&service.log);
    panic!("no frame arrived within 60s");
}
