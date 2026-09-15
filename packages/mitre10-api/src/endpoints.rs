//! Where Mitre 10 answers.
//!
//! Plain fields, not resolved from the environment: this crate takes values.
//! That is also how a test points the whole flow at a mock server.

/// The OCC basesite. Every catalogue path is prefixed `/occ/v2/{BASE_SITE}`.
pub const BASE_SITE: &str = "mitre10";

/// The OCC config property carrying the Algolia application id and its
/// search-only key.
///
/// The storefront reads it at boot rather than shipping the values in its
/// bundle, and so does this crate: they are Mitre 10's credentials, not this
/// repo's to hold, and a rotated key then costs nothing here. See
/// [`crate::Client::algolia`].
pub const ALGOLIA_CONFIG_KEY: &str = "mitre10.algolia.index.config";

/// The OAuth client the web storefront itself uses. A public client, so there
/// is no secret to hold -- PKCE stands in for one. See [`crate::auth`].
pub const OAUTH_CLIENT_ID: &str = "mobile_android_public";

/// The Algolia application the storefront names, as `/config/key` answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Algolia {
    pub app_id: String,
    /// Search-only: it cannot write, and the storefront puts it in the query
    /// string of every request it makes.
    pub search_key: String,
}

impl Algolia {
    pub fn new(app_id: impl Into<String>, search_key: impl Into<String>) -> Algolia {
        Algolia {
            app_id: app_id.into(),
            search_key: search_key.into(),
        }
    }

    /// The app's own DSN host, which is where the storefront queries.
    pub fn host(&self) -> String {
        format!("https://{}-dsn.algolia.net", self.app_id.to_lowercase())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// The commerce API: OCC and the authorization server.
    pub api: String,
    /// The website, which is the `Origin` the API's CORS rules accept and the
    /// OAuth `redirect_uri`.
    pub site: String,
    /// Overrides the search host, which is otherwise the application's own
    /// DSN. A test points this at a mock server.
    pub search: Option<String>,
    /// Set only to skip discovery -- a test, or a caller that has already
    /// asked. `None` means the client fetches it on first use.
    pub algolia: Option<Algolia>,
}

impl Default for Endpoints {
    fn default() -> Endpoints {
        Endpoints {
            api: "https://ccapi.mitre10.co.nz".into(),
            site: "https://www.mitre10.co.nz".into(),
            search: None,
            algolia: None,
        }
    }
}

impl Endpoints {
    pub fn defaults() -> Endpoints {
        Endpoints::default()
    }

    pub fn with_api(mut self, origin: impl Into<String>) -> Endpoints {
        self.api = trim(origin.into());
        self
    }

    pub fn with_site(mut self, origin: impl Into<String>) -> Endpoints {
        self.site = trim(origin.into());
        self
    }

    pub fn with_search(mut self, origin: impl Into<String>) -> Endpoints {
        self.search = Some(trim(origin.into()));
        self
    }

    pub fn with_algolia(mut self, app_id: impl Into<String>, key: impl Into<String>) -> Endpoints {
        self.algolia = Some(Algolia::new(app_id, key));
        self
    }

    /// An OCC catalogue path: `occ("/products/174969")`.
    pub fn occ(&self, path: &str) -> String {
        format!("{}/occ/v2/{}{}", self.api, BASE_SITE, path)
    }

    /// A path on the authorization server, which sits outside the basesite.
    pub fn auth(&self, path: &str) -> String {
        format!("{}/authorizationserver{}", self.api, path)
    }

    /// The Algolia multi-query endpoint, for the application the storefront
    /// named. The key travels in the query string because that is where the
    /// storefront puts it and the CORS rules on the header form are stricter.
    pub fn queries(&self, algolia: &Algolia) -> String {
        let host = self.search.clone().unwrap_or_else(|| algolia.host());
        format!(
            "{}/1/indexes/*/queries?x-algolia-api-key={}&x-algolia-application-id={}",
            host, algolia.search_key, algolia.app_id
        )
    }

    /// A media path off the API host, left absolute if it already is one.
    pub fn media(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.api)
        }
    }

    /// A product's page on the website.
    pub fn page(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.site)
        }
    }
}

fn trim(origin: String) -> String {
    origin.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalogue_path_carries_the_basesite() {
        let e = Endpoints::defaults();
        assert_eq!(
            e.occ("/products/174969"),
            "https://ccapi.mitre10.co.nz/occ/v2/mitre10/products/174969"
        );
    }

    #[test]
    fn the_authorization_server_sits_outside_the_basesite() {
        // /authorizationserver/csrf, not /occ/v2/mitre10/authorizationserver/csrf.
        let e = Endpoints::defaults();
        assert_eq!(
            e.auth("/oauth/token"),
            "https://ccapi.mitre10.co.nz/authorizationserver/oauth/token"
        );
    }

    #[test]
    fn an_overridden_origin_loses_its_trailing_slash() {
        // A mock server's base URL usually has one, and doubling it produces a
        // 404 that reads as a missing route.
        let e = Endpoints::defaults().with_api("http://127.0.0.1:9999/");
        assert_eq!(
            e.occ("/products/1"),
            "http://127.0.0.1:9999/occ/v2/mitre10/products/1"
        );
    }

    #[test]
    fn queries_go_to_the_app_ids_own_dsn() {
        let e = Endpoints::defaults();
        let algolia = Algolia::new("ABCDEF1234", "not-a-real-key");
        assert!(e
            .queries(&algolia)
            .starts_with("https://abcdef1234-dsn.algolia.net/1/indexes/*/queries"));
        assert!(e.queries(&algolia).contains("not-a-real-key"));
    }

    #[test]
    fn an_overridden_search_origin_wins_over_the_dsn() {
        let e = Endpoints::defaults().with_search("http://127.0.0.1:9999/");
        assert!(e
            .queries(&Algolia::new("ABCDEF1234", "not-a-real-key"))
            .starts_with("http://127.0.0.1:9999/1/indexes/*/queries"));
    }
}
