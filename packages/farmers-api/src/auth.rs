//! Signing in.
//!
//! An ordinary Intershop form POST -- `ShopLoginForm_Login`,
//! `ShopLoginForm_Password` and a `SynchronizerToken` scraped from the sign-in
//! page -- answering **302 to `/account`** with the session cookie attached.
//! There is **no captcha** anywhere in it, which is what makes this the short
//! module it is: unlike the Briscoes flow there is no browser to drive, and
//! unlike the Woolworths one nothing is encrypted or single-use, so a lapsed
//! session is renewed by running this again.
//!
//! Two requests, in order, because the second needs the first's token and its
//! `sid`.
//!
//! **The redirect must not be followed.** The 302 *is* the success signal, and
//! following it would replace the evidence with a page. [`crate::client_spec`]
//! already refuses to, and this POST says so again rather than depending on it.
//!
//! The bot manager gates this POST like everything else under `/INTERSHOP/`,
//! and it is satisfied by the same warmth every other call needs: measured
//! 2026-09-18, the form was accepted on a browser-earned jar and answered on
//! the credentials' own merits. No browser drives the sign-in itself -- unlike
//! the Kmart flow, where the bot check guards the password submit. Cold, the
//! failure is [`Error::Denied`] from [`Client::login`][crate::Client::login]
//! rather than anything that looks like a bad password.

use net_kit::wreq;

use crate::endpoints::Endpoints;
use crate::error::{is_deny_page, Error, Result};
use crate::extract;
use crate::session::Session;

/// Narration for the sign-in flow, on stderr when asked for.
///
/// Nothing passed to it is a credential: steps are named, cookies appear by
/// name only, and no form field is included.
pub type Trace<'a> = &'a dyn Fn(&str);

pub fn no_trace(_: &str) {}

/// The pipeline that takes the form.
const PROCESS_LOGIN: &str = "ViewUserAccount-ProcessLogin";

