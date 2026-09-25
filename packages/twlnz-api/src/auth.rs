//! Signing in.
//!
//! An ordinary form POST -- `dwfrm_login_email`, `dwfrm_login_password` and a
//! CSRF token scraped from the login page -- answering **302 with the session
//! cookies attached**. That is worth noting because it is the opposite of the
//! Woolworths flow: nothing here is encrypted or unrepeatable, so a lapsed
//! session can be renewed by running this again, and the only thing that has to
//! be kept is the password.
//!
//! Two requests, in order, because the second needs the first's token and its
//! `dwsid`: fetch the login page, then post to it.
//!
//! **The redirect must not be followed.** Every cookie that matters is set on
//! the 302 itself, and this client keeps no cookie jar -- so a followed
//! redirect drops all of them on the floor and lands on `/account` as a guest.
//! The failure is quiet and reads like a rejected password, which is why the
//! POST overrides the client's redirect policy rather than relying on it.

use std::collections::BTreeMap;

use net_kit::wreq;

use crate::endpoints::Endpoints;
use crate::error::{Error, Refusal, Result};
use crate::session::{Session, REFRESH_COOKIE};

/// Narration for the login flow, on stderr when asked for.
///
/// Nothing passed to it is a credential: steps are named, cookies appear by
/// name only, and no query string is included.
pub type Trace<'a> = &'a dyn Fn(&str);

pub fn no_trace(_: &str) {}

/// The CSRF field the login form posts.
const CSRF_FIELD: &str = "csrf_token";

/// Walk the login flow and hand back the session it produces.
///
/// Paced but not retried: a throttled sign-in is reported, not re-submitted.
///
/// `clearance` is Cloudflare's cookies, which the login page is challenged
/// without; see [`crate::Warmer`]. Nothing else is carried in -- an expired
/// account cookie sent with the form reads as a wrong password.
pub async fn login(
    http: &wreq::Client,
    pacer: &net_kit::Pacer,
    endpoints: &Endpoints,
    clearance: BTreeMap<String, String>,
    email: &str,
    password: &str,
    trace: Trace<'_>,
) -> Result<Session> {
    let mut session = Session::default();
    session.adopt_clearance(clearance);

    trace("fetching the login page");
    let url = endpoints.login_page();
    let mut req = http.get(&url);
    if let Some(cookies) = session.header() {
        if let Ok(mut value) = wreq::header::HeaderValue::from_str(&cookies) {
            value.set_sensitive(true);
            req = req.header(wreq::header::COOKIE, value);
        }
    }
    pacer.wait().await;
    let sent = req.send().await;
    let refusal = Refusal::of(&sent);
    let (headers, body) = net_kit::http::text("GET", &url, sent)
        .await
        .map_err(|e| crate::error::from_http(e, refusal))?;
    session.absorb(&headers);

    let token = csrf_token(&body).ok_or_else(|| Error::LoginRefused {
        step: "login page",
        detail: format!(
            ": it carried no {CSRF_FIELD} field, which usually means a bot check was served instead"
        ),
    })?;
    trace("found the form token");

    // `rurl=1` is what the site's own form posts; without it the controller
    // has no redirect target to build.
    let url = format!("{}?rurl=1", endpoints.login());
    let form = [
        ("dwfrm_login_email", email),
        ("dwfrm_login_password", password),
        (CSRF_FIELD, &token),
    ];
    let mut req = http
        .post(&url)
        // Not followed: the cookies are on the 302. See the module docs.
        .redirect(wreq::redirect::Policy::none())
        .header(wreq::header::ORIGIN, &endpoints.origin)
        .header(wreq::header::REFERER, endpoints.login_page())
        // A form submission is a navigation, and these have to say so -- the
        // same-origin `fetch` values that the action endpoints require would be
        // wrong here.
        .header("sec-fetch-dest", "document")
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-site", "same-origin")
        .header("sec-fetch-user", "?1");
    if let Some(cookies) = session.header() {
        if let Ok(mut value) = wreq::header::HeaderValue::from_str(&cookies) {
            value.set_sensitive(true);
            req = req.header(wreq::header::COOKIE, value);
        }
    }

    trace("posting the credentials");
    pacer.wait().await;
    // Handled without `net_kit::http::text`, which treats any non-2xx as a
    // failure -- and here the 302 *is* the success.
    let response = req.form(&form).send().await.map_err(|source| {
        Error::Http(net_kit::HttpError::Transport {
            method: "POST",
            url: url.clone(),
            source,
        })
    })?;
    if crate::error::is_challenge(&response) {
        return Err(Error::Challenged);
    }
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap_or_default();

    if status == 401 || status == 403 {
        return Err(Error::LoginRefused {
            step: "password",
            detail: String::new(),
        });
    }
    session.absorb(&headers);
    // Names only, never values -- and said before the outcome is known, so it
    // does not claim a sign-in that has not happened.
    trace(&format!(
        "the form answered {status}, holding {}",
        session
            .cookies()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if !session.account() {
        // The flow ran and the server answered, but nothing came back that
        // speaks for a person. Usually a wrong password, which this site
        // reports in the page rather than with a status code -- so the page is
        // worth reading before falling back to a generic complaint.
        return Err(match form_error(&body) {
            Some(message) => Error::LoginRefused {
                step: "password",
                detail: format!(": {message}"),
            },
            None => Error::NoSession {
                detail: format!(
                    ", so no {REFRESH_COOKIE} cookie was set. The form answered {status}\
                     {}",
                    match status {
                        // A 200 means the login page was re-rendered, which is
                        // what a refused password looks like.
                        200 => ", which is the sign-in page again rather than a redirect",
                        _ => "",
                    }
                ),
            },
        });
    }
    trace("signed in");
    Ok(session)
}

/// The form's CSRF token, from the hidden input the login page renders.
pub fn csrf_token(html: &str) -> Option<String> {
    let doc = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse(&format!(r#"input[name="{CSRF_FIELD}"]"#)).ok()?;
    doc.select(&selector)
        .find_map(|i| i.attr("value"))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// What the site said was wrong, from the alert it renders back into the page.
fn form_error(html: &str) -> Option<String> {
    let doc = scraper::Html::parse_document(html);
    let selector =
        scraper::Selector::parse("div.alert-danger, .login-error-message, .invalid-feedback")
            .ok()?;
    doc.select(&selector)
        .map(|e| e.text().collect::<String>().trim().to_string())
        .find(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_token_is_read_off_the_login_page() {
        let html = r#"<form action="/account/submit-login">
            <input type="hidden" name="csrf_token" value="a-long-opaque-token" />
            <input name="dwfrm_login_email" /></form>"#;
        assert_eq!(csrf_token(html).as_deref(), Some("a-long-opaque-token"));
    }

    #[test]
    fn a_page_with_no_token_is_reported_rather_than_posted_to_blind() {
        // A bot check served in place of the login page looks exactly like
        // this, and posting credentials into it would be worse than failing.
        assert_eq!(
            csrf_token("<html><body>Checking your browser</body></html>"),
            None
        );
        assert_eq!(csrf_token(r#"<input name="csrf_token" value="" />"#), None);
    }

    #[test]
    fn the_sites_own_words_for_a_bad_password_are_kept() {
        let html = r#"<div class="alert alert-danger">The email or password you entered is incorrect.</div>"#;
        assert_eq!(
            form_error(html).as_deref(),
            Some("The email or password you entered is incorrect.")
        );
        assert_eq!(form_error("<div>fine</div>"), None);
    }
}
