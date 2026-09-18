//! Where Farmers answers.
//!
//! Three backends, and the split is the shape of this whole crate:
//!
//! | | what it serves | how it answers |
//! |---|---|---|
//! | [`Endpoints::rest`] | products, prices, categories, variations | JSON |
//! | [`Endpoints::pipeline`] | per-store stock, sign-in, the account header | HTML fragments |
//! | [`Endpoints::constructor`] | search, autocomplete, browse | JSON, no Akamai |
//!
//! Plain fields, not resolved from the environment: this crate takes values.
//! That is also how a test points the whole flow at a mock server.

/// The Intershop ICM site, and the two path segments that name it. Every REST
/// path is prefixed `/INTERSHOP/rest/WFS/{SITE}/-;loc={LOCALE}`.
pub const SITE: &str = "Farmers-Shop-Site";

/// The storefront's locale. It is a *matrix* parameter on the REST base --
/// `-;loc=en_NZ`, not `?loc=en_NZ` -- which is Intershop's own convention and
/// not a typo.
pub const LOCALE: &str = "en_NZ";

/// The currency segment the `ViewX-` pipelines carry. Only NZD exists here.
pub const CURRENCY: &str = "NZD";

/// The Constructor.io index key the storefront ships in its own bundle.
///
/// Public and search-only by design: it appears in the query string of every
/// request the site's JavaScript makes, so it is Farmers' to rotate and not a
/// credential this repo holds. See [`crate::search`].
pub const CONSTRUCTOR_KEY: &str = "key_wafe8wrKCXfPyWKw";

/// The Constructor.io client version the storefront reports. Sent because the
/// service uses it for request routing; any recent value is accepted.
pub const CONSTRUCTOR_CLIENT: &str = "ciojs-2.1469.1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// The storefront. Intershop REST, the `ViewX-` pipelines and every page
    /// are all served from this one host.
    pub origin: String,
    /// The search backend, which is a different company's service on a
    /// different host and has no Akamai in front of it.
    pub search: String,
    /// The Constructor.io index key. Overridable because it is Farmers' to
    /// rotate, so a stale build must be fixable without a release.
    pub key: String,
}

impl Default for Endpoints {
    fn default() -> Endpoints {
        Endpoints {
            origin: "https://www.farmers.co.nz".into(),
            search: "https://ac.cnstrc.com".into(),
            key: CONSTRUCTOR_KEY.into(),
        }
    }
}

impl Endpoints {
    pub fn defaults() -> Endpoints {
        Endpoints::default()
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Endpoints {
        self.origin = trim(origin.into());
        self
    }

    pub fn with_search(mut self, origin: impl Into<String>) -> Endpoints {
        self.search = trim(origin.into());
        self
    }

    pub fn with_key(mut self, key: impl Into<String>) -> Endpoints {
        self.key = key.into();
        self
    }

    /// An Intershop REST path: `rest("/products/6867065002")`.
    pub fn rest(&self, path: &str) -> String {
        format!(
            "{}/INTERSHOP/rest/WFS/{SITE}/-;loc={LOCALE}{path}",
            self.origin
        )
    }

    /// A `ViewX-` web pipeline, which is what the storefront's own pages call
    /// for anything the REST API does not cover.
    ///
    /// Note the segment order differs from the REST base -- `WFS/{SITE}/{LOCALE}/-/{CURRENCY}`
    /// against `WFS/{SITE}/-;loc={LOCALE}` -- and the two are not
    /// interchangeable. Both are written as the site itself writes them.
    pub fn pipeline(&self, action: &str) -> String {
        format!(
            "{}/INTERSHOP/web/WFS/{SITE}/{LOCALE}/-/{CURRENCY}/{action}",
            self.origin
        )
    }

    /// A Constructor.io endpoint, with the credentials every call carries.
    ///
    /// `client` identifies the browser and `session` counts its visits. Both
    /// are required; the service answers 400 without them.
    pub fn constructor(&self, path: &str, client: &str, session: u64) -> String {
        format!(
            "{}{path}?key={}&c={CONSTRUCTOR_CLIENT}&i={client}&s={session}",
            self.search, self.key
        )
    }

    /// The home page, which is what a session is warmed against. See
    /// [`crate::Client::warm`].
    pub fn home(&self) -> String {
        format!("{}/", self.origin)
    }

    pub fn login_page(&self) -> String {
        format!("{}/login", self.origin)
    }

    pub fn account_page(&self) -> String {
        format!("{}/account", self.origin)
    }

    pub fn cart_page(&self) -> String {
        format!("{}/cart", self.origin)
    }

    pub fn orders_page(&self) -> String {
        format!("{}/orders", self.origin)
    }

    /// Plural. The account holds several lists, not one.
    pub fn wishlists_page(&self) -> String {
        format!("{}/wishlists", self.origin)
    }

    /// A static asset path, left absolute if it already is one.
    ///
    /// Product images arrive both ways: the REST API sends a root-relative
    /// `/INTERSHOP/static/...` and the search index sends a fully qualified
    /// URL for the same picture.
    pub fn media(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.origin)
        }
    }

    /// A product's page on the website, from the slug the search index
    /// carries.
    pub fn page(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}/{}", self.origin, path.trim_start_matches('/'))
        }
    }
}

