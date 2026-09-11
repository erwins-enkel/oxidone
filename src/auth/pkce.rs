//! PKCE (RFC 7636) and the `state` nonce, for one interactive consent.
//!
//! Both exist for the paste path. That redirect lands on the loopback of the
//! machine the *browser* is on — where oxidone is not listening and anything
//! else might be — so `state` is what lets the flow reject a callback that is
//! not the answer to this request, and PKCE is what makes a code intercepted
//! there useless without the verifier, which never leaves this process.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::api::ApiError;

/// Bytes behind each nonce. 32 of them is 256 bits, and base64url-encodes to 43
/// characters — the shortest `code_verifier` RFC 7636 permits, and well inside
/// its 43..=128 range.
const NONCE_BYTES: usize = 32;

/// The per-consent secrets: the verifier kept here, the challenge and `state`
/// sent to Google.
pub(super) struct Challenge {
    /// Sent only in the token exchange, never in the authorization URL.
    pub(super) verifier: String,
    /// `code_challenge`: base64url-unpadded SHA-256 of the verifier.
    pub(super) challenge: String,
    /// Echoed back on the callback, and the only thing that ties a pasted URL
    /// to this attempt.
    pub(super) state: String,
}

impl Challenge {
    /// A fresh verifier, its S256 challenge, and a fresh `state`.
    ///
    /// New for every consent rather than per provider: a verifier reused across
    /// attempts is a verifier an earlier attempt could have leaked.
    pub(super) fn new() -> Result<Self, ApiError> {
        let verifier = nonce()?;
        let challenge = s256(&verifier);
        Ok(Self {
            challenge,
            verifier,
            state: nonce()?,
        })
    }
}

/// [`NONCE_BYTES`] of system randomness, base64url-unpadded.
///
/// A failure here is the machine refusing entropy, which is neither Google's
/// fault nor a dead grant — so it keeps [`ApiError::ConsentFailed`] rather than
/// borrowing `Network`'s "try again shortly", which would be wrong advice.
fn nonce() -> Result<String, ApiError> {
    let mut bytes = [0u8; NONCE_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| ApiError::ConsentFailed(format!("the system refused randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// RFC 7636 `S256`: base64url-unpadded SHA-256 over the verifier's ASCII bytes.
fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B, the one published vector for `S256`. It pins the
    /// whole spelling at once — SHA-256 over ASCII, base64**url**, no padding —
    /// any one of which Google would answer with an opaque `invalid_grant`.
    #[test]
    fn the_rfc_7636_vector_round_trips() {
        assert_eq!(
            s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_verifier_is_long_enough_for_the_rfc_and_url_safe() {
        let challenge = Challenge::new().expect("system randomness");
        assert_eq!(challenge.verifier.len(), 43);
        assert!(challenge
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(challenge.challenge, s256(&challenge.verifier));
    }

    /// The regression this guards: a `state` derived from the verifier, or a
    /// nonce generated once and cached, would make two consents echo each other
    /// — and `state` would then stop distinguishing attempts, which is the only
    /// job it has.
    #[test]
    fn two_challenges_share_nothing() {
        let first = Challenge::new().expect("system randomness");
        let second = Challenge::new().expect("system randomness");
        assert_ne!(first.verifier, second.verifier);
        assert_ne!(first.state, second.state);
        assert_ne!(first.state, first.verifier);
    }
}
