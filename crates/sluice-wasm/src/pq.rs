//! `X25519MLKEM768` for the wasm build, and the probe that says which group a handshake agreed on.
//!
//! `ring` has no ML-KEM group, so a tab negotiates classical `X25519` and a native peer refuses it
//! (`docs/browser.md`). This module adds the hybrid group to `ring`'s provider: RustCrypto's
//! `ml-kem` 0.3 supplies ML-KEM-768, `ring` supplies X25519, and the two shares and the two secrets
//! are concatenated in the order rustls uses for the same group.
//!
//! The combiner is rustls's own, copied from
//! `rustls-0.23.46/src/crypto/aws_lc_rs/pq/hybrid.rs` (Apache-2.0 OR ISC OR MIT): that module is
//! `pub(crate)`, so the only way to reuse it is to say what it does. The layout there is the one
//! this file implements: the post-quantum share comes first in both directions, so a client share is
//! an ML-KEM-768 encapsulation key (1184 bytes) then an X25519 public key (32), a server share is an
//! ML-KEM-768 ciphertext (1088) then an X25519 public key (32), and the secret is the ML-KEM secret
//! (32) then the X25519 secret (32).
//!
//! This is a spike. Nothing in the product selects [`provider`] yet, and a tab still offers
//! `X25519` alone unless a caller asks for [`Mode::Hybrid`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use ml_kem::{
    DecapsulationKey, EncapsulationKey, MlKem768, Seed,
    kem::{Decapsulate, Encapsulate, KeyExport, KeyInit, TryKeyInit},
};
use rustls::{
    Error, NamedGroup, PeerMisbehaved, ProtocolVersion,
    crypto::{
        ActiveKeyExchange, CompletedKeyExchange, CryptoProvider, SharedSecret, SupportedKxGroup,
    },
};

/// The hybrid group's name as the draft spells it, which is also what a report prints.
pub const HYBRID_NAME: &str = "X25519MLKEM768";

/// The classical group's name, as a row that lost to the hybrid is reported.
pub const CLASSICAL_NAME: &str = "X25519";

/// An X25519 public key: the classical share in both directions, and the classical secret.
const X25519_SHARE: usize = 32;

/// A client's post-quantum share, which is an ML-KEM-768 encapsulation key.
const MLKEM768_ENCAPSULATION_KEY: usize = 1184;

/// A server's post-quantum share, which is an ML-KEM-768 ciphertext.
const MLKEM768_CIPHERTEXT: usize = 1088;

/// A shared secret that is not this group's share comes back as a peer that misbehaved, which is
/// what rustls's own hybrid does.
const INVALID_KEY_SHARE: Error = Error::PeerMisbehaved(PeerMisbehaved::InvalidKeyShare);

/// Which key exchange a run offers, so the same browser can be measured both ways.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `X25519` alone, which is what this build offers today.
    Classical,
    /// `X25519MLKEM768` first, then `X25519`, as rustls's post-quantum default orders them.
    Hybrid,
}

impl Mode {
    /// The mode a page or a command names.
    pub fn parse(text: &str) -> Option<Mode> {
        match text {
            "classical" => Some(Mode::Classical),
            "hybrid" => Some(Mode::Hybrid),
            _ => None,
        }
    }

    /// The name a report prints.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Classical => CLASSICAL_NAME,
            Mode::Hybrid => HYBRID_NAME,
        }
    }
}

/// The group a key exchange completed with, recorded as it completes.
///
/// A handshake that succeeds does not otherwise say which group it agreed on: rustls picks one from
/// the provider, and neither `iroh` nor `rustls` hands the negotiated group back through the
/// connection. Recording it where it is completed is the shortest way to make a run say what
/// happened instead of assuming it.
#[derive(Debug, Default)]
pub struct Probe {
    /// 0 for none, 1 for `X25519`, 2 for `X25519MLKEM768`.
    group: AtomicU8,
}

