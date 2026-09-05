//! Explicit sharing and one remote controller. Network work never runs on GPUI.
use crate::ports::{
    keyboard::{self, TerminalKeyEventType, TerminalKeystroke, TerminalModifiers},
    terminal::{RemoteFrame, TerminalHandle, TerminalSize},
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use vibra_remote::{
    self as wire, SinkExt, StreamExt,
    protocol::{Envelope, ErrorCode, Input, Key, Message, Modifier, Pane, ReleaseReason, Size},
    tokio,
    tungstenite::Message as Ws,
};
const KEYCHAIN_SERVICE: &str = "app.vibra.remote.v1";
#[derive(Clone, Serialize, Deserialize)]
struct Credentials {
    private: String,
    public: String,
    channel: String,
    host_token: String,
    phone_token: String,
    paired: Option<String>,
    relay: String,
}
impl Credentials {
    fn fresh(relay: String) -> Result<Self> {
        let key = wire::keypair()?;
        Ok(Self {
            private: wire::base64(&key.private),
            public: wire::base64(&key.public),
            channel: wire::secret()?,
            host_token: wire::secret()?,
            phone_token: wire::secret()?,
            paired: None,
            relay,
        })
    }
    fn save(&self) -> Result<()> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            "mac",
            &serde_json::to_vec(self)?,
        )?;
        Ok(())
    }
}
struct Shared {
    title: Option<String>,
    handle: Weak<dyn TerminalHandle>,
    shared: bool,
}
struct Pending {
    key: String,
    name: String,
    approved: Option<bool>,
}
struct State {
    enabled: bool,
    generation: u64,
    credentials: Option<Credentials>,
    invitation: Option<(String, u64)>,
    panes: HashMap<Uuid, Shared>,
    pending: Option<Pending>,
    status: String,
}
impl State {
    fn begin_pairing(
        &mut self,
        generation: u64,
        phone: String,
        intro: wire::Introduction,
    ) -> Result<()> {
        ensure!(
            self.generation == generation
                && self
                    .credentials
                    .as_ref()
                    .is_some_and(|c| c.paired.is_none()),
            "pairing unavailable"
        );
        ensure!(
            self.invitation
                .as_ref()
                .is_some_and(|(token, expiry)| token == &intro.invitation && *expiry > now()),
            "invitation expired"
        );
        self.invitation = None; // Consume once, including rejected/abandoned attempts.
        self.pending = Some(Pending {
            key: phone.clone(),
            name: intro.name,
            approved: None,
        });
        self.status = "Confirma el iPhone en Ajustes".into();
        Ok(())
    }
}
pub struct Hub {
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct Status {
    pub enabled: bool,
    pub description: String,
    pub relay: String,
    pub invitation: Option<String>,
    pub pending: Option<String>,
    pub paired: bool,
}
pub fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        state: Mutex::new(State {
            enabled: false,
            generation: 0,
            credentials: None,
            invitation: None,
            panes: HashMap::new(),
            pending: None,
            status: "Control remoto desactivado".into(),
        }),
    })
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
impl Hub {
    pub fn register(&self, id: Uuid, handle: &Arc<dyn TerminalHandle>) {
        self.state.lock().unwrap().panes.insert(
            id,
            Shared {
                title: None,
                handle: Arc::downgrade(handle),
                shared: false,
            },
        );
    }
    pub fn title(&self, id: Uuid, title: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(p) = state.panes.get_mut(&id) {
            let mut title = title.to_owned();
            while title.len() > 512 {
                title.pop();
            }
            p.title = Some(title);
        }
    }
    pub fn shared(&self, id: Uuid) -> bool {
        self.state
            .lock()
            .unwrap()
            .panes
            .get(&id)
            .is_some_and(|p| p.shared)
    }
    pub fn toggle_share(&self, id: Uuid) {
        let mut state = self.state.lock().unwrap();
        if let Some(pane) = state.panes.get_mut(&id) {
            pane.shared = !pane.shared;
            if !pane.shared
                && let Some(h) = pane.handle.upgrade()
            {
                h.remote_release();
            }
        }
    }
    pub fn reclaim(&self, id: Uuid) {
        if let Some(h) = self
            .state
            .lock()
            .unwrap()
            .panes
            .get(&id)
            .and_then(|p| p.handle.upgrade())
        {
            h.remote_release();
        }
    }
    fn release_all(state: &State) {
        for pane in state.panes.values() {
            if let Some(h) = pane.handle.upgrade() {
                h.remote_release();
            }
        }
    }
    pub fn disable(&self) {
        let mut s = self.state.lock().unwrap();
        s.enabled = false;
        s.generation += 1;
        s.pending = None;
        s.invitation = None;
        for p in s.panes.values_mut() {
            p.shared = false;
        }
        Self::release_all(&s);
        s.status = "Control remoto desactivado".into();
    }
    pub fn enable(&'static self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.enabled {
            return Ok(());
        }
        if s.credentials.is_none() {
            s.credentials = Some(
                match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, "mac") {
                    Ok(bytes) => serde_json::from_slice::<Credentials>(&bytes)?,
                    Err(error) if error.code() == -25300 => {
                        Credentials::fresh("ws://127.0.0.1:8787/ws".into())?
                    }
                    Err(error) => return Err(error.into()),
                },
            );
        }
        let c = s.credentials.as_ref().unwrap().clone();
        wire::validate_relay(&c.relay)?;
        c.save()?;
        s.enabled = true;
        s.generation += 1;
        s.status = "Conectando al relay…".into();
        let generation = s.generation;
        drop(s);
        std::thread::Builder::new()
            .name("vibra-remote".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("remote runtime");
                rt.block_on(async move {
                    while self.current(generation) {
                        let _ = self.connect(generation, &c).await;
                        self.reset_connection(generation);
                        if self.current(generation) {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                });
            })?;
        Ok(())
    }
    pub fn configure(&'static self, relay: &str) -> Result<()> {
        wire::validate_relay(relay)?;
        self.disable();
        let c = Credentials::fresh(relay.into())?;
        c.save()?;
        self.state.lock().unwrap().credentials = Some(c);
        self.enable()
    }
    pub fn pair(&'static self) -> Result<()> {
        self.enable()?;
        // Rotate relay credentials too: revoked phones cannot occupy the new route.
        let relay = self
            .state
            .lock()
            .unwrap()
            .credentials
            .as_ref()
            .unwrap()
            .relay
            .clone();
        self.disable();
        let c = Credentials::fresh(relay)?;
        c.save()?;
        {
            let mut s = self.state.lock().unwrap();
            s.credentials = Some(c);
            s.invitation = Some((wire::secret()?, now() + 300));
        }
        self.enable()
    }
    pub fn revoke(&'static self) -> Result<()> {
        let relay = self.status().relay;
        self.disable();
        let c = Credentials::fresh(relay)?;
        c.save()?;
        self.state.lock().unwrap().credentials = Some(c);
        Ok(())
    }
    pub fn approve(&self, approved: bool) {
        if let Some(p) = self.state.lock().unwrap().pending.as_mut() {
            p.approved = Some(approved);
        }
    }
    pub fn status(&self) -> Status {
        let s = self.state.lock().unwrap();
        let invitation = s
            .invitation
            .as_ref()
            .filter(|(_, expiry)| *expiry > now())
            .and_then(|(token, expiry)| {
                let c = s.credentials.as_ref()?;
                serde_json::to_string(&wire::Invitation {
                    version: 1,
                    relay: c.relay.clone(),
                    channel: c.channel.clone(),
                    token: c.phone_token.clone(),
                    public_key: c.public.clone(),
                    invitation: token.clone(),
                    expires: *expiry,
                })
                .ok()
            });
        Status {
            enabled: s.enabled,
            description: s.status.clone(),
            relay: s
                .credentials
                .as_ref()
                .map(|c| c.relay.clone())
                .unwrap_or_else(|| "ws://127.0.0.1:8787/ws".into()),
            invitation,
            pending: s
                .pending
                .as_ref()
                .filter(|p| p.approved.is_none())
                .map(|p| p.name.clone()),
            paired: s.credentials.as_ref().is_some_and(|c| c.paired.is_some()),
        }
    }
    fn current(&self, generation: u64) -> bool {
        let s = self.state.lock().unwrap();
        s.enabled && s.generation == generation
    }
    fn describe(&self, generation: u64, text: &str) {
        let mut s = self.state.lock().unwrap();
        if s.generation == generation {
            s.status = text.into();
        }
    }
    fn reset_connection(&self, generation: u64) {
        let mut s = self.state.lock().unwrap();
        if s.generation != generation {
            return;
        }
        Self::release_all(&s);
        s.pending = None;
        s.status = "Sin iPhone conectado · esperando relay".into();
    }
    fn handle(&self, id: Uuid) -> Result<Arc<dyn TerminalHandle>> {
        let s = self.state.lock().unwrap();
        s.panes
            .get(&id)
            .filter(|p| p.shared)
            .and_then(|p| p.handle.upgrade())
            .ok_or_else(|| anyhow::anyhow!("not shared"))
    }
    fn panes(&self) -> Vec<Pane> {
        let mut s = self.state.lock().unwrap();
        s.panes.retain(|_, p| p.handle.strong_count() > 0);
        s.panes
            .iter()
            .filter(|(_, p)| p.shared)
            .filter_map(|(id, p)| {
                let h = p.handle.upgrade()?;
                let snap = h.remote_size();
                Some(Pane {
                    id: *id,
                    title: p
                        .title
                        .clone()
                        .or_else(|| h.foreground_process_name())
                        .unwrap_or_else(|| "Terminal".into()),
                    size: Size {
                        columns: snap.columns,
                        rows: snap.rows,
                    },
                })
            })
            .take(128)
            .collect()
    }
    async fn connect(&self, generation: u64, c: &Credentials) -> Result<()> {
        let (mut socket, _) =
            tokio::time::timeout(Duration::from_secs(10), wire::connect_async(&c.relay)).await??;
        socket
            .send(Ws::Text(
                serde_json::to_string(&wire::Hello {
                    role: "host".into(),
                    channel: c.channel.clone(),
                    token: c.host_token.clone(),
                    peer_token: Some(c.phone_token.clone()),
                })?
                .into(),
            ))
            .await?;
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            ensure!(self.current(generation), "disabled");
            tokio::select! {
                incoming=socket.next()=>match incoming { Some(Ok(Ws::Text(t))) if t=="peer" => break, Some(Ok(Ws::Text(t))) if t=="ready" => self.describe(generation,"Relay conectado · esperando iPhone"), Some(Ok(Ws::Pong(_)))=>{}, _=>bail!("relay disconnected") },
                _=tick.tick()=>socket.send(Ws::Ping(vec![].into())).await?,
            }
        }
        let private = wire::unbase64(&c.private)?;
        let mut handshake = wire::handshake(&private, None)?;
        let mut plain = vec![0; wire::WIRE_LIMIT];
        let record = receive_binary(&mut socket, Duration::from_secs(15)).await?;
        let n = handshake.read_message(&record, &mut plain)?;
        let intro: wire::Introduction = serde_json::from_slice(&plain[..n])?;
        ensure!(
            intro.name.len() <= 80 && !intro.name.chars().any(char::is_control),
            "invalid name"
        );
        let phone = wire::base64(
            handshake
                .get_remote_static()
                .ok_or_else(|| anyhow::anyhow!("missing identity"))?,
        );
        let paired = {
            let s = self.state.lock().unwrap();
            s.credentials
                .as_ref()
                .and_then(|c| c.paired.as_ref())
                .is_some_and(|p| p == &phone)
        };
        if !paired {
            {
                let mut s = self.state.lock().unwrap();
                s.begin_pairing(generation, phone.clone(), intro)?;
            }
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                ensure!(
                    self.current(generation) && Instant::now() < deadline,
                    "approval timed out"
                );
                let approved = self
                    .state
                    .lock()
                    .unwrap()
                    .pending
                    .as_ref()
                    .and_then(|p| (p.key == phone).then_some(p.approved).flatten());
                match approved {
                    Some(false) => bail!("rejected"),
                    Some(true) => break,
                    None => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
            let mut s = self.state.lock().unwrap();
            let mut updated = s.credentials.as_ref().unwrap().clone();
            updated.paired = Some(phone);
            updated.save()?;
            s.credentials = Some(updated);
            s.pending = None;
        }
        ensure!(self.current(generation), "disabled");
        let n = handshake.write_message(b"approved", &mut plain)?;
        socket.send(Ws::Binary(plain[..n].to_vec().into())).await?;
        let mut cipher = wire::Channel::new(handshake.into_transport_mode()?);
        self.describe(
            generation,
            "iPhone conectado · selecciona una terminal compartida",
        );
        let mut controller: Option<(Uuid, Arc<dyn TerminalHandle>)> = None;
        let mut previous: Option<RemoteFrame> = None;
        let mut revision = 0u64;
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut heartbeat = Instant::now();
        let mut last_ping = Instant::now();
        let mut last_request = 0;
        let mut rate = (Instant::now(), 0usize, 0usize);
        // RAII releases PTY size on every error, cancellation and disconnected peer.
        let _release = ConnectionRelease(self, generation);
        loop {
            ensure!(self.current(generation), "disabled");
            tokio::select! {
                incoming=socket.next()=> {
                    let Some(Ok(incoming))=incoming else { bail!("disconnected") };
                    let record=match incoming { Ws::Binary(b)=>b, Ws::Ping(b)=>{socket.send(Ws::Pong(b)).await?;continue},Ws::Pong(_)=>continue,_=>bail!("invalid transport") };
                    let Some(bytes)=cipher.open(&record)? else {continue};
                    let envelope=Envelope::decode(&bytes)?;
                    ensure!(envelope.request_id>last_request,"replayed request"); last_request=envelope.request_id;
                    if rate.0.elapsed()>Duration::from_secs(1) { rate=(Instant::now(),0,0); } rate.1+=bytes.len(); rate.2+=1; ensure!(rate.1<=256*1024 && rate.2<=200,"input rate exceeded");
                    heartbeat=Instant::now();
                    let request=envelope.request_id;
                    let response=match envelope.message {
                        Message::Ping {nonce}=>Some(Message::Pong {nonce}),Message::Pong{..}=>None,
                        Message::ListPanes{}=>Some(Message::Panes {panes:self.panes()}),
                        Message::Open{pane_id,size}=> {
                            if let Ok(h)=self.handle(pane_id) {
                                if let Some((_,old))=controller.take() {old.remote_release();}
                                h.remote_claim(terminal_size(size))?; controller=Some((pane_id,h)); previous=None;
                                self.describe(generation,"iPhone controla una terminal · menú del pane para recuperar"); None
                            } else {Some(Message::Error{code:ErrorCode::NotShared})}
                        },
                        Message::Close{pane_id}=> {
                            if controller.as_ref().is_some_and(|(id,_)|*id==pane_id) {controller.take().unwrap().1.remote_release();previous=None;}
                            Some(Message::ControlReleased{pane_id,reason:ReleaseReason::Closed})
                        },
                        Message::Resize{pane_id,size}=> {
                            if let Some((id,h))=&controller && *id==pane_id && h.remote_controlled() && self.handle(pane_id).is_ok() {h.remote_resize(terminal_size(size))?;previous=None;None} else {Some(Message::Error{code:ErrorCode::NotController})}
                        },
                        Message::Input{pane_id,input}=> {
                            if let Some((id,h))=&controller && *id==pane_id && previous.is_some() && self.handle(pane_id).is_ok() && h.remote_controlled() {h.remote_input(encode_input(input,h.input_mode()))?;None} else {Some(Message::Error{code:ErrorCode::NotController})}
                        },
                        Message::Resync{pane_id}=> {if controller.as_ref().is_some_and(|(id,_)|*id==pane_id) {previous=None;} None},
                        Message::History{pane_id,lines}=>self.handle(pane_id).ok().map(|h|Message::HistoryResult{pane_id,text:bounded_history(h.recent_text(lines as usize).unwrap_or_default())}),
                        _=>Some(Message::Error{code:ErrorCode::InvalidMessage}),
                    };
                    if let Some(message)=response {send(&mut socket,&mut cipher,Envelope::new(request,message)).await?;}
                },
                _=tick.tick()=> {
                    ensure!(heartbeat.elapsed()<Duration::from_secs(15),"heartbeat expired");
                    if last_ping.elapsed()>Duration::from_secs(5) {send(&mut socket,&mut cipher,Envelope::new(0,Message::Ping{nonce:now()})).await?;last_ping=Instant::now();}
                    if let Some((id,h))=&controller {
                        let id=*id;
                        if !h.remote_controlled() || self.handle(id).is_err() {
                            h.remote_release();controller=None;previous=None;
                            send(&mut socket,&mut cipher,Envelope::new(0,Message::ControlReleased{pane_id:id,reason:ReleaseReason::Reclaimed})).await?;
                        } else {
                            let frame=h.remote_frame()?;
                            if previous.as_ref()!=Some(&frame) {
                                let full=previous.as_ref().is_none_or(|p|p.columns!=frame.columns || p.rows!=frame.rows);
                                let ansi=draw(&frame,if full {None} else {previous.as_ref()});
                                let base=revision;revision+=1;
                                let message=if full {Message::Screen{pane_id:id,revision,size:Size{columns:frame.columns,rows:frame.rows},ansi}} else {Message::Patch{pane_id:id,base_revision:base,revision,ansi}};
                                send(&mut socket,&mut cipher,Envelope::new(0,message)).await?;previous=Some(frame);
                            }
                        }
                    }
                }
            }
        }
    }
}
struct ConnectionRelease<'a>(&'a Hub, u64);
impl Drop for ConnectionRelease<'_> {
    fn drop(&mut self) {
        self.0.reset_connection(self.1);
    }
}
type Socket = wire::WebSocketStream<wire::MaybeTlsStream<tokio::net::TcpStream>>;
async fn receive_binary(socket: &mut Socket, timeout: Duration) -> Result<Vec<u8>> {
    tokio::time::timeout(timeout, async {
        loop {
            match socket.next().await {
                Some(Ok(Ws::Binary(b))) if b.len() <= wire::WIRE_LIMIT => return Ok(b.to_vec()),
                Some(Ok(Ws::Ping(b))) => socket.send(Ws::Pong(b)).await?,
                Some(Ok(Ws::Pong(_))) => {}
                _ => bail!("expected handshake"),
            }
        }
    })
    .await?
}
async fn send(socket: &mut Socket, cipher: &mut wire::Channel, envelope: Envelope) -> Result<()> {
    let records = cipher.seal(&envelope.encode()?)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        for record in records {
            socket.send(Ws::Binary(record.into())).await?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
fn terminal_size(s: Size) -> TerminalSize {
    TerminalSize {
        columns: s.columns,
        rows: s.rows,
        ..TerminalSize::default()
    }
}
pub(super) fn draw(frame: &RemoteFrame, previous: Option<&RemoteFrame>) -> String {
    // Disable wrapping/origin and reset margins so drawing never scrolls the viewport.
    let mut out = String::from("\x1b[?25l\x1b[?7l\x1b[?6l\x1b[r");
    if previous.is_none() {
        out.push_str("\x1b[0m\x1b[2J");
    }
    for (row, text) in frame.lines.iter().enumerate() {
        if previous.is_none_or(|p| p.lines.get(row) != Some(text)) {
            out.push_str(&format!("\x1b[{};1H\x1b[0m\x1b[2K{}", row + 1, text));
        }
    }
    if previous.is_none_or(|p| p.palette != frame.palette) {
        out.push_str(&frame.palette);
    }
    out.push_str(&frame.cursor);
    out
}
fn bounded_history(mut text: String) -> String {
    const LIMIT: usize = 512 * 1024;
    if text.len() > LIMIT {
        let mut start = text.len() - LIMIT;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text = format!("… historial truncado …\n{}", &text[start..]);
    }
    text
}
fn encode_input(input: Input, mode: crate::ports::terminal::TerminalInputMode) -> Vec<u8> {
    match input {
        Input::Paste { text } => {
            if mode.bracketed_paste {
                format!("\x1b[200~{text}\x1b[201~").into_bytes()
            } else {
                text.into_bytes()
            }
        }
        Input::Text { text } => text
            .chars()
            .flat_map(|ch| {
                let text = ch.to_string();
                keyboard::key_event_bytes(
                    &TerminalKeystroke {
                        key: text.clone(),
                        key_char: Some(text.clone()),
                        modifiers: TerminalModifiers::default(),
                    },
                    mode,
                    TerminalKeyEventType::Press,
                )
                .unwrap_or_else(|| text.into_bytes())
            })
            .collect(),
        Input::Key { key, modifiers } => {
            let key = match key {
                Key::Character(c) => c.to_string(),
                Key::Escape => "escape".into(),
                Key::Tab => "tab".into(),
                Key::Enter => "enter".into(),
                Key::Backspace => "backspace".into(),
                Key::Delete => "delete".into(),
                Key::Up => "up".into(),
                Key::Down => "down".into(),
                Key::Left => "left".into(),
                Key::Right => "right".into(),
                Key::Home => "home".into(),
                Key::End => "end".into(),
                Key::PageUp => "pageup".into(),
                Key::PageDown => "pagedown".into(),
            };
            keyboard::key_event_bytes(
                &TerminalKeystroke {
                    key_char: Some(key.clone()),
                    key: key.clone(),
                    modifiers: TerminalModifiers {
                        shift: modifiers.contains(&Modifier::Shift),
                        control: modifiers.contains(&Modifier::Control),
                        alt: modifiers.contains(&Modifier::Alt),
                        platform: modifiers.contains(&Modifier::Super),
                    },
                },
                mode,
                TerminalKeyEventType::Press,
            )
            .unwrap_or_else(|| key.into_bytes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::terminal::*;
    struct TestTerminal {
        state: Mutex<(bool, TerminalSize, Vec<u8>)>,
    }
    impl TerminalHandle for TestTerminal {
        fn events(&self) -> async_channel::Receiver<TerminalEvent> {
            async_channel::unbounded().1
        }
        fn send_input(&self, _: Vec<u8>) -> Result<()> {
            unreachable!()
        }
        fn resize(&self, _: TerminalSize) -> Result<()> {
            unreachable!()
        }
        fn scroll(&self, _: i32) {}
        fn clear_scrollback(&self) {}
        fn snapshot(&self) -> Arc<TerminalSnapshot> {
            unreachable!("remote must not use local snapshot")
        }
        fn input_mode(&self) -> TerminalInputMode {
            TerminalInputMode::default()
        }
        fn clear_selection(&self) {}
        fn start_selection(&self, _: TerminalSelectionType, _: TerminalPoint, _: TerminalCellSide) {
        }
        fn update_selection(&self, _: TerminalPoint, _: TerminalCellSide) {}
        fn selection_text(&self) -> Option<String> {
            None
        }
        fn search(&self, _: &str, _: TerminalSearchDirection) -> Result<bool> {
            Ok(false)
        }
        fn hyperlink_at(&self, _: TerminalPoint) -> Option<String> {
            None
        }
        fn acknowledge_wakeup(&self) {}
        fn shutdown(&self) {}
        fn remote_size(&self) -> TerminalSize {
            self.state.lock().unwrap().1
        }
        fn remote_claim(&self, size: TerminalSize) -> Result<()> {
            let mut s = self.state.lock().unwrap();
            s.0 = true;
            s.1 = size;
            Ok(())
        }
        fn remote_resize(&self, size: TerminalSize) -> Result<()> {
            let mut s = self.state.lock().unwrap();
            ensure!(s.0, "released");
            s.1 = size;
            Ok(())
        }
        fn remote_controlled(&self) -> bool {
            self.state.lock().unwrap().0
        }
        fn remote_release(&self) {
            self.state.lock().unwrap().0 = false;
        }
        fn remote_input(&self, bytes: Vec<u8>) -> Result<()> {
            let mut s = self.state.lock().unwrap();
            ensure!(s.0, "not controlled");
            s.2.extend(bytes);
            Ok(())
        }
        fn remote_frame(&self) -> Result<RemoteFrame> {
            let size = self.remote_size();
            Ok(RemoteFrame {
                columns: size.columns,
                rows: size.rows,
                lines: vec!["Español 日本語 🦀".into(); size.rows as usize],
                cursor: "\x1b[1;1H".into(),
                palette: String::new(),
            })
        }
    }
    struct Relay(std::process::Child);
    impl Drop for Relay {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    async fn read(socket: &mut Socket, cipher: &mut wire::Channel) -> Envelope {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let record = receive_binary(socket, Duration::from_secs(3))
                    .await
                    .unwrap();
                if let Some(message) = cipher.open(&record).unwrap() {
                    return Envelope::decode(&message).unwrap();
                }
            }
        })
        .await
        .unwrap()
    }
    #[test]
    fn remote_invitation_is_expiring_single_use_and_invalidated_by_disable() {
        let credentials = Credentials::fresh("ws://127.0.0.1:8787/ws".into()).unwrap();
        let mut state = State {
            enabled: true,
            generation: 1,
            credentials: Some(credentials),
            invitation: Some(("one-use".into(), now() + 300)),
            panes: HashMap::new(),
            pending: None,
            status: String::new(),
        };
        let intro = |token: &str| wire::Introduction {
            invitation: token.into(),
            name: "iPhone".into(),
        };
        assert!(
            state
                .begin_pairing(1, "phone".into(), intro("wrong"))
                .is_err()
        );
        assert!(state.invitation.is_some());
        state.invitation.as_mut().unwrap().1 = now() - 1;
        assert!(
            state
                .begin_pairing(1, "phone".into(), intro("one-use"))
                .is_err()
        );
        state.invitation.as_mut().unwrap().1 = now() + 300;
        state
            .begin_pairing(1, "phone".into(), intro("one-use"))
            .unwrap();
        assert!(state.invitation.is_none());
        assert!(
            state
                .begin_pairing(1, "phone".into(), intro("one-use"))
                .is_err()
        );
        let hub = Hub {
            state: Mutex::new(state),
        };
        hub.disable();
        hub.approve(true);
        assert!(!hub.current(1));
        assert!(hub.state.lock().unwrap().pending.is_none());
    }
    #[test]
    fn remote_keyboard_uses_host_modes() {
        let mode = TerminalInputMode {
            application_cursor: true,
            bracketed_paste: true,
            ..Default::default()
        };
        assert_eq!(
            encode_input(
                Input::Key {
                    key: Key::Up,
                    modifiers: vec![]
                },
                mode
            ),
            b"\x1bOA"
        );
        assert_eq!(
            encode_input(
                Input::Key {
                    key: Key::Character('c'),
                    modifiers: vec![Modifier::Control]
                },
                mode
            ),
            [3]
        );
        assert_eq!(
            encode_input(
                Input::Paste {
                    text: "one\ntwo".into()
                },
                mode
            ),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
        let mode = TerminalInputMode {
            report_all_keys_as_escape_codes: true,
            ..Default::default()
        };
        assert_eq!(
            encode_input(Input::Text { text: "a".into() }, mode),
            b"\x1b[97u"
        );
    }
    #[tokio::test]
    #[ignore = "requires built relay; run Scripts/verify_remote.sh"]
    async fn remote_relay_end_to_end() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let path = std::env::var("VIBRA_RELAY_TEST_BINARY").expect("relay binary");
        let _relay = Relay(
            std::process::Command::new(path)
                .env("VIBRA_RELAY_BIND", address.to_string())
                .spawn()
                .unwrap(),
        );
        for attempt in 0..100 {
            if tokio::net::TcpStream::connect(address).await.is_ok() {
                break;
            }
            assert!(attempt < 99);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let phone = wire::keypair().unwrap();
        let mut c = Credentials::fresh(format!("ws://{address}/ws")).unwrap();
        c.paired = Some(wire::base64(&phone.public));
        let hub = Hub {
            state: Mutex::new(State {
                enabled: true,
                generation: 1,
                credentials: Some(c.clone()),
                invitation: None,
                panes: HashMap::new(),
                pending: None,
                status: String::new(),
            }),
        };
        let terminal = Arc::new(TestTerminal {
            state: Mutex::new((false, TerminalSize::default(), Vec::new())),
        });
        let handle = terminal.clone() as Arc<dyn TerminalHandle>;
        let id = Uuid::new_v4();
        let hidden = Uuid::new_v4();
        hub.register(id, &handle);
        hub.register(hidden, &handle);
        hub.toggle_share(id);
        let host = hub.connect(1, &c);
        let client = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let (mut socket, _) = wire::connect_async(&c.relay).await.unwrap();
            socket
                .send(Ws::Text(
                    serde_json::to_string(&wire::Hello {
                        role: "phone".into(),
                        channel: c.channel.clone(),
                        token: c.phone_token.clone(),
                        peer_token: None,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            assert!(matches!(socket.next().await,Some(Ok(Ws::Text(t))) if t=="peer"));
            let mut noise =
                wire::handshake(&phone.private, Some(&wire::unbase64(&c.public).unwrap())).unwrap();
            let mut out = vec![0; wire::WIRE_LIMIT];
            let intro = serde_json::to_vec(&wire::Introduction {
                name: "iPhone test".into(),
                invitation: String::new(),
            })
            .unwrap();
            let n = noise.write_message(&intro, &mut out).unwrap();
            socket
                .send(Ws::Binary(out[..n].to_vec().into()))
                .await
                .unwrap();
            let response = receive_binary(&mut socket, Duration::from_secs(3))
                .await
                .unwrap();
            let n = noise.read_message(&response, &mut out).unwrap();
            assert_eq!(&out[..n], b"approved");
            let mut cipher = wire::Channel::new(noise.into_transport_mode().unwrap());
            send(
                &mut socket,
                &mut cipher,
                Envelope::new(1, Message::ListPanes {}),
            )
            .await
            .unwrap();
            match read(&mut socket, &mut cipher).await.message {
                Message::Panes { panes } => {
                    assert_eq!(panes.len(), 1);
                    assert_eq!(panes[0].id, id)
                }
                _ => panic!("panes"),
            }
            let size = Size {
                columns: 40,
                rows: 20,
            };
            send(
                &mut socket,
                &mut cipher,
                Envelope::new(
                    2,
                    Message::Open {
                        pane_id: hidden,
                        size,
                    },
                ),
            )
            .await
            .unwrap();
            assert_eq!(
                read(&mut socket, &mut cipher).await.message,
                Message::Error {
                    code: ErrorCode::NotShared
                }
            );
            send(
                &mut socket,
                &mut cipher,
                Envelope::new(3, Message::Open { pane_id: id, size }),
            )
            .await
            .unwrap();
            assert!(matches!(
                read(&mut socket, &mut cipher).await.message,
                Message::Screen {
                    size: Size {
                        columns: 40,
                        rows: 20
                    },
                    ..
                }
            ));
            assert!(terminal.remote_controlled());
            send(
                &mut socket,
                &mut cipher,
                Envelope::new(
                    4,
                    Message::Input {
                        pane_id: id,
                        input: Input::Key {
                            key: Key::Character('c'),
                            modifiers: vec![Modifier::Control],
                        },
                    },
                ),
            )
            .await
            .unwrap();
            send(
                &mut socket,
                &mut cipher,
                Envelope::new(5, Message::Ping { nonce: 5 }),
            )
            .await
            .unwrap();
            assert_eq!(
                read(&mut socket, &mut cipher).await.message,
                Message::Pong { nonce: 5 }
            );
            assert_eq!(terminal.state.lock().unwrap().2, [3]);
            hub.reclaim(id);
            assert!(matches!(
                read(&mut socket, &mut cipher).await.message,
                Message::ControlReleased {
                    reason: ReleaseReason::Reclaimed,
                    ..
                }
            ));
            assert!(!terminal.remote_controlled());
            send(
                &mut socket,
                &mut cipher,
                Envelope::new(6, Message::Open { pane_id: id, size }),
            )
            .await
            .unwrap();
            assert!(matches!(
                read(&mut socket, &mut cipher).await.message,
                Message::Screen { .. }
            ));
            hub.toggle_share(id);
            assert!(matches!(
                read(&mut socket, &mut cipher).await.message,
                Message::ControlReleased { .. }
            ));
            assert!(!terminal.remote_controlled());
            hub.toggle_share(id);
            send(
                &mut socket,
                &mut cipher,
                Envelope::new(7, Message::Open { pane_id: id, size }),
            )
            .await
            .unwrap();
            let _ = read(&mut socket, &mut cipher).await;
            // Stop all heartbeat responses while retaining the websocket: lease must expire.
            tokio::time::sleep(Duration::from_secs(16)).await;
            assert!(!terminal.remote_controlled());
            socket.close(None).await.ok();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(25), async {
            tokio::join!(host, client)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(!terminal.remote_controlled());
    }
}
