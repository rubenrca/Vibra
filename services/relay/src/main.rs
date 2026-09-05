//! Ephemeral, bounded, opaque websocket bridge. No terminal payload logging/storage.
use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore, mpsc};
use vibra_remote::{Hello, WIRE_LIMIT};
type Rooms = Arc<Mutex<HashMap<String, Room>>>;
struct Room {
    token: String,
    owner: String,
    active: bool,
    touched: Instant,
    join: Option<mpsc::Sender<WebSocket>>,
}
#[tokio::main]
async fn main() {
    let address = std::env::var("VIBRA_RELAY_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let app = Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(serde_json::json!({"service":"vibra-relay","protocol":1})) }),
        )
        .route("/ws", get(upgrade))
        .with_state(Rooms::default());
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("relay bind");
    eprintln!("Vibra relay listening on {address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("relay serve");
}
async fn upgrade(State(rooms): State<Rooms>, ws: WebSocketUpgrade) -> impl IntoResponse {
    static CONNECTIONS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let Ok(permit) = CONNECTIONS
        .get_or_init(|| Arc::new(Semaphore::new(512)))
        .clone()
        .try_acquire_owned()
    else {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(WIRE_LIMIT)
        .max_frame_size(WIRE_LIMIT)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            route(socket, rooms).await
        })
        .into_response()
}
async fn route(mut socket: WebSocket, rooms: Rooms) {
    let Ok(Some(Ok(Message::Text(text)))) =
        tokio::time::timeout(Duration::from_secs(5), socket.recv()).await
    else {
        return;
    };
    if text.len() > 1024 {
        return;
    }
    let Ok(hello) = serde_json::from_str::<Hello>(&text) else {
        return;
    };
    if !hello.valid() {
        return;
    }
    if hello.role == "phone" {
        let sender = {
            let mut rooms = rooms.lock().await;
            let Some(room) = rooms.get_mut(&hello.channel) else {
                return;
            };
            if room.token != hello.token {
                return;
            }
            room.join.take()
        };
        if let Some(sender) = sender {
            let _ = sender.send(socket).await;
        }
        return;
    }
    let (tx, mut rx) = mpsc::channel(1);
    {
        let mut rooms = rooms.lock().await;
        rooms.retain(|_, room| room.active || room.touched.elapsed() < Duration::from_secs(3600));
        if let Some(room) = rooms.get(&hello.channel) {
            if room.active
                || room.owner != hello.token
                || Some(&room.token) != hello.peer_token.as_ref()
            {
                return;
            }
        } else if rooms.len() >= 1024 {
            return;
        }
        rooms.insert(
            hello.channel.clone(),
            Room {
                token: hello.peer_token.unwrap(),
                owner: hello.token,
                active: true,
                touched: Instant::now(),
                join: Some(tx),
            },
        );
    }
    if socket.send(Message::Text("ready".into())).await.is_ok() {
        // Keep registration briefly after disconnect, so only the owner can reclaim it.
        let peer = loop {
            tokio::select! {
                peer = rx.recv() => break peer,
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Ping(data))) => { if socket.send(Message::Pong(data)).await.is_err() { break None } },
                    Some(Ok(Message::Pong(_))) => {},
                    _ => break None,
                },
                _ = tokio::time::sleep(Duration::from_secs(40)) => break None,
            }
        };
        if let Some(mut phone) = peer
            && socket.send(Message::Text("peer".into())).await.is_ok()
            && phone.send(Message::Text("peer".into())).await.is_ok()
        {
            bridge(socket, phone).await;
        }
    }
    if let Some(room) = rooms.lock().await.get_mut(&hello.channel) {
        room.active = false;
        room.join = None;
        room.touched = Instant::now();
    }
}
async fn bridge(host: WebSocket, phone: WebSocket) {
    let (mut hw, mut hr) = host.split();
    let (mut pw, mut pr) = phone.split();
    let forward = async {
        while let Some(Ok(message)) = hr.next().await {
            match message {
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                _ => break,
            }
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), pw.send(message)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
    };
    let reverse = async {
        while let Some(Ok(message)) = pr.next().await {
            match message {
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                _ => break,
            }
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), hw.send(message)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
    };
    tokio::select! { _ = forward => {}, _ = reverse => {}, _ = tokio::time::sleep(Duration::from_secs(24*60*60)) => {} }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibra_remote::{self as wire, tungstenite::Message as Ws};
    #[tokio::test]
    async fn credentials_gate_phone_and_host_reconnects() {
        let rooms = Rooms::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/ws", get(upgrade))
            .with_state(rooms.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let url = format!("ws://{address}/ws");
        let channel = wire::secret().unwrap();
        let owner = wire::secret().unwrap();
        let phone = wire::secret().unwrap();
        let host_hello = Hello {
            role: "host".into(),
            channel: channel.clone(),
            token: owner.clone(),
            peer_token: Some(phone.clone()),
        };
        let (mut host, _) = wire::connect_async(&url).await.unwrap();
        host.send(Ws::Text(serde_json::to_string(&host_hello).unwrap().into()))
            .await
            .unwrap();
        assert!(matches!(host.next().await,Some(Ok(Ws::Text(t))) if t=="ready"));
        let (mut wrong, _) = wire::connect_async(&url).await.unwrap();
        wrong
            .send(Ws::Text(
                serde_json::to_string(&Hello {
                    role: "phone".into(),
                    channel: channel.clone(),
                    token: wire::secret().unwrap(),
                    peer_token: None,
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        assert!(!matches!(wrong.next().await, Some(Ok(Ws::Text(_)))));
        assert!(rooms.lock().await[&channel].join.is_some());
        host.close(None).await.unwrap();
        for _ in 0..100 {
            if !rooms.lock().await[&channel].active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let (mut impostor, _) = wire::connect_async(&url).await.unwrap();
        let mut bad = host_hello.clone();
        bad.token = wire::secret().unwrap();
        impostor
            .send(Ws::Text(serde_json::to_string(&bad).unwrap().into()))
            .await
            .unwrap();
        assert!(!matches!(impostor.next().await, Some(Ok(Ws::Text(_)))));
        let (mut host, _) = wire::connect_async(&url).await.unwrap();
        host.send(Ws::Text(serde_json::to_string(&host_hello).unwrap().into()))
            .await
            .unwrap();
        assert!(matches!(host.next().await,Some(Ok(Ws::Text(t))) if t=="ready"));
        host.close(None).await.unwrap();
        server.abort();
    }
}
