//! 128-bit content hashing (SipHash-1-3, fixed key). Not cryptographic — it identifies files
//! for duplicate detection and keys caches; collisions are astronomically unlikely by accident.

use std::fmt;
use std::hash::Hasher;

use siphasher::sip128::{Hasher128 as _, SipHasher13};

/// A 128-bit hash, shown as 32 hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash128(pub u128);

impl fmt::Display for Hash128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl Hash128 {
    pub fn parse(s: &str) -> Option<Hash128> {
        (s.len() == 32).then(|| u128::from_str_radix(s, 16).ok().map(Hash128)).flatten()
    }
}

/// Streaming hasher.
#[derive(Clone, Debug)]
pub struct Hasher128(SipHasher13);

impl Default for Hasher128 {
    fn default() -> Self {
        Hasher128(SipHasher13::new_with_keys(0x4c69_6768_7443_7261, 0x6674_2043_6f6e_7465))
    }
}

impl Hasher128 {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn update(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.write(bytes);
        self
    }
    /// Length-prefixed string (so `("ab","c")` and `("a","bc")` differ).
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.0.write(&(s.len() as u64).to_le_bytes());
        self.0.write(s.as_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.write(&v.to_le_bytes());
        self
    }
    pub fn finish(&self) -> Hash128 {
        Hash128(self.0.finish128().as_u128())
    }
}

/// Hash of a whole byte string.
pub fn hash_bytes(bytes: &[u8]) -> Hash128 {
    Hasher128::new().update(bytes).finish()
}
