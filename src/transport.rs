//! landesk 的直连传输：Service 侧监听、Master 侧拨号。
//!
//! 与上游 rendezvous 打洞的区别只在于"连接从哪来"：这里是常驻监听与直接拨号，不需要任何协调
//! 服务。握手与鉴权沿用会话层既有路径，见 `architecture.md`。

use std::path::Path;

use base::message_proto::{message, Message, PublicKey};
use hbb_common::keyauth::{self, PinOutcome};
use hbb_common::rendezvous_proto::IdPk;
use hbb_common::tcp::KxTranscript;
use hbb_common::{bail, log, tcp, tokio, ResultType, Stream};
use hbb_common::protobuf::Message as _;

const DIAL_TIMEOUT_MS: u64 = 5_000;

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

/// 一次已经完成密钥交换与双向证明的连接。
///
/// `host_pk` 是对面主机签名公钥（已验签，并已按指纹库判定），给上层用作会话的身份记录。
pub struct Authenticated {
    pub stream: tcp::FramedStream,
    pub host_pk: [u8; 32],
    pub outcome: PinOutcome,
}

/// 拨号并完成身份：密钥交换 → 主机身份（TOFU 指纹）→ 用全局私钥回证明。
///
/// 密钥交换本身不验签：没有 rendezvous 可问主机公钥，主机身份在加密通道里由 `AuthNonce`
/// 携带并当场证明，再由地址簿里的指纹判定是否可信。
pub async fn connect_authenticated(
    addr: &str,
    book_path: &Path,
    key_path: &Path,
    control: bool,
) -> ResultType<Authenticated> {
    let keys = keyauth::GlobalKeyPair::load(key_path)?;
    let mut book = crate::addressbook::AddressBook::load(book_path)?;

    let mut conn = tcp::FramedStream::new(addr, None, DIAL_TIMEOUT_MS).await?;
    let bytes = match conn.next_timeout(DIAL_TIMEOUT_MS).await {
        Some(Ok(bytes)) => bytes,
        _ => bail!("no identity handshake from {addr}"),
    };
    let identity = Message::parse_from_bytes(&bytes)?;
    let Some(message::Union::SignedId(signed_id)) = identity.union else {
        bail!("{addr} did not start with a signed identity");
    };
    // sign::sign 的输出是 64 字节签名 + 消息本身；这里先取消息，验签留给通道内的主机证明。
    if signed_id.id.len() <= 64 {
        bail!("{addr} sent a signed identity that is too short");
    }
    let id_pk = IdPk::parse_from_bytes(&signed_id.id[64..])?;
    let Some(their_pk_b) = to_array_32(&id_pk.pk) else {
        bail!("{addr} sent a host public key of the wrong length");
    };

    let (asymmetric_value, symmetric_value, key) = crate::common::create_symmetric_key_msg(their_pk_b);
    let picked = hbb_common::tcp::kx_version_for(id_pk.kx_version);
    let mut out = Message::new();
    out.set_public_key(PublicKey {
        asymmetric_value: asymmetric_value.clone(),
        symmetric_value,
        kx_version: picked,
        ..Default::default()
    });
    conn.send(&out).await?;
    if picked == 0 {
        conn.set_key(key);
    } else {
        conn.set_key_split(
            key,
            true,
            &KxTranscript {
                initiator_pk: &asymmetric_value,
                responder_pk: &their_pk_b,
                advertised: id_pk.kx_version,
                picked,
            },
        )?;
    }

    let transcript = keyauth::transcript_digest(
        &asymmetric_value,
        &their_pk_b,
        id_pk.kx_version,
        picked,
    );
    let outcome = keyauth::answer_challenge(&mut conn, &keys, &transcript, book.pins_mut(), addr, control)
        .await
        .map_err(|reason| hbb_common::anyhow::anyhow!(keyauth::reject_log_line(reason, addr)))?;
    book.save(book_path)?;
    log::info!("landesk: authenticated {addr} ({outcome:?})");
    Ok(Authenticated {
        stream: conn,
        host_pk: their_pk_b,
        outcome,
    })
}

/// 拨号、证明身份，然后立即断开——冒烟与自检用的形态。
pub async fn dial(
    addr: &str,
    book_path: &Path,
    key_path: &Path,
    control: bool,
) -> ResultType<PinOutcome> {
    let authenticated = connect_authenticated(addr, book_path, key_path, control).await?;
    log::info!("landesk: dial done for {addr} ({:?})", authenticated.outcome);
    Ok(authenticated.outcome)
}

fn to_array_32(bytes: &[u8]) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if bytes.len() != out.len() {
        return None;
    }
    out.copy_from_slice(bytes);
    Some(out)
}
