//! The meeting place: plates, roles, burn, release, TTL and the per-IP limits. Time is a
//! millisecond value the transport passes in, so a test drives expiry exactly and the relay never
//! reads a clock here.
//!
//! The state machine never parses a ticket, a word or a SPAKE2 message. It copies bytes between the
//! two peers with one role rule: only the sender may send `Sealed`. Nothing is persisted, and the
//! only thing it logs is a refusal's code.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;

use crate::code::MAX_NAMEPLATE;
use crate::frame::{ClientFrame, ErrorCode, Role, ServerFrame};

mod config;
mod outbound;

pub use config::{Config, ConfigError, DEFAULT_MAX_CONNECTIONS_PER_IP};
pub use outbound::Outbound;

/// A connection's id, chosen by the meeting place. It is not a secret and not a nameplate.
pub type ConnectionId = u64;

#[derive(Debug, Clone, Copy)]
struct Conn {
    ip: IpAddr,
    role: Option<Role>,
    plate: Option<u32>,
}

#[derive(Debug)]
struct Plate {
    created_ms: u64,
    sender: ConnectionId,
    receiver: Option<ConnectionId>,
}

#[derive(Debug)]
struct Bucket {
    tokens: u64,
    last_ms: u64,
}

/// The relay's half of a meeting. One instance holds every plate and every connection's role.
#[derive(Debug)]
pub struct MeetingPlace {
    config: Config,
    next_connection: ConnectionId,
    next_plate: u32,
    plates: BTreeMap<u32, Plate>,
    connections: HashMap<ConnectionId, Conn>,
    per_ip: HashMap<IpAddr, usize>,
    buckets: HashMap<IpAddr, Bucket>,
}

