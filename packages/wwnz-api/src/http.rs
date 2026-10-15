//! How this client presents itself.

use net_kit::wreq_util::Profile;
use net_kit::ClientSpec;

/// The browser this client presents as, at every layer.
///
/// Akamai scores the TLS handshake, the HTTP/2 settings and the headers
/// together, so they have to agree. `wreq` derives all three from this one
/// value -- including the `User-Agent` -- which is why nothing here sets a user
/// agent by hand: a header naming a different Firefox than the handshake is
/// exactly the inconsistency being watched for.
///
/// The version is load-bearing. Auth0 enforces the sign-in CAPTCHA only on
/// fingerprints it distrusts: measured 2026-10-15, `Firefox139` and
/// `Firefox148` are refused at the email step with a bare 400, `Firefox151`
/// is not.
pub const EMULATION: Profile = Profile::Firefox151;

/// Firefox, **no cookie jar**, and **no redirect policy**.
///
/// Both absences are deliberate and neither should be copied from the
/// Foodstuffs client, which needs the opposite of each. There is nothing to
/// keep between requests -- the session cookie is sent explicitly per call --
/// and an unexpected redirect here is a bot check, which must surface rather
/// than be quietly followed. Redirects are opted into per request in exactly
/// two places: the login flow, and an update's asset download.
pub fn client_spec() -> ClientSpec {
    ClientSpec::new(EMULATION, net_kit::wreq::redirect::Policy::none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_agent_is_a_firefox_not_the_other_clients_chrome() {
        let ua = net_kit::http::user_agent(EMULATION);
        assert!(ua.contains("Firefox"), "{ua}");
        assert!(!ua.contains("Chrome"), "{ua}");
    }
}
