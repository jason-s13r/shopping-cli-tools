# Changelog

## fsnz-api/v0.2.1 (2026-09-18)

### Fixes

- read the password only when the refresh token fails
  It was fetched before every renewal, which is a keychain prompt for
  something the refresh token usually makes unnecessary. Club Plus's own
  refusal is kept, so it is still the error when there turns out to be no
  password.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2


## fsnz-api/v0.2.0 (2026-09-03)

### Features

- bind the account's cart to a store
  `store set` chose which store searches were scoped to and left the cart alone,
  so a freshly signed-in account could search but every `cart add` was refused
  with "Store is not defined" and nothing here could fix it.

  The account's cart carries its own store, and one bare POST sets it:

      POST /v1/edge/cart/store/{storeId}    no body, empty 200

  Found by recording a store change on the site; the earlier capture happened
  not to contain one, which is why an endpoint was assumed not to exist. The
  store cookies on the storefront are the browser's own copy, not the source of
  truth.

  `store set` now writes the local preference and binds the cart, so it means
  the same thing on all three shops, and Foodstuffs claims server_side_store
  like Woolworths already did. Signed out there is no cart to bind and browsing
  still works, so that case is passed over rather than raised.

  Verified against a live account: the cart reported no store, `store set` bound
  it to PAK'nSAVE Whangarei, and an add that had been refused went through.


## fsnz-api/v0.1.0 (2026-09-03)

### Features

- expose each crate's own version
  One subject for seven scopes because it is one line in each, and the reason
  is the same everywhere: `env!` expands where it is written, so a consumer
  asking for `CARGO_PKG_VERSION` gets its own version back. A crate that wants
  to report what it was built against has to be told by the crate itself.

- Club Plus login, cart and order history
  The account half: the three-call Club Plus chain, session storage with
  renew-on-demand, cart mutations, order list and detail, and previous
  purchases.

  Step 3 of the chain must go to Club Plus, not the banner API. The banner
  answers the same path with 200 and a plausible token, but the code it
  issues scopes back to NAT -- and a NAT token is not refused by the cart,
  it answers with an empty one belonging to nobody. There is now a two-server
  test asserting the banner receives zero requests for it, so the constraint
  is enforced rather than only commented.

  "Store is not defined" is matched exactly once, at the call site holding
  the raw body, and becomes Error::CartStoreUnbound immediately. Nothing
  downstream formats an error chain and greps it.

  Session, Challenge and StoredLogin implement Debug by hand to redact their
  tokens: a derived one puts bearer credentials into panic messages and
  anything that formats an error.

- Foodstuffs edge API, search, stores and departments
  Endpoints, client spec, wire types and the read half of the edge API.
  Endpoints are plain fields rather than environment reads, which is how a
  test points the whole flow at a mock server by assigning a string.

  Adds categories(), which the existing CLI does not have: the department
  tree is at GET /v1/edge/store/{id}/categories and is captured in the
  repo-root HAR. Store-scoped, and nodes carry a name and nothing else, so
  lookups match on name. Promotional nodes sit alongside real departments
  and are reported as they arrive -- classifying them would be a guess that
  goes stale weekly.

  Errors are typed. A Cloudflare interstitial is deliberately not an auth
  failure: renewing a session cannot clear a bot check, and treating it as
  one spends a login on it. HttpError keeps the raw body so the one signal
  that really is a bare string ("Store is not defined") can be matched once,
  at the boundary, rather than against a formatted error chain.

### Dependencies

- net-kit: 0.0.0 -> 0.1.0