impl Probe {
    /// A probe that lives for the rest of the process, which is what a provider needs.
    pub fn new() -> &'static Probe {
        Box::leak(Box::new(Probe::default()))
    }

    /// The group that completed a key exchange, or `None` if none has.
    pub fn agreed(&self) -> Option<&'static str> {
        match self.group.load(Ordering::SeqCst) {
            1 => Some(CLASSICAL_NAME),
            2 => Some(HYBRID_NAME),
            _ => None,
        }
    }

    /// Record a completed group. A group this spike does not name is left unrecorded.
    fn record(&self, group: NamedGroup) {
        let code = match group {
            NamedGroup::X25519 => 1,
            NamedGroup::X25519MLKEM768 => 2,
            _ => 0,
        };
        if code != 0 {
            self.group.store(code, Ordering::SeqCst);
        }
    }
}

/// A group wrapped so that [`Probe::record`] runs when it completes, which is how the native peer
/// reports the group it agreed on without touching its own crypto.
pub fn wrap(
    inner: &'static dyn SupportedKxGroup,
    probe: &'static Probe,
) -> &'static dyn SupportedKxGroup {
    Box::leak(Box::new(Recorded { inner, probe }))
}

/// `ring`'s provider with the hybrid group added first, or `X25519` alone, and a probe that records
/// which one the handshake used.
pub fn provider(mode: Mode, probe: &'static Probe) -> Arc<CryptoProvider> {
    let base = rustls::crypto::ring::default_provider();
    let classical = rustls::crypto::ring::kx_group::X25519;
    let kx_groups = match mode {
        Mode::Classical => vec![wrap(classical, probe)],
        Mode::Hybrid => vec![
            Box::leak(Box::new(Hybrid { classical, probe })) as &'static dyn SupportedKxGroup,
            wrap(classical, probe),
        ],
    };
    Arc::new(CryptoProvider {
        kx_groups,
        ..base.clone()
    })
}

/// A group with a probe behind it.
#[derive(Debug)]
struct Recorded {
    inner: &'static dyn SupportedKxGroup,
    probe: &'static Probe,
}

impl SupportedKxGroup for Recorded {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        Ok(Box::new(RecordingActive {
            inner: self.inner.start()?,
            probe: self.probe,
        }))
    }

    fn start_and_complete(&self, peer_pub_key: &[u8]) -> Result<CompletedKeyExchange, Error> {
        let completed = self.inner.start_and_complete(peer_pub_key)?;
        self.probe.record(completed.group);
        Ok(completed)
    }

    fn name(&self) -> NamedGroup {
        self.inner.name()
    }

    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        self.inner.usable_for_version(version)
    }
}

/// An exchange in progress, with a probe behind it.
struct RecordingActive {
    inner: Box<dyn ActiveKeyExchange>,
    probe: &'static Probe,
}

impl ActiveKeyExchange for RecordingActive {
    fn complete(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        let group = self.inner.group();
        let secret = self.inner.complete(peer_pub_key)?;
        self.probe.record(group);
        Ok(secret)
    }

    fn hybrid_component(&self) -> Option<(NamedGroup, &[u8])> {
        self.inner.hybrid_component()
    }

    fn complete_hybrid_component(
        self: Box<Self>,
        peer_pub_key: &[u8],
    ) -> Result<SharedSecret, Error> {
        let group = self.inner.group();
        let secret = self.inner.complete_hybrid_component(peer_pub_key)?;
        self.probe.record(group);
        Ok(secret)
    }

    fn pub_key(&self) -> &[u8] {
        self.inner.pub_key()
    }

    fn group(&self) -> NamedGroup {
        self.inner.group()
    }
}

/// `ring`'s X25519 and RustCrypto's ML-KEM-768, combined the way rustls combines them.
#[derive(Debug)]
struct Hybrid {
    classical: &'static dyn SupportedKxGroup,
    probe: &'static Probe,
}

