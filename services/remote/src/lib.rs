//! Relay routing is separate from Noise authentication. Never log these messages.
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
pub struct Hello {
    pub role: String,
    pub channel: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_token: Option<String>,
}
impl Hello {
    pub fn valid(&self) -> bool {
        matches!(self.role.as_str(), "host" | "phone")
            && [&self.channel, &self.token]
                .iter()
                .all(|s| s.len() == 44 && unbase64(s).is_ok_and(|x| x.len() == 32))
            && match self.role.as_str() {
                "host" => self
                    .peer_token
                    .as_ref()
                    .is_some_and(|x| x.len() == 44 && unbase64(x).is_ok_and(|b| b.len() == 32)),
                _ => self.peer_token.is_none(),
            }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    pub version: u16,
    pub relay: String,
    pub channel: String,
    pub token: String,
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
pub fn validate_relay(value: &str) -> Result<()> {
    let url = url::Url::parse(value)?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "relay URL must not contain credentials/query/fragment"
    );
    ensure!(url.path() == "/ws", "relay path must be /ws");
    let local = url
        .host_str()
        .is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]");
    ensure!(
        url.scheme() == "wss" || (url.scheme() == "ws" && local),
        "use wss://; ws:// is only allowed on loopback"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
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
