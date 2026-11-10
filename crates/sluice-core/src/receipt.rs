//! The receipt of `SPEC.md` section 10: a sender's signed statement that it offered content.
//!
//! A receipt names no stream, so its signed message is the label, a `0x00` byte and the first 80
//! bytes, where a live record's is the label, the byte, a stream id and the body. The two labels
//! differ, so neither signature is good for the other.
//!
//! A receipt is not a secret the way a ticket is, but it does name the sender's key and the
//! content's root, so the errors here never quote the text they refused, for the same reason a
//! ticket's do not.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::Error;
use crate::ticket::{base32, key};

/// The text every receipt starts with. A decoder ignores its case.
const PREFIX: &str = "sluicereceipt1";
/// The bytes the signature covers: root, total length, sender key and time.
const SIGNED: usize = 80;
/// The whole receipt: the signed bytes and the signature.
const LEN: usize = SIGNED + 64;
/// The label the signature covers, which is what keeps it from being good for a live record.
const LABEL: &[u8] = b"sluice/receipt/1";
/// The longest receipt text, in bytes, that is decoded. A receipt is 144 bytes, which is 231
/// characters of base32 and the prefix.
const MAX_TEXT_BYTES: usize = 256;

/// `Error::Policy` for a receipt text that breaks a rule. The reason never holds a part of it.
fn refuse(reason: &str) -> Error {
    Error::Policy(format!("receipt {reason}"))
}

/// A sender's statement that it offered the content with this root and length, signed at this time.
///
/// # Examples
///
/// ```
/// use ed25519_dalek::SigningKey;
/// use sluice_core::receipt::Receipt;
///
/// let key = SigningKey::from_bytes(&[7; 32]);
/// let receipt = Receipt::sign(&key, [9; 32], 1024, 1_800_000_000_000);
/// let text = receipt.encode();
/// assert!(text.starts_with("sluicereceipt1"));
/// let read = Receipt::decode(&text)?;
/// assert_eq!(read.total_len(), 1024);
/// assert_eq!(read, receipt);
/// read.verify()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    root: [u8; 32],
    total_len: u64,
    sender_key: [u8; 32],
    utc_ms: u64,
    sig: [u8; 64],
}

impl Receipt {
    /// Signs a statement that the content with `root` and `total_len` bytes was offered at `utc_ms`
    /// (UTC milliseconds since the Unix epoch) by `key`'s endpoint.
    pub fn sign(key: &SigningKey, root: [u8; 32], total_len: u64, utc_ms: u64) -> Self {
        let mut receipt = Self {
            root,
            total_len,
            sender_key: key.verifying_key().to_bytes(),
            utc_ms,
            sig: [0; 64],
        };
        let signed = receipt.signed();
        receipt.sig = key.sign(&message(&signed)).to_bytes();
        receipt
    }

    /// The BLAKE3 root of the content the receipt names.
    pub fn root(&self) -> &[u8; 32] {
        &self.root
    }

    /// The length in bytes of the content the receipt names.
    pub fn total_len(&self) -> u64 {
        self.total_len
    }

    /// The endpoint id that signed it.
    pub fn sender_key(&self) -> &[u8; 32] {
        &self.sender_key
    }

    /// When it was signed, in UTC milliseconds since the Unix epoch.
    pub fn utc_ms(&self) -> u64 {
        self.utc_ms
    }

    /// The text form: the prefix, then the 144 bytes in the ticket's base32, lower case.
    ///
    /// One receipt has one text, so this is the form to compare, to store and to write to a file.
    pub fn encode(&self) -> String {
        format!("{PREFIX}{}", base32::encode(&self.to_bytes()))
    }

