//! Direct local transport and Noise authentication. Never log these messages.
use anyhow::{Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
pub use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
pub use tokio;
pub use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};
pub use vibra_remote_protocol as protocol;
/// Bound allocation before a peer can send an oversized websocket message.
pub async fn connect_async(
    url: &str,
) -> Result<
    (
        WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        tungstenite::handshake::client::Response,
    ),
    tungstenite::Error,
> {
    let config = tungstenite::protocol::WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(Some(WIRE_LIMIT))
        .max_frame_size(Some(WIRE_LIMIT));
    tokio_tungstenite::connect_async_with_config(url, Some(config), false).await
}
pub const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
pub const PROLOGUE: &[u8] = b"Vibra remote v1";
pub const CHUNK: usize = 60_000;
pub const WIRE_LIMIT: usize = 65_535;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    pub version: u16,
    pub endpoint: String,
    pub public_key: String,
    pub invitation: String,
    pub expires: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Introduction {
    pub invitation: String,
    pub name: String,
}
pub fn base64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}
pub fn unbase64(value: &str) -> Result<Vec<u8>> {
    Ok(B64.decode(value)?)
}
pub fn keypair() -> Result<snow::Keypair> {
    Ok(snow::Builder::new(PATTERN.parse()?).generate_keypair()?)
}
pub fn secret() -> Result<String> {
    Ok(base64(&keypair()?.private))
}
pub fn handshake(private: &[u8], remote: Option<&[u8]>) -> Result<snow::HandshakeState> {
    let builder = snow::Builder::new(PATTERN.parse()?)
        .local_private_key(private)?
        .prologue(PROLOGUE)?;
    Ok(match remote {
        Some(key) => builder.remote_public_key(key)?.build_initiator()?,
        None => builder.build_responder()?,
    })
}
/// One websocket binary message per Noise record. Encrypted fragment header:
/// 0 = more, 1 = final. Ordered AEAD nonces prevent substitution/replay.
pub struct Channel {
    state: snow::TransportState,
    partial: Vec<u8>,
}
impl Channel {
    pub fn new(state: snow::TransportState) -> Self {
        Self {
            state,
            partial: Vec::new(),
        }
    }
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>> {
        ensure!(
            !message.is_empty() && message.len() <= protocol::MAX_MESSAGE_BYTES,
            "invalid message size"
        );
        let count = message.len().div_ceil(CHUNK);
        message
            .chunks(CHUNK)
            .enumerate()
            .map(|(i, bytes)| {
                let mut plain = Vec::with_capacity(bytes.len() + 1);
                plain.push(u8::from(i + 1 == count));
                plain.extend_from_slice(bytes);
                let mut cipher = vec![0; plain.len() + 16];
                let n = self.state.write_message(&plain, &mut cipher)?;
                cipher.truncate(n);
                Ok(cipher)
            })
            .collect()
    }
    pub fn open(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>> {
        ensure!(record.len() <= WIRE_LIMIT, "record too large");
        let mut plain = vec![0; record.len()];
        let n = self.state.read_message(record, &mut plain)?;
        ensure!(n > 1 && n <= CHUNK + 1 && plain[0] <= 1, "invalid fragment");
        if self.partial.len() + n - 1 > protocol::MAX_MESSAGE_BYTES {
            self.partial.clear();
            bail!("message too large")
        }
        self.partial.extend_from_slice(&plain[1..n]);
        Ok((plain[0] == 1).then(|| std::mem::take(&mut self.partial)))
    }
}
/// Only local network destinations are accepted for the direct transport.
pub fn is_local_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
    }
}
pub fn validate_local_endpoint(value: &str) -> Result<()> {
    let url = url::Url::parse(value)?;
    let host = url.host_str().unwrap_or_default();
    let bonjour = host.strip_suffix(".local").is_some_and(|name| {
        !name.is_empty()
            && name.len() <= 63
            && !name.starts_with('-')
            && !name.ends_with('-')
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    let local = host == "localhost"
        || bonjour
        || host
            .trim_matches(['[', ']'])
            .parse()
            .is_ok_and(is_local_address);
    ensure!(
        url.scheme() == "ws"
            && local
            && url.port().is_some_and(|p| p >= 1024)
            && url.path() == "/local"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid local endpoint"
    );
    Ok(())
}
/// WebSocket is just framing; authentication and encryption happen in Noise IK.
/// Routing credentials and terminal content are never sent as plaintext.
#[allow(clippy::result_large_err)] // Tungstenite requires an HTTP ErrorResponse in its callback.
pub async fn accept_local(
    stream: tokio::net::TcpStream,
) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, tungstenite::Error> {
    use tungstenite::handshake::server::{Request, Response};
    let config = tungstenite::protocol::WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(Some(WIRE_LIMIT))
        .max_frame_size(Some(WIRE_LIMIT));
    tokio_tungstenite::accept_hdr_async_with_config(
        MaybeTlsStream::Plain(stream),
        |request: &Request, response: Response| {
            if request.uri().path() != "/local" || request.uri().query().is_some() {
                return Err(Response::builder().status(404).body(None).unwrap());
            }
            Ok(response)
        },
        Some(config),
    )
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_endpoints_reject_public_hosts_and_legacy_routing() {
        for endpoint in [
            "ws://my-mac.local:8788/local",
            "ws://192.168.1.2:8788/local",
            "ws://10.0.0.2:8788/local",
            "ws://172.16.0.2:8788/local",
            "ws://127.0.0.1:8788/local",
        ] {
            assert!(validate_local_endpoint(endpoint).is_ok(), "{endpoint}");
        }
        for endpoint in [
            "wss://relay.example/ws",
            "ws://relay.example:8788/local",
            "ws://8.8.8.8:8788/local",
            "ws://172.32.0.2:8788/local",
            "ws://mac.local.evil.com:8788/local",
            "ws://mac.local:8788/ws",
            "ws://mac.local:8788/local?token=x",
            "ws://user@mac.local:8788/local",
            "ws://mac.local/local",
            "ws://mac.local:80/local",
        ] {
            assert!(validate_local_endpoint(endpoint).is_err(), "{endpoint}");
        }
    }
    #[tokio::test]
    async fn local_upgrade_rejects_other_paths() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let host = async { accept_local(listener.accept().await.unwrap().0).await };
        let url = format!("ws://{address}/ws");
        let phone = connect_async(&url);
        let (host, phone) = tokio::join!(host, phone);
        assert!(host.is_err());
        assert!(phone.is_err());
    }
    #[test]
    fn encrypted_fragments_replay_tamper_and_identity() {
        let a = keypair().unwrap();
        let b = keypair().unwrap();
        let mut i = handshake(&a.private, Some(&b.public)).unwrap();
        let mut r = handshake(&b.private, None).unwrap();
        let mut buf = vec![0; WIRE_LIMIT];
        let mut out = vec![0; WIRE_LIMIT];
        let n = i.write_message(b"invitation", &mut buf).unwrap();
        assert_eq!(r.read_message(&buf[..n], &mut out).unwrap(), 10);
        assert_eq!(r.get_remote_static().unwrap(), a.public);
        let n = r.write_message(b"approved", &mut buf).unwrap();
        i.read_message(&buf[..n], &mut out).unwrap();
        let mut i = Channel::new(i.into_transport_mode().unwrap());
        let mut r = Channel::new(r.into_transport_mode().unwrap());
        let message = vec![42; 200_000];
        let records = i.seal(&message).unwrap();
        for record in &records[..records.len() - 1] {
            assert!(r.open(record).unwrap().is_none());
        }
        assert_eq!(r.open(records.last().unwrap()).unwrap().unwrap(), message);
        assert!(r.open(&records[0]).is_err());
        let mut corrupt = i.seal(b"secret").unwrap().remove(0);
        corrupt[0] ^= 1;
        assert!(r.open(&corrupt).is_err());
    }
}