impl MeetingPlace {
    /// A meeting place with `config` and no plates or connections.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            next_connection: 1,
            next_plate: 0,
            plates: BTreeMap::new(),
            connections: HashMap::new(),
            per_ip: HashMap::new(),
            buckets: HashMap::new(),
        }
    }

    /// The limits this meeting place enforces.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// How many plates are held.
    pub fn plates(&self) -> usize {
        self.plates.len()
    }

    /// How many `/meet` connections `ip` holds.
    pub fn connections_for(&self, ip: IpAddr) -> usize {
        self.per_ip.get(&ip).copied().unwrap_or(0)
    }

    /// Registers one `/meet` connection. The per-IP cap is checked before the connection is
    /// counted, so a refused connection does not keep a place.
    pub fn connect(&mut self, ip: IpAddr, _now_ms: u64) -> Result<ConnectionId, ErrorCode> {
        let held = self.connections_for(ip);
        if held >= self.config.max_connections_per_ip {
            return Err(ErrorCode::RateLimited);
        }
        let id = self.next_connection;
        self.next_connection = self.next_connection.saturating_add(1);
        self.connections.insert(
            id,
            Conn {
                ip,
                role: None,
                plate: None,
            },
        );
        *self.per_ip.entry(ip).or_default() += 1;
        Ok(id)
    }

    /// Ends one connection: the sender's close releases its plate and tells the receiver; the
    /// receiver's close before `Done` burns the plate and tells the sender.
    pub fn disconnect(&mut self, id: ConnectionId, _now_ms: u64) -> Vec<Outbound> {
        let Some(conn) = self.connections.remove(&id) else {
            return Vec::new();
        };
        if let Some(count) = self.per_ip.get_mut(&conn.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.per_ip.remove(&conn.ip);
            }
        }
        let (Some(role), Some(plate)) = (conn.role, conn.plate) else {
            return Vec::new();
        };
        let Some(held) = self.plates.get(&plate) else {
            return Vec::new();
        };
        match role {
            Role::Sender => {
                let receiver = held.receiver;
                self.plates.remove(&plate);
                self.detach(Some(id));
                self.detach(receiver);
                receiver
                    .map(|to| Outbound {
                        to,
                        frame: ServerFrame::PeerClosed,
                    })
                    .into_iter()
                    .collect()
            }
            Role::Receiver => self.burn(plate),
        }
    }

    /// Handles one frame from `id`. The returned frames are addressed to the connections that
    /// should receive them. An `Err` is the refusal to send back on `id` before closing it.
    pub fn receive(
        &mut self,
        id: ConnectionId,
        frame: ClientFrame,
        now_ms: u64,
    ) -> Result<Vec<Outbound>, ErrorCode> {
        let conn = *self.connections.get(&id).ok_or(ErrorCode::BadFrame)?;
        match frame {
            ClientFrame::Claim => self.claim(id, conn, now_ms),
            ClientFrame::Join(nameplate) => self.join(id, conn, nameplate, now_ms),
            ClientFrame::Spake(message) => {
                let role = conn.role.ok_or(ErrorCode::BadFrame)?;
                self.forward(id, role, |_| ServerFrame::Spake(message))
            }
            ClientFrame::Confirm(tag) => {
                let role = conn.role.ok_or(ErrorCode::BadFrame)?;
                self.forward(id, role, |_| ServerFrame::Confirm(tag))
            }
            ClientFrame::Sealed(sealed) => {
                if conn.role != Some(Role::Sender) {
                    return Err(ErrorCode::BadFrame);
                }
                self.forward(id, Role::Sender, |_| ServerFrame::Sealed(sealed))
            }
            ClientFrame::Fail(_) => {
                let plate = conn.plate.ok_or(ErrorCode::BadFrame)?;
                Ok(self.burn(plate))
            }
            ClientFrame::Done => {
                if conn.role != Some(Role::Receiver) {
                    return Err(ErrorCode::BadFrame);
                }
                let plate = conn.plate.ok_or(ErrorCode::BadFrame)?;
                Ok(self.done(plate))
            }
        }
    }

    /// Removes every plate past its TTL and tells both sides. A relay calls this on a timer and may
    /// call it after any frame; [`MeetingPlace::receive`] does not need it to be exact.
    pub fn expire(&mut self, now_ms: u64) -> Vec<Outbound> {
        let expired: Vec<u32> = self
            .plates
            .iter()
            .filter(|(_, plate)| self.past_ttl(plate, now_ms))
            .map(|(plate, _)| *plate)
            .collect();
        let mut out = Vec::new();
        for plate in expired {
            out.extend(self.expire_plate(plate));
        }
        self.prune_buckets(now_ms);
        out
    }

    fn claim(
        &mut self,
        id: ConnectionId,
        conn: Conn,
        now_ms: u64,
    ) -> Result<Vec<Outbound>, ErrorCode> {
        if conn.role.is_some() {
            return Err(ErrorCode::BadFrame);
        }
        self.take_bucket(conn.ip, now_ms)?;
        let mut out = self.expire(now_ms);
        if self.plates.len() >= self.config.max_plates {
            return Err(ErrorCode::Full);
        }
        let plate = self.free_plate().ok_or(ErrorCode::Full)?;
        self.plates.insert(
            plate,
            Plate {
                created_ms: now_ms,
                sender: id,
                receiver: None,
            },
        );
        if let Some(conn) = self.connections.get_mut(&id) {
            conn.role = Some(Role::Sender);
            conn.plate = Some(plate);
        }
        out.push(Outbound {
            to: id,
            frame: ServerFrame::Claimed(plate),
        });
        Ok(out)
    }

    fn join(
        &mut self,
        id: ConnectionId,
        conn: Conn,
        plate: u32,
        now_ms: u64,
    ) -> Result<Vec<Outbound>, ErrorCode> {
        if conn.role.is_some() {
            return Err(ErrorCode::BadFrame);
        }
        self.take_bucket(conn.ip, now_ms)?;
        let held = self.plates.get(&plate).ok_or(ErrorCode::NoCode)?;
        if self.past_ttl(held, now_ms) {
            let out = self.expire_plate(plate);
            return Err(if out.is_empty() {
                ErrorCode::NoCode
            } else {
                ErrorCode::Expired
            });
        }
        if held.receiver.is_some() {
            return Err(ErrorCode::Busy);
        }
        let sender = held.sender;
        if let Some(held) = self.plates.get_mut(&plate) {
            held.receiver = Some(id);
        }
        if let Some(conn) = self.connections.get_mut(&id) {
            conn.role = Some(Role::Receiver);
            conn.plate = Some(plate);
        }
        Ok(vec![
            Outbound {
                to: sender,
                frame: ServerFrame::Joined(Role::Sender),
            },
            Outbound {
                to: id,
                frame: ServerFrame::Joined(Role::Receiver),
            },
        ])
    }

    fn forward(
        &self,
        id: ConnectionId,
        role: Role,
        frame: impl FnOnce(Role) -> ServerFrame,
    ) -> Result<Vec<Outbound>, ErrorCode> {
        let conn = self.connections.get(&id).ok_or(ErrorCode::BadFrame)?;
        let plate = self.plates.get(&conn.plate.ok_or(ErrorCode::BadFrame)?);
        let Some(plate) = plate else {
            return Err(ErrorCode::NoCode);
        };
        let to = match role {
            Role::Sender => plate.receiver,
            Role::Receiver => Some(plate.sender),
        };
        Ok(to
            .map(|to| Outbound {
                to,
                frame: frame(role),
            })
            .into_iter()
            .collect())
    }

    fn burn(&mut self, plate: u32) -> Vec<Outbound> {
        let Some(held) = self.plates.remove(&plate) else {
            return Vec::new();
        };
        let mut ids = vec![held.sender];
        ids.extend(held.receiver);
        for id in &ids {
            self.detach(Some(*id));
        }
        ids.iter()
            .map(|to| Outbound {
                to: *to,
                frame: ServerFrame::Burned,
            })
            .collect()
    }

    fn done(&mut self, plate: u32) -> Vec<Outbound> {
        let Some(held) = self.plates.remove(&plate) else {
            return Vec::new();
        };
        self.detach(Some(held.sender));
        self.detach(held.receiver);
        vec![Outbound {
            to: held.sender,
            frame: ServerFrame::Done,
        }]
    }

    fn expire_plate(&mut self, plate: u32) -> Vec<Outbound> {
        let Some(held) = self.plates.remove(&plate) else {
            return Vec::new();
        };
        let mut ids = vec![held.sender];
        ids.extend(held.receiver);
        for id in &ids {
            self.detach(Some(*id));
        }
        ids.iter()
            .map(|to| Outbound {
                to: *to,
                frame: ServerFrame::Error(ErrorCode::Expired),
            })
            .collect()
    }

    fn detach(&mut self, id: Option<ConnectionId>) {
        if let Some(conn) = id.and_then(|id| self.connections.get_mut(&id)) {
            conn.role = None;
            conn.plate = None;
        }
    }

    fn past_ttl(&self, plate: &Plate, now_ms: u64) -> bool {
        now_ms.saturating_sub(plate.created_ms) >= self.config.ttl_ms()
    }

    fn free_plate(&mut self) -> Option<u32> {
        for _ in 0..MAX_NAMEPLATE {
            self.next_plate = self.next_plate % MAX_NAMEPLATE + 1;
            if !self.plates.contains_key(&self.next_plate) {
                return Some(self.next_plate);
            }
        }
        None
    }

    fn take_bucket(&mut self, ip: IpAddr, now_ms: u64) -> Result<(), ErrorCode> {
        let capacity = self.config.bucket_capacity();
        let bucket = self.buckets.entry(ip).or_insert(Bucket {
            tokens: capacity,
            last_ms: now_ms,
        });
        let elapsed = now_ms.saturating_sub(bucket.last_ms);
        bucket.tokens = bucket
            .tokens
            .saturating_add(elapsed.saturating_mul(u64::from(self.config.rate_per_ip)))
            .min(capacity);
        bucket.last_ms = now_ms;
        if bucket.tokens < 60_000 {
            return Err(ErrorCode::RateLimited);
        }
        bucket.tokens -= 60_000;
        Ok(())
    }

    fn prune_buckets(&mut self, now_ms: u64) {
        let capacity = self.config.bucket_capacity();
        self.buckets.retain(|_, bucket| {
            bucket.tokens < capacity || now_ms.saturating_sub(bucket.last_ms) < 60_000
        });
    }
}

#[cfg(test)]
mod tests;
