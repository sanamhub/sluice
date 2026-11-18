//! SPAKE2 over the two words, key confirmation, and the sealed ticket.
//!
//! The two identities bind the fixed label and the nameplate, so a transcript from another plate or
//! another protocol version cannot be replayed. The SPAKE2 key is run through HKDF-SHA256 with the
//! transcript as salt; two confirmation tags and one AEAD key come out. The ticket travels under
//! ChaCha20-Poly1305 with the transcript as associated data.
//!
//! The word-bound exchange is the security path, and `spake2` 0.4.0 has no independent audit; see the
//! crate documentation.

use std::fmt;

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf::{self, KeyType, Salt};
use ring::rand::{SecureRandom, SystemRandom};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::code::Code;
use crate::frame::{MAX_SEALED, MAX_TICKET, Role, SPAKE_LEN};

/// The fixed label both identities and the transcript start with.
pub const LABEL: &[u8] = b"sluice short code v1";

/// The nonce length of the sealed ticket.
pub const NONCE_LEN: usize = 12;

/// The tag length of ChaCha20-Poly1305.
pub const TAG_LEN: usize = 16;

/// Why the exchange, the confirmation or the seal failed. No variant holds a code, word, key or
/// ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// The peer's SPAKE2 message was not 33 bytes or did not complete the group operation.
    #[error("the peer's SPAKE2 message is not a valid one")]
    Spake,
    /// HKDF could not derive a key.
    #[error("the key schedule failed")]
    Hkdf,
    /// The AEAD refused to seal or open.
    #[error("the sealed ticket did not open")]
    Aead,
    /// The system had no random bytes for a nonce.
    #[error("the system has no random bytes")]
    Rng,
    /// The peer's confirmation tag was not the one this side derived.
    #[error("the code's confirmation did not match")]
    WrongTag,
    /// A fixed shared key for a vector was not 32 bytes.
    #[error("the shared key is not 32 bytes")]
    KeyLength,
    /// A ticket text was longer than 4096 bytes.
    #[error("the ticket is longer than a meeting place allows")]
    TicketTooLong,
    /// The opened bytes were not UTF-8.
    #[error("the opened ticket is not text")]
    TicketNotUtf8,
    /// A sealed value was outside its nonce-plus-tag bounds.
    #[error("the sealed value is not the right length")]
    SealedLength,
}

/// One side's SPAKE2 run. The state is consumed by [`Exchange::finish`], so a peer cannot finish
/// twice or reuse the random draw.
pub struct Exchange {
    role: Role,
    nameplate: u32,
    outbound: [u8; SPAKE_LEN],
    state: Option<Spake2<Ed25519Group>>,
}

impl fmt::Debug for Exchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Exchange")
            .field("role", &self.role)
            .field("nameplate", &self.nameplate)
            .field("state", &format_args!("<redacted>"))
            .finish()
    }
}

impl Exchange {
    /// Starts SPAKE2 for `role` on `code`'s words, binding the label and the nameplate into both
    /// identities. The sender is side A and the receiver side B.
    pub fn new(role: Role, code: &Code) -> Self {
        let nameplate = code.nameplate();
        let password = code.password();
        let sender = Identity::new(format!("{}/nameplate/{nameplate}/sender", label()).as_bytes());
        let receiver =
            Identity::new(format!("{}/nameplate/{nameplate}/receiver", label()).as_bytes());
        let (state, outbound) = match role {
            Role::Sender => Spake2::<Ed25519Group>::start_a(
                &Password::new(password.as_bytes()),
                &sender,
                &receiver,
            ),
            Role::Receiver => Spake2::<Ed25519Group>::start_b(
                &Password::new(password.as_bytes()),
                &sender,
                &receiver,
            ),
        };
        // `spake2` 0.4.0's start functions return one 33-byte message for this group; the wire
        // format depends on it, so a change in the dependency is a bug here and not a frame to
        // accept.
        let outbound: [u8; SPAKE_LEN] = outbound
            .try_into()
            .expect("spake2 0.4.0's Ed25519 message is 33 bytes");
        Self {
            role,
            nameplate,
            outbound,
            state: Some(state),
        }
    }

    /// This side's 33-byte message.
    pub fn outbound(&self) -> &[u8; SPAKE_LEN] {
        &self.outbound
    }

    /// The role this side plays.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The nameplate both identities and the transcript bind.
    pub fn nameplate(&self) -> u32 {
        self.nameplate
    }

    /// Consumes the state and finishes against the peer's 33-byte message. The resulting keys are
    /// derived from the shared key and the transcript, not from the messages alone.
    pub fn finish(self, peer: &[u8]) -> Result<Keys, CryptoError> {
        if peer.len() != SPAKE_LEN {
            return Err(CryptoError::Spake);
        }
        let state = self.state.ok_or(CryptoError::Spake)?;
        let shared = state.finish(peer).map_err(|_| CryptoError::Spake)?;
        let (first, second) = match self.role {
            Role::Sender => (&self.outbound[..], peer),
            Role::Receiver => (peer, &self.outbound[..]),
        };
        let transcript = transcript(self.nameplate, first, second);
        Keys::from_shared(&shared, &transcript)
    }
}

/// The three 32-byte values HKDF derives, and the transcript the AEAD authenticates.
pub struct Keys {
    transcript: Vec<u8>,
    confirm_sender: Zeroizing<[u8; 32]>,
    confirm_receiver: Zeroizing<[u8; 32]>,
    ticket: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keys")
            .field("transcript", &self.transcript.len())
            .field("values", &format_args!("<redacted>"))
            .finish()
    }
}

