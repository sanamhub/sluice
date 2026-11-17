//! SPAKE2 over the two words, key confirmation, and the sealed ticket.
//!
//! The nameplate and a fixed label are bound into both SPAKE2 identities. The SPAKE2 key is run
//! through HKDF-SHA256 with the transcript as salt; two confirmation tags and one AEAD key come out.

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use subtle::ConstantTimeEq;
use ring::hkdf::{self, KeyType, Salt};
use ring::rand::{SecureRandom, SystemRandom};
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::code::Code;
use crate::wire::Role;

pub const LABEL: &[u8] = b"sluice short code v1";

#[derive(Debug)]
pub enum CryptoError {
    Spake,
    Hkdf,
    Aead,
    Rng,
    WrongTag,
}

pub struct Started {
    pub role: Role,
    pub plate: u32,
    pub outbound: Vec<u8>,
    state: Option<Spake2<Ed25519Group>>,
}

impl Started {
    pub fn new(role: Role, code: &Code) -> Self {
        let plate = code.nameplate;
        let password = Password::new(code.password().as_bytes());
        let sender = Identity::new(format!("sluice short code v1/nameplate/{plate}/sender").as_bytes());
        let receiver = Identity::new(format!("sluice short code v1/nameplate/{plate}/receiver").as_bytes());
        let (state, outbound) = match role {
            Role::Sender => Spake2::<Ed25519Group>::start_a(&password, &sender, &receiver),
            Role::Receiver => Spake2::<Ed25519Group>::start_b(&password, &sender, &receiver),
        };
        Self {
            role,
            plate,
            outbound,
            state: Some(state),
        }
    }

    /// Consumes the SPAKE2 state and returns the keys. `peer` is the other side's SPAKE2 message.
    pub fn finish(mut self, peer: &[u8]) -> Result<Keys, CryptoError> {
        let state = self.state.take().ok_or(CryptoError::Spake)?;
        let shared = state.finish(peer).map_err(|_| CryptoError::Spake)?;
        let (a, b) = match self.role {
            Role::Sender => (&self.outbound[..], peer),
            Role::Receiver => (peer, &self.outbound[..]),
        };
        let transcript = transcript(self.plate, a, b);
        Keys::derive(&shared, transcript)
    }
}

#[derive(Clone)]
pub struct Keys {
    pub transcript: Vec<u8>,
    confirm_sender: [u8; 32],
    confirm_receiver: [u8; 32],
    aead: [u8; 32],
}

impl Keys {
    fn derive(shared: &[u8], transcript: Vec<u8>) -> Result<Self, CryptoError> {
        let salt = Salt::new(hkdf::HKDF_SHA256, &transcript);
        let prk = salt.extract(shared);
        Ok(Self {
            confirm_sender: expand(&prk, b"confirm sender")?,
            confirm_receiver: expand(&prk, b"confirm receiver")?,
            aead: expand(&prk, b"ticket")?,
            transcript,
        })
    }

    pub fn confirmation(&self, role: Role) -> [u8; 32] {
        match role {
            Role::Sender => self.confirm_sender,
            Role::Receiver => self.confirm_receiver,
        }
    }

    /// Constant-time check of the tag the other side sent for its role.
    pub fn verify(&self, role: Role, tag: &[u8; 32]) -> Result<(), CryptoError> {
        let expected = self.confirmation(role);
        if expected.ct_eq(tag).into() { Ok(()) } else { Err(CryptoError::WrongTag) }
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut nonce = [0u8; 12];
        SystemRandom::new().fill(&mut nonce).map_err(|_| CryptoError::Rng)?;
        let key = LessSafeKey::new(
            UnboundKey::new(&aead::CHACHA20_POLY1305, &self.aead).map_err(|_| CryptoError::Aead)?,
        );
        let mut out = plaintext.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(&self.transcript),
            &mut out,
        )
        .map_err(|_| CryptoError::Aead)?;
        let mut sealed = Vec::with_capacity(12 + out.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&out);
        Ok(sealed)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if sealed.len() < 12 + 16 {
            return Err(CryptoError::Aead);
        }
        let (nonce, ciphertext) = sealed.split_at(12);
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| CryptoError::Aead)?;
        let key = LessSafeKey::new(
            UnboundKey::new(&aead::CHACHA20_POLY1305, &self.aead).map_err(|_| CryptoError::Aead)?,
        );
        let mut out = ciphertext.to_vec();
        let plain = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&self.transcript),
                &mut out,
            )
            .map_err(|_| CryptoError::Aead)?;
        Ok(plain.to_vec())
    }
}

fn transcript(plate: u32, a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LABEL.len() + 4 + 8 + a.len() + b.len());
    out.extend_from_slice(LABEL);
    out.extend_from_slice(&plate.to_be_bytes());
    out.extend_from_slice(&(a.len() as u32).to_be_bytes());
    out.extend_from_slice(a);
    out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    out.extend_from_slice(b);
    out
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
mod tests {
    use super::*;

    fn code(text: &str) -> Code {
        Code::parse(text).unwrap()
    }

    #[test]
    fn both_sides_derive_the_same_keys_and_tags() {
        let c = code("7-orbit-velvet");
        let a = Started::new(Role::Sender, &c);
        let b = Started::new(Role::Receiver, &c);
        let (a_msg, b_msg) = (a.outbound.clone(), b.outbound.clone());
        let a_keys = a.finish(&b_msg).unwrap();
        let b_keys = b.finish(&a_msg).unwrap();
        assert_eq!(a_keys.confirm_sender, b_keys.confirm_sender);
        assert_eq!(a_keys.confirm_receiver, b_keys.confirm_receiver);
        assert!(b_keys.verify(Role::Sender, &a_keys.confirmation(Role::Sender)).is_ok());
        assert!(a_keys.verify(Role::Receiver, &b_keys.confirmation(Role::Receiver)).is_ok());
    }

    #[test]
    fn a_different_word_pair_gives_a_different_key() {
        let a = Started::new(Role::Sender, &code("7-orbit-velvet"));
        let b = Started::new(Role::Receiver, &code("7-orbit-vendor"));
        let (a_msg, b_msg) = (a.outbound.clone(), b.outbound.clone());
        let a_keys = a.finish(&b_msg).unwrap();
        let b_keys = b.finish(&a_msg).unwrap();
        assert!(a_keys.verify(Role::Receiver, &b_keys.confirmation(Role::Receiver)).is_err());
    }

    #[test]
    fn the_ticket_seals_and_opens() {
        let c = code("7-orbit-velvet");
        let a = Started::new(Role::Sender, &c);
        let b = Started::new(Role::Receiver, &c);
        let (a_msg, b_msg) = (a.outbound.clone(), b.outbound.clone());
        let a_keys = a.finish(&b_msg).unwrap();
        let b_keys = b.finish(&a_msg).unwrap();
        let ticket = b"sluice1abcdefghjkmnpqrstvwxyz";
        let sealed = a_keys.seal(ticket).unwrap();
        assert_eq!(b_keys.open(&sealed).unwrap(), ticket);
        // A different transcript (a different nameplate) must not open it.
        let other = Started::new(Role::Sender, &code("8-orbit-velvet"));
        let other_r = Started::new(Role::Receiver, &code("8-orbit-velvet"));
        let (om, orm) = (other.outbound.clone(), other_r.outbound.clone());
        let other_keys = other.finish(&orm).unwrap();
        let _ = om;
        // The keys differ, so the tag check fails before any open.
        assert!(other_keys.open(&sealed).is_err());
    }
}
