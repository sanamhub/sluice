//! The meeting place: two connections meet at a nameplate and nothing is persisted.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_websockets::{Message, WebSocketStream};

use crate::wire::{ClientMsg, ErrorCode, Role, ServerMsg, MAX_FRAME};

#[derive(Debug, Clone)]
pub struct Config {
    pub max_plates: usize,
    pub ttl: Duration,
    pub claim_burst: f64,
    pub claim_per_second: f64,
    pub max_conns_per_ip: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_plates: 4096,
            ttl: Duration::from_secs(600),
            claim_burst: 5.0,
            claim_per_second: 1.0,
            max_conns_per_ip: 8,
        }
    }
}

pub struct MeetingPlace {
    config: Config,
    inner: Mutex<Inner>,
}

struct Inner {
    plates: HashMap<u32, Plate>,
    next: u32,
    buckets: HashMap<IpAddr, Bucket>,
    connections: HashMap<IpAddr, usize>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

struct Plate {
    created: Instant,
    sender: Peer,
    receiver: Option<Peer>,
}

struct Peer {
    tx: UnboundedSender<ServerMsg>,
    #[allow(dead_code)]
    ip: IpAddr,
}

impl MeetingPlace {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: Mutex::new(Inner {
                plates: HashMap::new(),
                next: 0,
                buckets: HashMap::new(),
                connections: HashMap::new(),
            }),
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Counts a `/meet` connection against its IP. Returns false when the IP holds too many.
    pub fn connect(&self, ip: IpAddr) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let count = inner.connections.entry(ip).or_default();
        *count += 1;
        *count <= self.config.max_conns_per_ip
    }

    pub fn disconnect_conn(&self, ip: IpAddr) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(count) = inner.connections.get_mut(&ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                inner.connections.remove(&ip);
            }
        }
    }

    pub fn claim(&self, ip: IpAddr, tx: UnboundedSender<ServerMsg>) -> Result<u32, ErrorCode> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        inner.take(ip, now, &self.config)?;
        inner.sweep(now, self.config.ttl);
        if inner.plates.len() >= self.config.max_plates {
            return Err(ErrorCode::Full);
        }
        let plate = inner.free_plate().ok_or(ErrorCode::Full)?;
        inner.plates.insert(
            plate,
            Plate {
                created: now,
                sender: Peer { tx, ip },
                receiver: None,
            },
        );
        Ok(plate)
    }

    pub fn join(
        &self,
        ip: IpAddr,
        plate: u32,
        tx: UnboundedSender<ServerMsg>,
    ) -> Result<Role, ErrorCode> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        inner.take(ip, now, &self.config)?;
        inner.sweep(now, self.config.ttl);
        let p = inner.plates.get_mut(&plate).ok_or(ErrorCode::NoCode)?;
        if p.receiver.is_some() {
            return Err(ErrorCode::Busy);
        }
        p.receiver = Some(Peer { tx, ip });
        let _ = p.sender.tx.send(ServerMsg::Joined(Role::Sender));
        Ok(Role::Receiver)
    }

    pub fn forward(&self, plate: u32, from: Role, msg: ServerMsg) {
        let inner = self.inner.lock().unwrap();
        let Some(p) = inner.plates.get(&plate) else {
            return;
        };
        let target = match from {
            Role::Sender => p.receiver.as_ref(),
            Role::Receiver => Some(&p.sender),
        };
        if let Some(peer) = target {
            let _ = peer.tx.send(msg);
        }
    }

    pub fn burn(&self, plate: u32) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(p) = inner.plates.remove(&plate) {
            let _ = p.sender.tx.send(ServerMsg::Burned);
            if let Some(r) = p.receiver {
                let _ = r.tx.send(ServerMsg::Burned);
            }
        }
    }

    pub fn done(&self, plate: u32) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(p) = inner.plates.remove(&plate) {
            let _ = p.sender.tx.send(ServerMsg::Done);
        }
    }

    /// A connection ended. The sender's close releases the plate; the receiver's close before the
    /// ticket was sealed burns it, because it may have used the one guess.
    pub fn disconnected(&self, plate: u32, role: Role) {
        let mut inner = self.inner.lock().unwrap();
        let Some(p) = inner.plates.remove(&plate) else {
            return;
        };
        match role {
            Role::Sender => {
                if let Some(r) = p.receiver {
                    let _ = r.tx.send(ServerMsg::PeerClosed);
                }
            }
            Role::Receiver => {
                let _ = p.sender.tx.send(ServerMsg::Burned);
            }
        }
    }

    /// Drops plates that outlived the TTL and tells both sides.
    pub fn sweep_now(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.sweep(Instant::now(), self.config.ttl);
    }
}

