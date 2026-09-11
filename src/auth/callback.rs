//! Reading an authorization code out of whatever came back from Google.
//!
//! Two things can deliver it — the loopback listener's redirect, and a URL the
//! user pasted because that redirect could not reach this machine — and they
//! deliver the *same* thing: a callback URL. One parser therefore serves both,
//! so a pasted callback cannot be accepted on terms the redirected one is not.
//!
//! Every failure here is retryable by design: the flow reports it and keeps
//! waiting. Only the token exchange settles a consent.

/// The base a bare `/?code=…` target — what the listener reads off the request
/// line — is resolved against. Never sent anywhere; it exists so that one
/// parser can take both an absolute pasted URL and a relative request target.
const RELATIVE_BASE: &str = "http://127.0.0.1";

/// Why a callback yielded no code.
///
/// The variants differ in what the user has to *do*, which is the whole reason
/// they are apart: paste something else, authorize again, or start over.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum CallbackError {
    #[error("that is not a URL — paste the whole address from your browser's address bar")]
    NotAUrl,
    #[error("that URL carried no authorization code — paste the whole address, query and all")]
    NoCode,
    #[error("google refused the authorization ({0}); authorize again to retry")]
    Refused(String),
    #[error("that URL answers a different authorization attempt; use the one this prompt opened")]
    StateMismatch,
}

/// The authorization code in `input`, if it is this attempt's callback.
///
/// `input` may be an absolute URL (pasted from the address bar) or the bare
/// `/?code=…` target the loopback listener reads. Surrounding whitespace is
/// trimmed — a pasted line almost always carries some.
///
/// `state` is checked **before** anything else is believed, `error` included: a
/// URL that does not answer this request has nothing to tell us, not even a
/// refusal.
pub(super) fn parse_callback(input: &str, expected_state: &str) -> Result<String, CallbackError> {
    let input = input.trim();
    let parsed = if input.starts_with('/') {
        reqwest::Url::parse(RELATIVE_BASE)
            .expect("RELATIVE_BASE is a literal URL")
            .join(input)
            .map_err(|_| CallbackError::NotAUrl)?
    } else {
        reqwest::Url::parse(input).map_err(|_| CallbackError::NotAUrl)?
    };

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }

    if state.as_deref() != Some(expected_state) {
        return Err(CallbackError::StateMismatch);
    }
    if let Some(error) = error {
        return Err(CallbackError::Refused(error));
    }
    // An empty `code=` is as useless as an absent one, and Google's token
    // endpoint would answer it with an opaque `invalid_grant`.
    code.filter(|code| !code.is_empty())
        .ok_or(CallbackError::NoCode)
}

/// The request target of a `GET /… HTTP/1.1` line, or `None` for anything else.
///
/// Only `GET` — the redirect is one, and refusing the rest means a stray probe
/// on the loopback port cannot be mistaken for a callback.
pub(super) fn request_target(line: &str) -> Option<&str> {
    let mut parts = line.trim_end().split(' ');
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if method != "GET" || !version.starts_with("HTTP/") || !target.starts_with('/') {
        return None;
    }
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "s-t-a-t-e";

    #[test]
    fn a_pasted_address_bar_url_yields_its_code() {
        assert_eq!(
            parse_callback(
                "  http://127.0.0.1:37137/?code=4%2F0AX4abc&scope=tasks&state=s-t-a-t-e\n",
                STATE
            ),
            Ok("4/0AX4abc".to_string())
        );
    }

    /// What the listener hands over: no scheme, no host, just the target off the
    /// request line. The same parser has to take it, or the redirect and the
    /// paste would be accepted on different terms.
    #[test]
    fn a_bare_request_target_yields_its_code() {
        assert_eq!(
            parse_callback("/?code=4/0AX4abc&state=s-t-a-t-e", STATE),
            Ok("4/0AX4abc".to_string())
        );
    }

    #[test]
    fn a_declined_consent_is_refused_not_empty() {
        assert_eq!(
            parse_callback("/?error=access_denied&state=s-t-a-t-e", STATE),
            Err(CallbackError::Refused("access_denied".to_string()))
        );
    }

    /// A callback from an earlier attempt, or from something else listening on
    /// that port, answers a different question — and is not believed even when
    /// it carries a perfectly good-looking code.
    #[test]
    fn a_foreign_callback_is_rejected_before_its_contents_are_read() {
        assert_eq!(
            parse_callback("/?code=4/0AX4abc&state=someone-else", STATE),
            Err(CallbackError::StateMismatch)
        );
        assert_eq!(
            parse_callback("/?code=4/0AX4abc", STATE),
            Err(CallbackError::StateMismatch)
        );
        assert_eq!(
            parse_callback("/?error=access_denied&state=someone-else", STATE),
            Err(CallbackError::StateMismatch)
        );
    }

    #[test]
    fn a_url_without_a_code_is_not_a_callback() {
        assert_eq!(
            parse_callback("/?state=s-t-a-t-e", STATE),
            Err(CallbackError::NoCode)
        );
        assert_eq!(
            parse_callback("/?code=&state=s-t-a-t-e", STATE),
            Err(CallbackError::NoCode)
        );
    }

    #[test]
    fn something_that_is_not_a_url_says_so() {
        assert_eq!(
            parse_callback("4/0AX4abc", STATE),
            Err(CallbackError::NotAUrl)
        );
        assert_eq!(parse_callback("", STATE), Err(CallbackError::NotAUrl));
    }

    #[test]
    fn a_get_request_line_yields_its_target() {
        assert_eq!(
            request_target("GET /?code=abc&state=xyz HTTP/1.1\r\n"),
            Some("/?code=abc&state=xyz")
        );
    }

    /// The regression this guards: a browser asking for `/favicon.ico`, or a
    /// port scanner speaking something else entirely, being read as a callback
    /// and answered with "that URL carried no authorization code".
    #[test]
    fn anything_that_is_not_a_get_line_is_not_a_target() {
        assert_eq!(request_target("POST /?code=abc HTTP/1.1"), None);
        assert_eq!(request_target("GET ?code=abc HTTP/1.1"), None);
        assert_eq!(request_target("GET /"), None);
        assert_eq!(request_target(""), None);
    }
}
