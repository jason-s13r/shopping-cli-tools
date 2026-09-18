# Changelog

## net-kit/v0.1.2 (2026-09-18)

### Fixes

- fetch each stored secret once per process
  Every credential-store access is a keychain prompt on a Mac, and a
  single command made several: the same session read three times over, a
  password read whether or not anything would spend it.

  A store now remembers what it has fetched, skips a write that would
  change nothing, and keeps its contents out of Debug. Source::Stored
  holds the store rather than the password, so a caller can name where the
  password is and read it only when signing in again.


## net-kit/v0.1.1 (2026-09-11)

### Fixes

- return the landed URL alongside response text


## net-kit/v0.1.0 (2026-09-03)

### Features

- expose each crate's own version
  One subject for seven scopes because it is one line in each, and the reason
  is the same everywhere: `env!` expands where it is written, so a consumer
  asking for `CARGO_PKG_VERSION` gets its own version back. A crate that wants
  to report what it was built against has to be told by the crate itself.

- shared process boundary for the CLIs
  HTTP that is not scored as a bot, cookies that survive a run, credentials
  that are not a plaintext file, and the paths those live under -- the half
  of both existing CLIs that is copy-paste.

  Nothing here reads the environment. The existing apps call std::env::var
  from inside Endpoints::resolve and Secrets::new, which is why their unit
  tests set_var and race; every entry point takes the value instead, and
  clippy.toml enforces it. It is also what lets two apps with different
  variable prefixes share this code.

  ClientSpec has no Default and requires profile and redirect: one vendor
  needs a cookie jar and followed redirects, the other needs neither, and a
  default would eventually be pointed at the wrong one silently. HttpError
  keeps the status code and raw body so callers ask instead of matching
  substrings against a formatted error chain.

  The file secrets backend keys on service as well as account, so two tools
  sharing a state directory cannot read each other's logins.

### Fixes

- show the TOML parse detail Display was dropping
  Error::Toml captured the parse error into `detail` and then never printed it:
  the variant's Display was "{context}" alone, and `detail` carried no #[source]
  either, so the chain walk could not reach it. A config typo reported as
  "reading config.toml" and nothing else.