impl Inner {
    fn take(&mut self, ip: IpAddr, now: Instant, config: &Config) -> Result<(), ErrorCode> {
        let bucket = self.buckets.entry(ip).or_insert(Bucket {
            tokens: config.claim_burst,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * config.claim_per_second).min(config.claim_burst);
        bucket.last = now;
        if bucket.tokens < 1.0 {
            return Err(ErrorCode::RateLimited);
        }
        bucket.tokens -= 1.0;
        Ok(())
    }

    fn free_plate(&mut self) -> Option<u32> {
        for _ in 0..crate::code::MAX_NAMEPLATE {
            self.next = self.next % crate::code::MAX_NAMEPLATE + 1;
            if !self.plates.contains_key(&self.next) {
                return Some(self.next);
            }
        }
        None
    }

    fn sweep(&mut self, now: Instant, ttl: Duration) {
        let expired: Vec<u32> = self
            .plates
            .iter()
            .filter(|(_, p)| now.saturating_duration_since(p.created) >= ttl)
            .map(|(plate, _)| *plate)
            .collect();
        for plate in expired {
            if let Some(p) = self.plates.remove(&plate) {
                let _ = p.sender.tx.send(ServerMsg::Error(ErrorCode::Expired));
                if let Some(r) = p.receiver {
                    let _ = r.tx.send(ServerMsg::Error(ErrorCode::Expired));
                }
            }
        }
    }
}

/// Serves one `/meet` WebSocket until it closes.
pub async fn serve_meet<S>(place: Arc<MeetingPlace>, mut ws: WebSocketStream<S>, ip: IpAddr)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if !place.connect(ip) {
        let _ = send(&mut ws, ServerMsg::Error(ErrorCode::RateLimited)).await;
        place.disconnect_conn(ip);
        return;
    }

    let (tx, mut rx): (UnboundedSender<ServerMsg>, _) = unbounded_channel();
    let mut role: Option<Role> = None;
    let mut plate: Option<u32> = None;
    let mut attached = false;

    loop {
        tokio::select! {
            incoming = ws.next() => {
                let Some(Ok(message)) = incoming else { break };
                if message.is_close() {
                    break;
                }
                if !message.is_binary() {
                    continue;
                }
                let bytes = message.into_payload();
                if bytes.len() > MAX_FRAME {
                    let _ = send(&mut ws, ServerMsg::Error(ErrorCode::TooLarge)).await;
                    break;
                }
                let Some(msg) = ClientMsg::decode(bytes.as_ref()) else {
                    let _ = send(&mut ws, ServerMsg::Error(ErrorCode::BadFrame)).await;
                    break;
                };
                match msg {
                    ClientMsg::Claim => {
                        if plate.is_some() {
                            let _ = send(&mut ws, ServerMsg::Error(ErrorCode::BadFrame)).await;
                            break;
                        }
                        match place.claim(ip, tx.clone()) {
                            Ok(p) => {
                                plate = Some(p);
                                role = Some(Role::Sender);
                                attached = true;
                                if send(&mut ws, ServerMsg::Claimed(p)).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                let _ = send(&mut ws, ServerMsg::Error(e)).await;
                                break;
                            }
                        }
                    }
                    ClientMsg::Join(p) => {
                        if plate.is_some() {
                            let _ = send(&mut ws, ServerMsg::Error(ErrorCode::BadFrame)).await;
                            break;
                        }
                        match place.join(ip, p, tx.clone()) {
                            Ok(r) => {
                                plate = Some(p);
                                role = Some(r);
                                attached = true;
                                if send(&mut ws, ServerMsg::Joined(r)).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                let _ = send(&mut ws, ServerMsg::Error(e)).await;
                                break;
                            }
                        }
                    }
                    ClientMsg::Spake(msg) => {
                        let (Some(p), Some(r)) = (plate, role) else { break };
                        place.forward(p, r, ServerMsg::Spake(msg));
                    }
                    ClientMsg::Confirm(tag) => {
                        let (Some(p), Some(r)) = (plate, role) else { break };
                        place.forward(p, r, ServerMsg::Confirm(tag));
                    }
                    ClientMsg::Sealed(sealed) => {
                        let (Some(p), Some(r)) = (plate, role) else { break };
                        if r == Role::Sender {
                            place.forward(p, r, ServerMsg::Sealed(sealed));
                        }
                    }
                    ClientMsg::Fail(_) => {
                        let Some(p) = plate else { break };
                        place.burn(p);
                        plate = None;
                    }
                    ClientMsg::Done => {
                        let Some(p) = plate else { break };
                        place.done(p);
                        plate = None;
                    }
                }
            }
            outgoing = rx.recv() => {
                let Some(msg) = outgoing else { break };
                if send(&mut ws, msg).await.is_err() {
                    break;
                }
            }
        }
    }

    if let (Some(p), Some(r)) = (plate, role) {
        if attached {
            place.disconnected(p, r);
        }
    }
    place.disconnect_conn(ip);
}

async fn send<S>(ws: &mut WebSocketStream<S>, msg: ServerMsg) -> Result<(), ()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    ws.send(Message::binary(msg.encode())).await.map_err(|_| ())
}
