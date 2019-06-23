// Copyright 2015-2016 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY
// SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
// OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
// CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

//! HMAC is specified in [RFC 2104].
//!
//! After a `Key` is constructed, it can be used for multiple signing or
//! verification operations. Separating the construction of the key from the
//! rest of the HMAC operation allows the per-key precomputation to be done
//! only once, instead of it being done in every HMAC operation.
//!
//! Frequently all the data to be signed in a message is available in a single
//! contiguous piece. In that case, the module-level `sign` function can be
//! used. Otherwise, if the input is in multiple parts, `Context` should be
//! used.
//!
//! # Examples:
//!
//! ## Signing a value and verifying it wasn't tampered with
//!
//! ```
//! use ring::{digest, hmac, rand};
//!
//! let rng = rand::SystemRandom::new();
//! let key = hmac::Key::generate(&digest::SHA256, &rng)?;
//!
//! let msg = "hello, world";
//!
//! let tag = hmac::sign(&key, msg.as_bytes());
//!
//! // [We give access to the message to an untrusted party, and they give it
//! // back to us. We need to verify they didn't tamper with it.]
//!
//! hmac::verify(&key, msg.as_bytes(), tag.as_ref())?;
//!
//! # Ok::<(), ring::error::Unspecified>(())
//! ```
//!
//! ## Using the one-shot API:
//!
//! ```
//! use ring::{digest, hmac, rand};
//! use ring::rand::SecureRandom;
//!
//! let msg = "hello, world";
//!
//! // The sender generates a secure key value and signs the message with it.
//! // Note that in a real protocol, a key agreement protocol would be used to
//! // derive `key_value`.
//! let mut key_value = [0u8; 32];
//! let rng = rand::SystemRandom::new();
//! rng.fill(&mut key_value)?;
//!
//! let s_key = hmac::Key::new(&digest::SHA256, key_value.as_ref());
//! let tag = hmac::sign(&s_key, msg.as_bytes());
//!
//! // The receiver (somehow!) knows the key value, and uses it to verify the
//! // integrity of the message.
//! let v_key = hmac::Key::new(&digest::SHA256, key_value.as_ref());
//! hmac::verify(&v_key, msg.as_bytes(), tag.as_ref())?;
//!
//! # Ok::<(), ring::error::Unspecified>(())
//! ```
//!
//! ## Using the multi-part API:
//! ```
//! use ring::{digest, hmac, rand};
//! use ring::rand::SecureRandom;
//!
//! let parts = ["hello", ", ", "world"];
//!
//! // The sender generates a secure key value and signs the message with it.
//! // Note that in a real protocol, a key agreement protocol would be used to
//! // derive `key_value`.
//! let mut key_value = [0u8; 48];
//! let rng = rand::SystemRandom::new();
//! rng.fill(&mut key_value)?;
//!
//! let s_key = hmac::Key::new(&digest::SHA384, key_value.as_ref());
//! let mut s_ctx = hmac::Context::with_key(&s_key);
//! for part in &parts {
//!     s_ctx.update(part.as_bytes());
//! }
//! let tag = s_ctx.sign();
//!
//! // The receiver (somehow!) knows the key value, and uses it to verify the
//! // integrity of the message.
//! let v_key = hmac::Key::new(&digest::SHA384, key_value.as_ref());
//! let mut msg = Vec::<u8>::new();
//! for part in &parts {
//!     msg.extend(part.as_bytes());
//! }
//! hmac::verify(&v_key, &msg.as_ref(), tag.as_ref())?;
//!
//! # Ok::<(), ring::error::Unspecified>(())
//! ```
//!
//! [RFC 2104]: https://tools.ietf.org/html/rfc2104
//! [code for `ring::pbkdf2`]:
//!     https://github.com/briansmith/ring/blob/master/src/pbkdf2.rs
//! [code for `ring::hkdf`]:
//!     https://github.com/briansmith/ring/blob/master/src/hkdf.rs

use crate::{constant_time, digest, error, hkdf};

/// A deprecated alias for `Tag`.
#[deprecated(note = "`Signature` was renamed to `Tag`. This alias will be removed soon.")]
pub type Signature = Tag;

