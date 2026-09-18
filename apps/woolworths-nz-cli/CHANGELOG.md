# Changelog

## woolworths-nz-cli/v0.3.2 (2026-09-18)

### Fixes

- stop reading the password on every command
  The client was built with the password already fetched, so every command
  paid a keychain prompt for a renewal that almost never happens. It now
  carries where the password is. auth status and auth refresh still ask
  outright, because that is what they report on.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2
- wwnz-api: 0.1.0 -> 0.1.1


## woolworths-nz-cli/v0.3.1 (2026-09-11)

### Dependencies

- build-kit: 0.2.0 -> 0.3.0
- net-kit: 0.1.0 -> 0.1.1


## woolworths-nz-cli/v0.3.0 (2026-09-04)

### Features

- show release notes in update --check
  A version number alone is not enough to decide whether to take an update, and
  the release body was being dropped at the wire type.

  `changelog()` returns every release being crossed, newest first, skipping
  previews a stable build was never offered; a downgrade answers with the notes
  of the version asked about. Under --json it is a `changelog` array.

- rebuild on the shared gsnz libraries
  The hand-rolled HTTP, auth, domain and rendering code is replaced by a Woolworths-only fork of grocery-nz-cli's architecture:
  gsnz-core, wwnz-api, cli-kit, gsnz-ui, net-kit and build-kit do the work.
  No fsnz-api, no Foodstuffs.

  The surface moves with it. A 0.2 config file is refused rather than ignored: password_command and store_password are under [auth] now, and store_name is gone.
  `--store` and WWNZ_STORE_ID no longer rebind the cart per run -- 0.2 did that on every listing command -- so `store set` is the only thing that binds it and `--store` is refused with that advice.
  WWNZ_SESSION and WWNZ_EMAIL are dropped, `orders show` and `auth refresh` exist, `--unit kg` replaces `--unit KGM`, and `auth status --json` is the status object rather than a wrapper.

### Fixes

- declare build-kit once
  dispat rewrites one version range per manifest, so the [build-dependencies]
  build-kit stayed at ^0.1.0 when the [dependencies] one moved to ^0.2.0 and the
  release failed to select a version. Latent until build-kit's first bump.

  A path dependency needs a version only to be publishable, so the second
  declaration drops it rather than carrying a number nothing keeps honest.

### Dependencies

- build-kit: 0.1.0 -> 0.2.0


## woolworths-nz-cli/v0.2.0 (2026-09-02)

### Features

- keep the password for unattended re-login
  The session cookie is encrypted with nothing to refresh it, so walking the
  login flow again is the only renewal there is -- and that needs a password.
  `auth login` now keeps it in the credential store, and the API client signs
  in again and retries once when a call comes back AUTH_NOT_AUTHENTICATED.

  Sessions from `auth import` or WWNZ_SESSION are left alone: neither names an
  account to sign back in as. `--no-store-password` and `store_password =
  false` opt out; a configured `password_command` is preferred over the stored
  copy.


## woolworths-nz-cli/v0.1.3 (2026-09-02)

### Fixes

- bump wreq version


## woolworths-nz-cli/v0.1.2 (2026-09-02)

### Fixes

- follow the redirect on a release asset download
  A GitHub release asset URL is a 302 to release-assets.githubusercontent.com,
  and wreq's ClientBuilder defaults to Policy::none() -- unlike reqwest, and
  unlike the doc comment wreq inherited from it. `wwnz update` stopped at the
  302 and reported it as a failed GET, so no published binary could install
  itself. Both fetches went through it, the tarball and SHA256SUMS.

  The policy is set per request rather than on the shared client: everywhere
  else a redirect means a bot check, and reporting one beats following it.

  The test serves the release the way GitHub does, a 302 to the host holding
  the bytes, and fails without the fix.


## woolworths-nz-cli/v0.1.1 (2026-09-02)

### Fixes

- read cart quantities as decimals, not counts
  `wwnz cart list` failed outright on any cart holding a weighed line:

      error: parsing the cart: invalid type: floating point `0.3`, expected u32

  Loose produce and meat are sold by the kilogram -- the `-KGM` variants that
  `variant_key` already knew about -- and their quantities come back in
  kilograms, so 300g of onions is `0.3`. Every quantity in the tool was a `u32`,
  so one such line took the whole command down, cart-wide.

  Quantities are `f64` throughout now, including the ones sent back: `cart add`
  and `cart update` take a decimal, so a weighed line can be set at all. Whole
  quantities still print and serialise as integers, because `2.0` is not an
  `Int` to the schema and `--json` consumers were already reading `2`.

  `totalItemQuantity` is made tolerant the same way, though the site does not in
  fact use it that way: it counts a weighed line as one item however much of it
  there is, so the item count and the sum of the quantities disagree by design.
  Both are now reported as the site reports them.

  Verified against a real cart of nineteen products with three weighed lines,
  which is also what confirmed the sign-in flow works end to end -- the README
  no longer hedges on either.


## woolworths-nz-cli/v0.1.0 (2026-09-01)

### Features

- add wwnz, a CLI for the Woolworths NZ API
  Search, specials, department browsing, store selection, cart and order
  history, against the GraphQL endpoint the woolworths.co.nz site calls.
  Replaces woolies-nz-cli, which broke when they moved to it.

  Two things about the API shape the tool. The store is a property of the
  cart rather than a search parameter, so selecting one is a mutation and
  every product command binds it before searching. Search, browse, specials
  and buy-again are four fields of one CompositeSearchInput, so they share a
  single document and a single SearchBy enum.

  Uses wreq rather than reqwest. Akamai sits in front of the site and scores
  the TLS handshake, not the headers: with rustls the storefront withholds
  its bot-manager cookies and the login is refused with a bare 400, while a
  browser handshake is answered normally. Matching headers by hand changes
  nothing, and curl -- which is how foodstuffs-nz-cli gets past Cloudflare --
  fares worse here than rustls.
