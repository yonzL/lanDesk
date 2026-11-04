//! landesk 的直连监听：在固定端口上接受连接，交给会话层。
//!
//! 与上游 rendezvous 打洞的区别只在于"连接从哪来"：这里是一个常驻监听，不需要任何协调服务。
//! 握手与鉴权沿用会话层既有路径，见 `architecture.md`。

use hbb_common::{log, tcp, tokio, ResultType, Stream};

pub async fn start(port: u16) -> ResultType<()> {
    let listener = tcp::new_listener(("0.0.0.0", port), true).await?;
    log::info!("landesk: service listening on {}", listener.local_addr()?);
    let server = crate::server::new();
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                log::warn!("landesk: accept failed on port {port}: {err}");
                continue;
            }
        };
        stream.set_nodelay(true).ok();
        let Ok(local) = stream.local_addr() else {
            continue;
        };
        let server = server.clone();
        tokio::spawn(async move {
            if let Err(err) = crate::server::create_tcp_connection(
                server,
                Stream::from(stream, local),
                addr,
                // secure: 与上游直连连接一致，先做密钥交换再谈会话。
                true,
                Default::default(),
            )
            .await
            {
                log::warn!("landesk: connection from {addr} ended: {err}");
            }
        });
    }
}
