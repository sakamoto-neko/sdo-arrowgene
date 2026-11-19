//! Browser connections can only reach the lobby owned by this server.
use anyhow::{Result, ensure};
use axum::{
    Router,
    body::Bytes,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::header,
    response::Redirect,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use sdo_net::{
    Command,
    codec::{MAX_FRAME, Packet},
};

pub fn routes(mut game: SocketAddr, local_assets: bool) -> Result<Router> {
    if game.ip().is_unspecified() {
        game.set_ip(if game.is_ipv4() {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        });
    }
    let mut router = Router::new().route(
        "/play/ws",
        get(move |upgrade: WebSocketUpgrade| async move {
            upgrade
                .max_message_size(MAX_FRAME)
                .max_frame_size(MAX_FRAME)
                .on_upgrade(move |ws| async move {
                    let _ = relay(ws, game).await;
                })
        }),
    );
    router = router
        .route("/play", get(|| async { Redirect::temporary("play/") }))
        .route("/", get(|| async { Redirect::temporary("play/") }));
    router = router.route(
        "/play/config.json",
        get(move || async move {
            (
                [(header::CONTENT_TYPE, "application/json")],
                if local_assets {
                    r#"{"assets":true}"#
                } else {
                    r#"{"assets":false}"#
                },
            )
        }),
    );
    macro_rules! file {
        ($route:literal, $file:literal, $mime:literal) => {
            router = router.route(
                $route,
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, $mime)],
                        Bytes::from_static(include_bytes!(concat!("../../../../web/", $file))),
                    )
                }),
            );
        };
    }
    file!("/play/", "index.html", "text/html; charset=utf-8");
    file!("/play/client.mjs", "client.mjs", "text/javascript");
    file!("/play/decode.mjs", "decode.mjs", "text/javascript");
    file!(
        "/play/pkg/sdo_arrowgene.js",
        "pkg/sdo_arrowgene.js",
        "text/javascript"
    );
    file!(
        "/play/pkg/sdo_arrowgene_bg.wasm",
        "pkg/sdo_arrowgene_bg.wasm",
        "application/wasm"
    );
    file!("/play/player.css", "player.css", "text/css");
    file!("/play/pkg/sdo_web.js", "pkg/sdo_web.js", "text/javascript");
    file!(
        "/play/pkg/sdo_web_bg.wasm",
        "pkg/sdo_web_bg.wasm",
        "application/wasm"
    );
    Ok(router)
}

// One WebSocket message corresponds to one native length-prefixed packet.
async fn relay(ws: WebSocket, game: SocketAddr) -> Result<()> {
    let tcp = tokio::net::TcpStream::connect(game).await?;
    tcp.set_nodelay(true)?;
    let (mut read, mut write) = tcp.into_split();
    let (mut outgoing, mut incoming) = ws.split();
    let to_game = async {
        while let Some(message) = incoming.next().await {
            match message? {
                Message::Binary(bytes) => {
                    ensure!(
                        !matches!(Command::decode(&bytes)?, Command::Create { .. }),
                        "Browser players may join existing rooms"
                    );
                    write.write_u32(bytes.len() as u32).await?;
                    write.write_all(&bytes).await?;
                }
                Message::Close(_) => break,
                Message::Text(_) => anyhow::bail!("Expected a binary game packet"),
                _ => {}
            }
        }
        Ok(())
    };
    let from_game = async {
        loop {
            let size = read.read_u32().await? as usize;
            ensure!(size <= MAX_FRAME, "Game packet exceeds limit");
            let mut bytes = vec![0; size];
            read.read_exact(&mut bytes).await?;
            outgoing.send(Message::Binary(bytes.into())).await?;
        }
    };
    let result = tokio::select! { result = to_game => result, result = from_game => result };
    let _ = outgoing.close().await;
    result
}
