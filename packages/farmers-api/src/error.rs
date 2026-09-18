//! What Farmers said no with.
//!
//! One failure here is unlike anything in the other storefront crates and
//! shapes the whole client: **Akamai denies with a 200.** The body is a
//! `WAF_Deny_Page`, the status is success, and every JSON decode against it
//! fails with something that reads like a schema change. [`Error::Denied`]
//! exists so that is diagnosed once, by looking at the body, instead of
//! surfacing as a decode error at whichever call happened to be first.
//!
//! It has **four** shapes, and they are one problem. `200` with a
//! `WAF_Deny_Page` body from the REST API, `403 Access Denied` from a `ViewX-`
//! pipeline, `429 {"cpr_chlge":"true"}` -- a demand that the client solve a
//! JavaScript proof-of-work -- and the interstitial challenge: a `200` whose
//! body is the sensor widget and which carries none of the deny markers. That
//! last one is the one to know about, because it is the only refusal that can
//! pass for a successful fetch of an empty page. All four arrive as
//! [`Error::Denied`] or [`Error::Challenged`]. None is about credentials, and
//! the `429` is not about rate.
//!
//! **What the gate reads is where the `_abck` came from.** One this crate
//! fetched for itself is refused whatever profile it wears, and one a browser
//! earned is accepted from any of them -- measured, see [`crate::http`]. So the
//! remedy for all four shapes is a fresh browser warm-up rather than a wait, a
//! retry or a different profile. One re-warm and one retry is the whole retry
//! policy: no number of attempts re-earns a cookie.
//! Requests are still kept few for their own sake, which is why nothing fans
//! out and [`crate::Client::stock`] walks thirteen regions in sequence.

use net_kit::{AuthFault, Fault, HttpError};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Http(#[from] HttpError),

    #[error(transparent)]
    Net(#[from] net_kit::Error),

    /// Akamai Bot Manager served its deny page.
    ///
    /// Carried as its own variant because it arrives as **HTTP 200** with an
    /// HTML body, so nothing about the status says anything is wrong. The fix
    /// is a fresh warm-up, which [`crate::Client`] does once on its own before
    /// this ever reaches a caller.
    #[error("Farmers' bot protection refused this request{}", match .warmed {
        true => ", and it was refused again after a fresh warm-up",
        false => "",
    })]
    Denied { warmed: bool },

    /// Akamai demanded a JavaScript proof-of-work.
    ///
    /// `429`, with `{"cpr_chlge":"true"}` for a body. **Not a rate limit**,
    /// despite the status: the same client is challenged on its first request
    /// of the day and stays challenged however long it waits, so backing off
    /// does nothing. It is the bot manager saying "you look like a browser, so
    /// behave like one" -- and the only thing that satisfies it is running the
    /// sensor script, which this crate does not do. See [`crate::http`].
    #[error("Farmers' bot protection demanded a browser challenge this client cannot answer")]
    Challenged,

    /// The session was never warmed and a REST call was made anyway.
    ///
    /// Only reachable by a caller driving the transport by hand;
    /// [`crate::Client`] warms itself.
    #[error("this client has not been warmed against the Farmers home page yet")]
    NotWarmed,

    #[error("the sign-in page refused the {step}{detail}")]
    LoginRefused { step: &'static str, detail: String },

    #[error("signing in did not produce a session{detail}")]
    NoSession { detail: String },

    #[error("the Farmers session has expired")]
    SessionExpired,

    #[error("not signed in to Farmers")]
    NotSignedIn,

    /// The page parsed, but what was being looked for was not in it. Named
    /// apart from a decode failure: this is the site's markup having moved,
    /// not malformed input.
    #[error("{what} was not found in the page{detail}")]
    NotInPage { what: String, detail: String },

    #[error("no product called {0}")]
    NoSuchProduct(String),

    #[error("no category called {0}")]
    NoSuchCategory(String),

    #[error("no region called {0}")]
    NoSuchRegion(String),

    /// The answer parsed but did not contain what was asked for. Named apart
    /// from a decode failure: the schema moved, the JSON was not malformed.
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

    /// Whether the bot manager refused this, at any point and in any of its
    /// three shapes.
    pub fn is_denied(&self) -> bool {
        matches!(self, Error::Denied { .. } | Error::Challenged)
    }

    /// Whether a caller holding a password should try signing in again.
    pub fn is_lapsed(&self) -> bool {
        matches!(self, Error::SessionExpired | Error::NotSignedIn)
    }

    /// Whether the only way forward is a fresh sign-in with a password.
    pub fn needs_login(&self) -> bool {
        matches!(self, Error::LoginRefused { .. } | Error::NoSession { .. })
    }
}

/// Reclassify a transport failure that is really the bot manager.
///
/// `/INTERSHOP/web/` denies with a `403` where the REST API denies with a
/// `200`, and `429 {"cpr_chlge":"true"}` is a third shape again: a demand that
/// the client solve a JavaScript proof-of-work. None of the three is about
/// credentials or about rate, and all three are fixed the same way -- warm
/// again -- so they arrive here as one variant.
pub fn from_http(e: net_kit::HttpError) -> Error {
    match e.status() {
        Some(403) if is_deny_page(e.body()) => Error::Denied { warmed: false },
        // Not RateLimited, despite the code. The body is the tell: an ordinary
        // 429 has no `cpr_chlge` in it, and reading this one as a rate limit
        // would have a caller sleep through something sleeping will not fix.
        Some(429) if e.body().contains("cpr_chlge") => Error::Challenged,
        _ => Error::Http(e),
    }
}