impl SupportedKxGroup for Hybrid {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let classical = self.classical.start()?;
        let mut seed = Seed::default();
        fill(seed.as_mut_slice());
        let decapsulation = DecapsulationKey::<MlKem768>::new(&seed);
        let encapsulation = decapsulation.encapsulation_key();
        let pub_key = concat(encapsulation.to_bytes().as_slice(), classical.pub_key());
        Ok(Box::new(ActiveHybrid {
            classical,
            decapsulation,
            pub_key,
            probe: self.probe,
        }))
    }

    fn start_and_complete(&self, client_share: &[u8]) -> Result<CompletedKeyExchange, Error> {
        let (encapsulation, classical_share) = split(client_share, MLKEM768_ENCAPSULATION_KEY)?;
        let encapsulation = EncapsulationKey::<MlKem768>::new_from_slice(encapsulation)
            .map_err(|_| INVALID_KEY_SHARE)?;
        let (ciphertext, post_quantum) =
            encapsulation.encapsulate_with_rng(&mut RingRng::default());
        let classical = self.classical.start_and_complete(classical_share)?;
        Ok(CompletedKeyExchange {
            group: self.name(),
            pub_key: concat(ciphertext.as_slice(), &classical.pub_key),
            secret: SharedSecret::from(concat(
                post_quantum.as_slice(),
                classical.secret.secret_bytes(),
            )),
        })
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }

    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        version == ProtocolVersion::TLSv1_3
    }
}

/// A client's half of the hybrid exchange: the classical exchange, the decapsulation key whose
/// encapsulation key the client sent, and the two shares as one.
struct ActiveHybrid {
    classical: Box<dyn ActiveKeyExchange>,
    decapsulation: DecapsulationKey<MlKem768>,
    pub_key: Vec<u8>,
    probe: &'static Probe,
}

impl ActiveKeyExchange for ActiveHybrid {
    fn complete(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        let (ciphertext, classical_share) = split(peer_pub_key, MLKEM768_CIPHERTEXT)?;
        let post_quantum = self
            .decapsulation
            .decapsulate_slice(ciphertext)
            .map_err(|_| INVALID_KEY_SHARE)?;
        let classical = self.classical.complete(classical_share)?;
        self.probe.record(NamedGroup::X25519MLKEM768);
        Ok(SharedSecret::from(concat(
            post_quantum.as_slice(),
            classical.secret_bytes(),
        )))
    }

    /// The classical share goes into the same `ClientHello`, as rustls's own hybrid does, so a
    /// server that prefers `X25519` costs no extra round trip.
    fn hybrid_component(&self) -> Option<(NamedGroup, &[u8])> {
        Some((self.classical.group(), self.classical.pub_key()))
    }

    fn complete_hybrid_component(
        self: Box<Self>,
        peer_pub_key: &[u8],
    ) -> Result<SharedSecret, Error> {
        let secret = self.classical.complete(peer_pub_key)?;
        self.probe.record(NamedGroup::X25519);
        Ok(secret)
    }

    fn pub_key(&self) -> &[u8] {
        &self.pub_key
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }
}

/// Split a share into its post-quantum and classical halves, in the order rustls sends them.
fn split(share: &[u8], post_quantum: usize) -> Result<(&[u8], &[u8]), Error> {
    if share.len() != post_quantum + X25519_SHARE {
        return Err(INVALID_KEY_SHARE);
    }
    Ok(share.split_at(post_quantum))
}

/// The post-quantum half first, then the classical one, which is the order for this group.
fn concat(post_quantum: &[u8], classical: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(post_quantum.len() + classical.len());
    out.extend_from_slice(post_quantum);
    out.extend_from_slice(classical);
    out
}

/// Fill `dest` from the same entropy TLS draws on.
fn fill(dest: &mut [u8]) {
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), dest)
        .expect("the system random failed");
}

/// `ml-kem`'s trait surface takes a `rand_core` generator; TLS already has one, in `ring`.
#[derive(Debug)]
struct RingRng(ring::rand::SystemRandom);

impl Default for RingRng {
    fn default() -> Self {
        RingRng(ring::rand::SystemRandom::new())
    }
}

impl rand_core::TryRng for RingRng {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut bytes = [0u8; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0u8; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
        ring::rand::SecureRandom::fill(&self.0, dest).expect("the system random failed");
        Ok(())
    }
}

impl rand_core::TryCryptoRng for RingRng {}

#[cfg(test)]
mod tests;