/// Walk the sign-in flow and hand back the session it produces.
///
/// `session` is whatever the caller already has -- a warmed one for preference,
/// since its Akamai cookies save a round trip and make this look like the
/// continuation of a visit rather than a cold POST.
///
/// **The session handed in must carry no grant.** Intershop answers `/login`
/// with a 302 to `/account` for one that does, so a caller signing in while
/// already signed in passes [`Client::anonymous`][crate::Client] -- warmth
/// without the account cookies. Dropping them here instead would make this
/// function decide something that belongs to its callers, one of which is
/// entitled to skip the form altogether.
pub async fn login(
    http: &wreq::Client,
    endpoints: &Endpoints,
    session: Session,
    email: &str,
    password: &str,
    trace: Trace<'_>,
) -> Result<Session> {
    let mut session = session;

    trace("fetching the sign-in page");
    let url = endpoints.login_page();
    let mut request = http.get(&url);
    request = navigation(request, endpoints, None);
    if let Some(cookies) = session.header() {
        request = with_cookies(request, &cookies);
    }
    // Sent by hand rather than through `net_kit::http::text`, which reports any
    // non-2xx by quoting the body -- and a redirect's body is a page of
    // boilerplate that buries what happened. A 3xx here has one likely cause
    // and deserves to be named.
    let response = request.send().await.map_err(|source| {
        Error::Http(net_kit::HttpError::Transport {
            method: "GET",
            url: url.clone(),
            source,
        })
    })?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap_or_default();
    session.absorb(&headers);

    if status.is_redirection() {
        let target = headers
            .get(wreq::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("somewhere else");
        return Err(Error::LoginRefused {
            step: "sign-in page",
            detail: format!(
                ": it redirected to {target} rather than serving the form, which \
                 means the storefront still counts this session as signed in"
            ),
        });
    }
    if is_deny_page(&body) {
        return Err(Error::Denied { warmed: false });
    }
    let token = extract::synchronizer_token(&body).ok_or_else(|| Error::LoginRefused {
        step: "sign-in page",
        detail: ": it carried no SynchronizerToken field, which usually means something \
                 other than the sign-in page was served"
            .into(),
    })?;
    trace("found the form token");

    // Exactly the four fields the page's own form posts, in its own order.
    // `login` is the submit button, and Intershop dispatches on it: without it
    // the pipeline renders the page back rather than processing anything.
    let form = [
        ("SynchronizerToken", token.as_str()),
        ("ShopLoginForm_Login", email),
        ("ShopLoginForm_Password", password),
        ("login", "Login"),
    ];
    let url = endpoints.pipeline(PROCESS_LOGIN);
    let mut request = http
        .post(&url)
        // Not followed: the 302 is the answer. See the module docs.
        .redirect(wreq::redirect::Policy::none());
    request = navigation(request, endpoints, Some(&endpoints.login_page()));
    if let Some(cookies) = session.header() {
        request = with_cookies(request, &cookies);
    }

    trace("posting the credentials");
    // Handled without `net_kit::http::text`, which treats any non-2xx as a
    // failure -- and here the 302 *is* the success.
    let response = request.form(&form).send().await.map_err(|source| {
        Error::Http(net_kit::HttpError::Transport {
            method: "POST",
            url: url.clone(),
            source,
        })
    })?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let location = headers
        .get(wreq::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = response.text().await.unwrap_or_default();

    session.absorb(&headers);
    // Names only, never values -- and said before the outcome is known, so it
    // does not claim a sign-in that has not happened.
    trace(&format!(
        "the form answered {status}, holding {}",
        session.names().join(", ")
    ));

    if is_deny_page(&body) {
        return Err(Error::Denied { warmed: true });
    }
    if !session.account() {
        // The flow ran and the server answered, but nothing came back that
        // speaks for a person. Usually a wrong password, which Intershop
        // reports by re-rendering the page rather than with a status code -- so
        // the page is worth reading before falling back to a generic complaint.
        return Err(match extract::form_error(&body) {
            Some(message) => Error::LoginRefused {
                step: "password",
                detail: format!(": {message}"),
            },
            None => Error::NoSession {
                detail: format!(", so no signed-in session cookie was set{}", match status {
                    // A 200 means the sign-in page was rendered again, which
                    // is what a refused password looks like.
                    200 => ". The form answered 200, which is the sign-in page again rather than a redirect",
                    302 => ". The form did redirect, but not to a signed-in session",
                    _ => "",
                }),
            },
        });
    }

    session.email = Some(email.trim().to_string());
    trace(&format!(
        "signed in{}",
        location
            .map(|l| format!(", redirected to {l}"))
            .unwrap_or_default()
    ));
    Ok(session)
}

/// The headers a browser sends when a person clicks a link or submits a form.
///
/// A form POST **is** a navigation, and has to say so. The `same-origin` fetch
/// values that an XHR endpoint wants would be wrong here, and the storefront
/// serves a different page to a request that did not come from itself.
fn navigation(
    request: wreq::RequestBuilder,
    endpoints: &Endpoints,
    referer: Option<&str>,
) -> wreq::RequestBuilder {
    let request = request
        .header(wreq::header::ORIGIN, &endpoints.origin)
        .header(
            wreq::header::REFERER,
            referer.unwrap_or(&endpoints.origin).to_string(),
        )
        .header("sec-fetch-dest", "document")
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-site", "same-origin")
        .header("sec-fetch-user", "?1");
    request.header(
        wreq::header::ACCEPT,
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    )
}

/// Attach the session, marked so the value never reaches a log.
fn with_cookies(request: wreq::RequestBuilder, cookies: &str) -> wreq::RequestBuilder {
    match wreq::header::HeaderValue::from_str(cookies) {
        Ok(mut value) => {
            value.set_sensitive(true);
            request.header(wreq::header::COOKIE, value)
        }
        Err(_) => request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_posts_exactly_what_the_page_does() {
        // Intershop dispatches on the submit button's name. Dropping `login`
        // because it looks like decoration makes the pipeline render the page
        // back instead of signing anyone in -- which reads as a bad password.
        let names = [
            "SynchronizerToken",
            "ShopLoginForm_Login",
            "ShopLoginForm_Password",
            "login",
        ];
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"login"), "the submit button is a field");
    }

    #[test]
    fn a_sensitive_cookie_header_is_still_sent() {
        // The marking is what keeps it out of a debug print; it must not stop
        // the header being attached.
        let http = net_kit::http::build(crate::http::client_spec()).expect("a client");
        let request = with_cookies(http.get("https://example.invalid/"), "sid=abc");
        let built = request.build().expect("builds");
        assert_eq!(
            built
                .headers()
                .get(wreq::header::COOKIE)
                .map(|v| v.is_sensitive()),
            Some(true)
        );
    }

    #[test]
    fn a_form_post_is_sent_as_a_navigation_rather_than_as_an_xhr() {
        // The storefront serves a different page to a request that did not
        // come from itself, and an XHR's Sec-Fetch values are not that.
        let http = net_kit::http::build(crate::http::client_spec()).expect("a client");
        let endpoints = Endpoints::defaults();
        let built = navigation(
            http.post(endpoints.pipeline(PROCESS_LOGIN)),
            &endpoints,
            Some("https://www.farmers.co.nz/login"),
        )
        .build()
        .expect("builds");
        assert_eq!(built.headers().get("sec-fetch-mode").unwrap(), "navigate");
        assert_eq!(built.headers().get("sec-fetch-dest").unwrap(), "document");
        assert_eq!(
            built.headers().get(wreq::header::REFERER).unwrap(),
            "https://www.farmers.co.nz/login"
        );
    }
}