impl Fault for Error {
    fn auth(&self) -> Option<AuthFault> {
        match self {
            Error::SessionExpired => Some(AuthFault::Expired),
            Error::NotSignedIn => Some(AuthFault::Missing),
            Error::LoginRefused { .. } => Some(AuthFault::Rejected),
            // Deliberately not Forbidden: nothing was rejected on its
            // credentials, so a caller must not read this as "sign in again".
            Error::Denied { .. } | Error::Challenged => None,
            Error::Http(e) => e.auth(),
            _ => None,
        }
    }

    fn is_transport(&self) -> bool {
        matches!(self, Error::Http(e) if e.is_transport())
    }
}

/// The marker Akamai's deny page carries.
///
/// Matched on the body rather than the status because the status is 200. The
/// page is a support template: it names the reference number a shopper would
/// quote, and this string is the stable part of it.
const DENY_MARKERS: [&str; 3] = ["WAF_Deny_Page", "Access Denied", "Reference&#32;&#35;18"];

/// The markers Akamai's *interstitial challenge* carries.
///
/// A fourth refusal shape, and the most dangerous of them: **200, and the body
/// is neither the deny page nor what was asked for.** It is the sensor
/// interstitial -- a container for the proof-of-work widget plus the script
/// that runs it -- and it carries none of [`DENY_MARKERS`], so a client
/// checking only those reads it as a successful fetch of an empty page.
/// Measured 2026-09-18: `/orders` answered this and the order history was
/// reported as "no orders" rather than as a refusal.
const CHALLENGE_MARKERS: [&str; 3] = [
    "sec-if-cpt-container",
    "sec-bc-tile-container",
    "scf-akamai-logo",
];

/// Whether a body is the bot manager's refusal rather than an answer.
///
/// Cheap enough to run on every response, and it has to be: any of them can be
/// this instead of what was asked for.
///
/// Read the body, never the status. The same page arrives as a `200` from the
/// REST API and as a `403` from a `ViewX-` pipeline, so a status test would
/// catch one and miss the other.
/// Whether this body is Akamai's interstitial challenge rather than an answer.
///
/// Checked before [`is_deny_page`] wherever both are read, because this one is
/// the shape that otherwise passes for success.
pub fn is_challenge_page(body: &str) -> bool {
    let head = &body[..body.len().min(4096)];
    CHALLENGE_MARKERS.iter().any(|m| head.contains(m))
}

pub fn is_deny_page(body: &str) -> bool {
    // Bounded so a large legitimate document is not scanned end to end. The
    // deny page is about five kilobytes in total and says so in its first.
    let head = &body[..body.len().min(4096)];
    DENY_MARKERS.iter().any(|m| head.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deny_page_is_recognised_by_its_body_because_its_status_is_success() {
        // The whole reason this function exists: nothing in the status line
        // distinguishes this from the JSON that was asked for.
        assert!(is_deny_page(
            "<HTML><HEAD><TITLE>Access Denied</TITLE></HEAD><BODY>WAF_Deny_Page</BODY></HTML>"
        ));
        assert!(!is_deny_page(r#"{"sku":"6867065002","name":"a robe"}"#));
    }

    #[test]
    fn a_product_that_merely_mentions_access_is_not_a_deny_page() {
        // Matching anywhere in a long body would misread a catalogue listing
        // as a refusal, and the client retries on a refusal.
        let body = format!(
            r#"{{{}"name":"Access Denied"}}"#,
            r#""pad":"xxxxxxxxxxxxxxxx","#.repeat(300)
        );
        assert!(body.len() > 4096, "the fixture has to overflow the window");
        assert!(!is_deny_page(&body), "the marker is past the first 4KB");
    }

    #[test]
    fn a_multibyte_body_shorter_than_the_window_is_not_sliced_mid_character() {
        // The window is a byte index, so a body whose length is under it must
        // take the whole string rather than a truncated slice.
        assert!(!is_deny_page("piñata ✨"));
    }

    #[test]
    fn a_denial_is_not_reported_as_an_authentication_problem() {
        // It is the one failure that looks like a wall but is not about
        // credentials; exiting through the auth path would send someone to
        // retype a password that was never wrong.
        let e = Error::Denied { warmed: true };
        assert!(e.is_denied());
        assert_eq!(e.auth(), None);
        assert!(!e.is_lapsed());
        assert!(!e.needs_login());
        assert!(e.to_string().contains("after a fresh warm-up"), "{e}");
    }

    #[test]
    fn a_first_denial_does_not_claim_a_warm_up_was_tried() {
        let e = Error::Denied { warmed: false };
        assert!(!e.to_string().contains("warm-up"), "{e}");
    }

    #[test]
    fn being_signed_out_and_being_refused_a_password_are_different_questions() {
        assert!(Error::NotSignedIn.is_lapsed());
        assert!(!Error::NotSignedIn.needs_login());
        assert!(Error::LoginRefused {
            step: "password",
            detail: String::new()
        }
        .needs_login());
        assert_eq!(Error::NotSignedIn.auth(), Some(AuthFault::Missing));
    }
}