/// An HMAC tag.
///
/// For a given tag `t`, use `t.as_ref()` to get the tag value as a byte slice.
#[derive(Clone, Copy, Debug)]
pub struct Tag(digest::Digest);

impl AsRef<[u8]> for Tag {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

/// A key to use for HMAC signing.
#[derive(Clone)]
pub struct Key {
    ctx_prototype: Context,
}

/// `hmac::SigningKey` was renamed to `hmac::Key`.
#[deprecated(note = "Renamed to `hmac::Key`.")]
pub type SigningKey = Key;

/// `hmac::VerificationKey` was merged into `hmac::Key`.
#[deprecated(
    note = "The distinction between verification & signing keys was removed. Use `hmac::Key`."
)]
pub type VerificationKey = Key;

impl core::fmt::Debug for Key {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> Result<(), core::fmt::Error> {
        f.debug_struct("Key")
            .field("algorithm", self.digest_algorithm())
            .finish()
    }
}

impl Key {
    /// Derive an HMAC key from the output of an `HKDF-Expand` operation.
    ///
    /// The key will be `digest_alg.output_len` bytes long, based on the
    /// recommendation in https://tools.ietf.org/html/rfc2104#section-3.
    pub fn derive(digest_alg: &'static digest::Algorithm, okm: hkdf::Okm) -> Self {
        let mut key_bytes = [0; digest::MAX_OUTPUT_LEN];
        let key_bytes = &mut key_bytes[..digest_alg.output_len];
        okm.fill(key_bytes).unwrap();
        Self::new(digest_alg, key_bytes)
    }

    /// Generate an HMAC signing key using the given digest algorithm with a
    /// random value generated from `rng`.
    ///
    /// The key will be `digest_alg.output_len` bytes long, based on the
    /// recommendation in https://tools.ietf.org/html/rfc2104#section-3.
    #[cfg(feature = "rand")]
    pub fn generate(
        digest_alg: &'static digest::Algorithm,
        rng: &dyn crate::rand::SecureRandom,
    ) -> Result<Self, error::Unspecified> {
        let mut key_bytes = [0; digest::MAX_OUTPUT_LEN];
        let key_bytes = &mut key_bytes[..digest_alg.output_len];
        rng.fill(key_bytes)?;
        Ok(Self::new(digest_alg, key_bytes))
    }

    /// Construct an HMAC signing key using the given digest algorithm and key
    /// value.
    ///
    /// `key_value` should be a value generated using a secure random number
    /// generator (e.g. the `key_value` output by
    /// `SealingKey::generate_serializable()`) or derived from a random key by
    /// a key derivation function (e.g. `ring::hkdf`). In particular,
    /// `key_value` shouldn't be a password.
    ///
    /// As specified in RFC 2104, if `key_value` is shorter than the digest
    /// algorithm's block length (as returned by `digest::Algorithm::block_len`,
    /// not the digest length returned by `digest::Algorithm::output_len`) then
    /// it will be padded with zeros. Similarly, if it is longer than the block
    /// length then it will be compressed using the digest algorithm.
    ///
    /// You should not use keys larger than the `digest_alg.block_len` because
    /// the truncation described above reduces their strength to only
    /// `digest_alg.output_len * 8` bits. Support for such keys is likely to be
    /// removed in a future version of *ring*.
    pub fn new(digest_alg: &'static digest::Algorithm, key_value: &[u8]) -> Self {
        let mut key = Self {
            ctx_prototype: Context {
                inner: digest::Context::new(digest_alg),
                outer: digest::Context::new(digest_alg),
            },
        };

        let key_hash;
        let key_value = if key_value.len() <= digest_alg.block_len {
            key_value
        } else {
            key_hash = digest::digest(digest_alg, key_value);
            key_hash.as_ref()
        };

        const IPAD: u8 = 0x36;

        let mut padded_key = [IPAD; digest::MAX_BLOCK_LEN];
        let padded_key = &mut padded_key[..digest_alg.block_len];

        // If the key is shorter than one block then we're supposed to act like
        // it is padded with zero bytes up to the block length. `x ^ 0 == x` so
        // we can just leave the trailing bytes of `padded_key` untouched.
        for (padded_key, key_value) in padded_key.iter_mut().zip(key_value.iter()) {
            *padded_key ^= *key_value;
        }
        key.ctx_prototype.inner.update(&padded_key);

        const OPAD: u8 = 0x5C;

        // Remove the `IPAD` masking, leaving the unmasked padded key, then
        // mask with `OPAD`, all in one step.
        for b in padded_key.iter_mut() {
            *b ^= IPAD ^ OPAD;
        }
        key.ctx_prototype.outer.update(&padded_key);

        key
    }