fn trim(origin: String) -> String {
    origin.trim_end_matches('/').to_string()
}

/// Percent-encode one query component.
///
/// Hand-rolled, and this crate encodes only what it composes itself. A URL it
/// was handed -- a product slug out of the search index, say -- is never
/// re-parsed and re-emitted, because round-tripping already-encoded text
/// through a URL type doubles the escapes.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `a=1&b=2` from pairs, in the order given.
pub fn query_string(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rest_path_carries_the_locale_as_a_matrix_parameter() {
        // `-;loc=en_NZ`, not `?loc=en_NZ`. Getting this wrong is answered with
        // the default locale rather than an error, so it would surface as
        // prices in the wrong currency.
        let e = Endpoints::defaults();
        assert_eq!(
            e.rest("/products/6867065002"),
            "https://www.farmers.co.nz/INTERSHOP/rest/WFS/Farmers-Shop-Site/-;loc=en_NZ/products/6867065002"
        );
    }

    #[test]
    fn a_pipeline_path_orders_its_segments_differently_from_the_rest_base() {
        // Not a transcription slip: the two bases genuinely disagree about
        // where the locale goes, and each is written as the site writes it.
        let e = Endpoints::defaults();
        assert_eq!(
            e.pipeline("ViewUserAccount-ProcessLogin"),
            "https://www.farmers.co.nz/INTERSHOP/web/WFS/Farmers-Shop-Site/en_NZ/-/NZD/ViewUserAccount-ProcessLogin"
        );
    }

    #[test]
    fn a_search_url_carries_the_key_the_storefront_publishes() {
        let e = Endpoints::defaults();
        let url = e.constructor("/search/robe", "a-client-id", 1);
        assert!(
            url.starts_with("https://ac.cnstrc.com/search/robe?"),
            "{url}"
        );
        assert!(url.contains("key=key_wafe8wrKCXfPyWKw"), "{url}");
        assert!(url.contains("i=a-client-id"), "{url}");
    }

    #[test]
    fn the_account_pages_are_the_short_urls_rather_than_pipelines() {
        // These are storefront aliases; the `ViewX-` paths behind them are not
        // interchangeable with them.
        let e = Endpoints::defaults();
        assert_eq!(e.cart_page(), "https://www.farmers.co.nz/cart");
        assert_eq!(e.orders_page(), "https://www.farmers.co.nz/orders");
        assert_eq!(e.wishlists_page(), "https://www.farmers.co.nz/wishlists");
    }

    #[test]
    fn an_overridden_origin_loses_its_trailing_slash() {
        // A mock server's base URL usually has one, and doubling it produces a
        // 404 that reads as a missing route.
        let e = Endpoints::defaults()
            .with_origin("http://127.0.0.1:9999/")
            .with_search("http://127.0.0.1:9998/");
        assert!(e
            .rest("/products/1")
            .starts_with("http://127.0.0.1:9999/INTERSHOP"));
        assert!(e
            .constructor("/search/x", "c", 1)
            .starts_with("http://127.0.0.1:9998/search/x?"));
    }

    #[test]
    fn an_image_url_is_absolute_whichever_half_of_the_site_sent_it() {
        // The REST API sends a root-relative path and the search index sends a
        // fully qualified one, for the same picture.
        let e = Endpoints::defaults();
        assert_eq!(
            e.media("/INTERSHOP/static/a.jpg"),
            "https://www.farmers.co.nz/INTERSHOP/static/a.jpg"
        );
        assert_eq!(
            e.media("https://www.farmers.co.nz:443/INTERSHOP/static/a.jpg"),
            "https://www.farmers.co.nz:443/INTERSHOP/static/a.jpg"
        );
    }

    #[test]
    fn a_query_string_encodes_values_and_keeps_the_order_it_was_given() {
        let params = [
            (
                "filters[manufacturername]".to_string(),
                "Karen Walker".to_string(),
            ),
            ("page".to_string(), "2".to_string()),
        ]
        .to_vec();
        assert_eq!(
            query_string(&params),
            "filters%5Bmanufacturername%5D=Karen%20Walker&page=2"
        );
    }
}