    /// Reads a receipt from text a receiver was given. One line ending, `\n` or `\r\n`, is allowed
    /// and nothing else is forgiven: a wrong prefix, a character outside the base32 alphabet, a
    /// length other than 144 bytes, unused bits that are not zero, or text over 256 bytes all fail
    /// with [`Error::Policy`], and the message names the rule and never the text.
    pub fn decode(text: &str) -> Result<Self, Error> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(refuse(&format!(
                "is {} bytes of text, and a receipt is at most {MAX_TEXT_BYTES}",
                text.len()
            )));
        }
        let text = text.strip_suffix('\n').unwrap_or(text);
        let text = text.strip_suffix('\r').unwrap_or(text);
        let Some(head) = text.get(..PREFIX.len()) else {
            return Err(refuse("does not start with its prefix"));
        };
        if !head.eq_ignore_ascii_case(PREFIX) {
            return Err(refuse("does not start with its prefix"));
        }
        let bytes =
            base32::decode_bytes(&text[PREFIX.len()..]).map_err(|reason| refuse(&reason))?;
        if bytes.len() != LEN {
            return Err(refuse(&format!(
                "is {} bytes long, and a receipt is {LEN}",
                bytes.len()
            )));
        }
        let mut root = [0; 32];
        let mut total_len = [0; 8];
        let mut sender_key = [0; 32];
        let mut utc_ms = [0; 8];
        let mut sig = [0; 64];
        root.copy_from_slice(&bytes[..32]);
        total_len.copy_from_slice(&bytes[32..40]);
        sender_key.copy_from_slice(&bytes[40..72]);
        utc_ms.copy_from_slice(&bytes[72..80]);
        sig.copy_from_slice(&bytes[80..]);
        Ok(Self {
            root,
            total_len: u64::from_le_bytes(total_len),
            sender_key,
            utc_ms: u64::from_le_bytes(utc_ms),
            sig,
        })
    }

    /// Checks the signature strictly under the key the receipt names, and that the key is one a
    /// ticket may name: in its one encoding and not of small order, the rule of section 4.
    ///
    /// This is what makes a receipt worth keeping. It says nothing about whether the content was
    /// received or is still held: only the sender's offer is signed.
    pub fn verify(&self) -> Result<(), Error> {
        if !key::is_valid(&self.sender_key) {
            return Err(Error::Verify(
                "the receipt's sender_key is not a canonical Ed25519 key of large order"
                    .to_string(),
            ));
        }
        let key = VerifyingKey::from_bytes(&self.sender_key)
            .map_err(|_| Error::Verify("the receipt's sender_key is not a key".to_string()))?;
        key.verify_strict(&message(&self.signed()), &Signature::from_bytes(&self.sig))
            .map_err(|_| {
                Error::Verify(
                    "the receipt's signature does not verify under the key it names".to_string(),
                )
            })
    }

    /// The 144 bytes, in the order of `SPEC.md` section 10.
    fn to_bytes(&self) -> [u8; LEN] {
        let mut out = [0; LEN];
        out[..32].copy_from_slice(&self.root);
        out[32..40].copy_from_slice(&self.total_len.to_le_bytes());
        out[40..72].copy_from_slice(&self.sender_key);
        out[72..80].copy_from_slice(&self.utc_ms.to_le_bytes());
        out[80..].copy_from_slice(&self.sig);
        out
    }

    /// The first 80 bytes, which are the part the signature covers.
    fn signed(&self) -> [u8; SIGNED] {
        let mut out = [0; SIGNED];
        out[..32].copy_from_slice(&self.root);
        out[32..40].copy_from_slice(&self.total_len.to_le_bytes());
        out[40..72].copy_from_slice(&self.sender_key);
        out[72..80].copy_from_slice(&self.utc_ms.to_le_bytes());
        out
    }
}

/// The message a receipt's signature covers: the label, one `0x00` byte, and the signed bytes.
fn message(signed: &[u8; SIGNED]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LABEL.len() + 1 + SIGNED);
    out.extend_from_slice(LABEL);
    out.push(0);
    out.extend_from_slice(signed);
    out
}

#[cfg(test)]
mod tests;
