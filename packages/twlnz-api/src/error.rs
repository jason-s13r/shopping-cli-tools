//! What The Warehouse said no with.
//!
//! Two cases are particular to this storefront. A `verify` token is minted into
//! a page and expires, so a write can fail for a reason that has nothing to do
//! with the account -- and the fix is to fetch the page again, not to sign in.
//! And most responses are HTML, so "the markup moved" is a real failure mode
//! that has to be told apart from "the site said no".

use net_kit::{AuthFault, Fault, HttpError};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Http(#[from] HttpError),

    #[error(transparent)]
    Net(#[from] net_kit::Error),

    /// The site asked this client to slow down.
    ///
    /// Worth its own variant rather than an anonymous 429: it is the one
    /// failure where the right response is to stop, not to retry harder, and a
    /// person seeing "HTTP 429" often does the opposite.
    #[error("The Warehouse is rate-limiting this client{}", match .retry_after {
        Some(secs) => format!("; it asked for {secs}s before the next request"),
        None => String::new(),
    })]
    RateLimited { retry_after: Option<u64> },

    #[error("the Warehouse session has expired")]
    SessionExpired,

    #[error("not signed in to The Warehouse")]
    NotSignedIn,

    /// A `verify` token was refused. They are minted into a page with a
    /// timestamp, so this means the page was read too long ago -- fetch it
    /// again and the new token will work.
    #[error("the page token for {action} has expired")]
    TokenExpired { action: &'static str },

    /// The page parsed, but the thing being looked for was not in it. Named
    /// separately from a decode failure because the fix is different: this is
    /// the site's markup having moved.
    #[error("{what} was not found in the page{detail}")]
    NotInPage { what: String, detail: String },

    #[error("no product called {0}")]
    NoSuchProduct(String),

    #[error("no store called {0}")]
    NoSuchStore(String),

    /// The storefront answered an action with `error: true` and a message of
    /// its own, which is usually worth passing through verbatim.
    #[error("{action} was refused: {message}")]
    Refused {
        action: &'static str,
        message: String,
    },

    #[error("the sign-in page refused the {step}{detail}")]
    LoginRefused { step: &'static str, detail: String },

    #[error("signing in did not produce a session{detail}")]
    NoSession { detail: String },

    #[error("{0}")]
    Shape(String),

    #[error("{context}")]
    Decode {
        context: String,
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    pub fn decode(context: impl Into<String>, source: serde_json::Error) -> Error {
        Error::Decode {
            context: context.into(),
            source,
        }
    }

    pub fn not_in_page(what: impl Into<String>) -> Error {
        Error::NotInPage {
            what: what.into(),
            detail: String::new(),
        }
    }

    pub fn body(&self) -> &str {
        match self {
            Error::Http(e) => e.body(),
            _ => "",
        }
    }

    /// Whether the site asked this client to back off.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Error::RateLimited { .. })
    }

    /// Whether a client holding a password should try signing in again.
    pub fn is_lapsed(&self) -> bool {
        matches!(self, Error::SessionExpired | Error::NotSignedIn)
    }

    /// Whether re-fetching the page this call came from would fix it. Unlike a
    /// lapsed session this costs one request and no credentials, so the client
    /// retries it without being asked.
    pub fn is_stale_token(&self) -> bool {
        matches!(self, Error::TokenExpired { .. })
    }
}

impl Fault for Error {
    fn auth(&self) -> Option<AuthFault> {
        match self {
            Error::SessionExpired => Some(AuthFault::Expired),
            Error::NotSignedIn => Some(AuthFault::Missing),
            Error::LoginRefused { .. } => Some(AuthFault::Rejected),
            Error::Http(e) => e.auth(),
            _ => None,
        }
    }

    fn is_transport(&self) -> bool {
        matches!(self, Error::Http(e) if e.is_transport())
    }
}

/// Turn a transport failure into this crate's own, naming a rate limit rather
/// than leaving it as an anonymous 429.
///
/// `retry_after` is read off the response by the caller: `net_kit` surfaces
/// the status and the body, not the headers.
pub(crate) fn from_http(e: HttpError, retry_after: Option<u64>) -> Error {
    match e.status() {
        Some(429) => Error::RateLimited { retry_after },
        _ => Error::Http(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_limit_is_named_rather_than_left_as_a_status_code() {
        // The one failure where retrying harder is the wrong move, so it must
        // not look like every other transport error.
        let limited = Error::RateLimited {
            retry_after: Some(30),
        };
        assert!(limited.is_rate_limited());
        assert!(limited.to_string().contains("rate-limiting"), "{limited}");
        assert!(limited.to_string().contains("30s"), "{limited}");
        assert!(!limited.is_lapsed());
        assert_eq!(limited.auth(), None);
    }

    #[test]
    fn an_expired_page_token_is_not_an_expired_session() {
        // The distinction is the whole point: one is fixed by re-fetching a
        // page, the other by signing in, and confusing them would send someone
        // to re-enter a password that was never the problem.
        let stale = Error::TokenExpired { action: "cart add" };
        assert!(stale.is_stale_token());
        assert!(!stale.is_lapsed());
        assert_eq!(stale.auth(), None);

        assert!(Error::SessionExpired.is_lapsed());
        assert!(!Error::SessionExpired.is_stale_token());
    }

    #[test]
    fn auth_kinds_are_distinct() {
        assert_eq!(Error::SessionExpired.auth(), Some(AuthFault::Expired));
        assert_eq!(Error::NotSignedIn.auth(), Some(AuthFault::Missing));
        assert_eq!(Error::Shape("x".into()).auth(), None);
    }
}