impl Keys {
    /// Derives the keys from a fixed shared key and transcript. The SPAKE2 caller passes the key
    /// `finish` returned; a vector passes the published one, so a third implementation can check the
    /// schedule without reproducing a random draw.
    pub fn from_shared(shared: &[u8], transcript: &[u8]) -> Result<Self, CryptoError> {
        if shared.len() != 32 {
            return Err(CryptoError::KeyLength);
        }
        let salt = Salt::new(hkdf::HKDF_SHA256, transcript);
        let prk = salt.extract(shared);
        Ok(Self {
            transcript: transcript.to_vec(),
            confirm_sender: Zeroizing::new(expand(&prk, b"confirm sender")?),
            confirm_receiver: Zeroizing::new(expand(&prk, b"confirm receiver")?),
            ticket: Zeroizing::new(expand(&prk, b"ticket")?),
        })
    }

    /// The transcript both sides bind as associated data.
    pub fn transcript(&self) -> &[u8] {
        &self.transcript
    }

    /// The AEAD key, for a key-schedule vector. It is a copy and is wiped on drop; a caller outside
    /// the test vectors has no use for it.
    pub fn ticket_key(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.ticket)
    }

    /// The confirmation tag `role` sends.
    pub fn confirmation(&self, role: Role) -> [u8; 32] {
        match role {
            Role::Sender => *self.confirm_sender,
            Role::Receiver => *self.confirm_receiver,
        }
    }

    /// Checks the peer's tag for `role` in constant time.
    pub fn verify(&self, role: Role, tag: &[u8; 32]) -> Result<(), CryptoError> {
        let expected = self.confirmation(role);
        if bool::from(expected.ct_eq(tag)) {
            Ok(())
        } else {
            Err(CryptoError::WrongTag)
        }
    }

    /// Seals a ticket under a fresh random 12-byte nonce: `nonce || ciphertext || tag`.
    pub fn seal(&self, ticket: &str) -> Result<Vec<u8>, CryptoError> {
        if ticket.len() > MAX_TICKET {
            return Err(CryptoError::TicketTooLong);
        }
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| CryptoError::Rng)?;
        self.seal_with_nonce(&nonce, ticket)
    }

    /// Seals under a caller's nonce, for a test vector. Production uses [`Keys::seal`], which draws
    /// a fresh nonce; the caller of this function owns the nonce's uniqueness.
    pub fn seal_with_nonce(
        &self,
        nonce: &[u8; NONCE_LEN],
        ticket: &str,
    ) -> Result<Vec<u8>, CryptoError> {
        if ticket.len() > MAX_TICKET {
            return Err(CryptoError::TicketTooLong);
        }
        let key = self.aead_key()?;
        let mut out = ticket.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(*nonce),
            Aad::from(&self.transcript),
            &mut out,
        )
        .map_err(|_| CryptoError::Aead)?;
        let mut sealed = Vec::with_capacity(NONCE_LEN + out.len());
        sealed.extend_from_slice(nonce);
        sealed.extend_from_slice(&out);
        Ok(sealed)
    }

    /// Opens a sealed ticket and checks that the plaintext is UTF-8 and a ticket-sized text.
    pub fn open(&self, sealed: &[u8]) -> Result<Zeroizing<String>, CryptoError> {
        if !(NONCE_LEN + TAG_LEN..=MAX_SEALED).contains(&sealed.len()) {
            return Err(CryptoError::SealedLength);
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| CryptoError::SealedLength)?;
        let key = self.aead_key()?;
        let mut out = ciphertext.to_vec();
        let plain = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&self.transcript),
                &mut out,
            )
            .map_err(|_| CryptoError::Aead)?;
        if plain.len() > MAX_TICKET {
            return Err(CryptoError::TicketTooLong);
        }
        let text = std::str::from_utf8(plain).map_err(|_| CryptoError::TicketNotUtf8)?;
        Ok(Zeroizing::new(text.to_string()))
    }

    fn aead_key(&self) -> Result<LessSafeKey, CryptoError> {
        let key = UnboundKey::new(&CHACHA20_POLY1305, self.ticket.as_ref())
            .map_err(|_| CryptoError::Aead)?;
        Ok(LessSafeKey::new(key))
    }
}

/// The transcript of `SPEC.md` section 4.1.3:
/// `"sluice short code v1" || nameplate:u32be || len(A):u32be || A || len(B):u32be || B`.
pub fn transcript(nameplate: u32, first: &[u8], second: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LABEL.len() + 12 + first.len() + second.len());
    out.extend_from_slice(LABEL);
    out.extend_from_slice(&nameplate.to_be_bytes());
    out.extend_from_slice(&(first.len() as u32).to_be_bytes());
    out.extend_from_slice(first);
    out.extend_from_slice(&(second.len() as u32).to_be_bytes());
    out.extend_from_slice(second);
    out
}

fn label() -> &'static str {
    std::str::from_utf8(LABEL).unwrap_or("sluice short code v1")
}

struct Key32;

impl KeyType for Key32 {
    fn len(&self) -> usize {
        32
    }
}

fn expand(prk: &hkdf::Prk, info: &[u8]) -> Result<[u8; 32], CryptoError> {
    let info = [info];
    let okm = prk.expand(&info, Key32).map_err(|_| CryptoError::Hkdf)?;
    let mut out = [0u8; 32];
    okm.fill(&mut out).map_err(|_| CryptoError::Hkdf)?;
    Ok(out)
}

#[cfg(test)]
mod tests;