    /// The digest algorithm for the key.
    pub fn digest_algorithm(&self) -> &'static digest::Algorithm {
        self.ctx_prototype.inner.algorithm()
    }
}

/// A context for multi-step (Init-Update-Finish) HMAC signing.
///
/// Use `sign` for single-step HMAC signing.
#[derive(Clone)]
pub struct Context {
    inner: digest::Context,
    outer: digest::Context,
}

/// `hmac::SigningContext` was renamed to `hmac::Context`.
#[deprecated(note = "Renamed to `hmac::Context`.")]
pub type SigningContext = Context;

impl core::fmt::Debug for Context {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> Result<(), core::fmt::Error> {
        f.debug_struct("Context")
            .field("algorithm", self.inner.algorithm())
            .finish()
    }
}

impl Context {
    /// Constructs a new HMAC signing context using the given digest algorithm
    /// and key.
    pub fn with_key(signing_key: &Key) -> Self {
        signing_key.ctx_prototype.clone()
    }

    /// Updates the HMAC with all the data in `data`. `update` may be called
    /// zero or more times until `finish` is called.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finalizes the HMAC calculation and returns the HMAC value. `sign`
    /// consumes the context so it cannot be (mis-)used after `sign` has been
    /// called.
    ///
    /// It is generally not safe to implement HMAC verification by comparing
    /// the return value of `sign` to a tag. Use `verify` for verification
    /// instead.
    pub fn sign(mut self) -> Tag {
        self.outer.update(self.inner.finish().as_ref());
        Tag(self.outer.finish())
    }
}

/// Calculates the HMAC of `data` using the key `key` in one step.
///
/// Use `Context` to calculate HMACs where the input is in multiple parts.
///
/// It is generally not safe to implement HMAC verification by comparing the
/// return value of `sign` to a tag. Use `verify` for verification instead.
pub fn sign(key: &Key, data: &[u8]) -> Tag {
    let mut ctx = Context::with_key(key);
    ctx.update(data);
    ctx.sign()
}

/// Calculates the HMAC of `data` using the signing key `key`, and verifies
/// whether the resultant value equals `tag`, in one step.
///
/// This is logically equivalent to, but more efficient than, constructing a
/// `Key` with the same value as `key` and then using `verify`.
///
/// The verification will be done in constant time to prevent timing attacks.
pub fn verify(key: &Key, data: &[u8], tag: &[u8]) -> Result<(), error::Unspecified> {
    constant_time::verify_slices_are_equal(sign(key, data).as_ref(), tag)
}

#[cfg(test)]
mod tests {
    use crate::{digest, hmac};

    // Make sure that `Key::generate` and `verify_with_own_key` aren't
    // completely wacky.
    #[test]
    #[cfg(feature = "rand")]
    pub fn hmac_signing_key_coverage() {
        let mut rng = crate::rand::SystemRandom::new();

        const HELLO_WORLD_GOOD: &[u8] = b"hello, world";
        const HELLO_WORLD_BAD: &[u8] = b"hello, worle";

        for d in &digest::test_util::ALL_ALGORITHMS {
            let key = hmac::Key::generate(d, &mut rng).unwrap();
            let tag = hmac::sign(&key, HELLO_WORLD_GOOD);
            assert!(hmac::verify(&key, HELLO_WORLD_GOOD, tag.as_ref()).is_ok());
            assert!(hmac::verify(&key, HELLO_WORLD_BAD, tag.as_ref()).is_err())
        }
    }
}
