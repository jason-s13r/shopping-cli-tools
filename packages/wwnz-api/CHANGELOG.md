# Changelog

## wwnz-api/v0.1.2 (2026-09-19)

### Fixes

- name cart lines the variant fragments miss
  The cart document reads a line's name from its variant, spreading only
  GroceryVariant and RegulatedVariant. A general-merchandise line matches
  neither, so it arrives unnamed and prints as a blank row. Look those up
  by SKU through search, which knows every product.


## wwnz-api/v0.1.1 (2026-09-18)

### Fixes

- name the password rather than fetch it
  Building a client read the stored password every time, for a renewal
  that almost never runs -- a keychain prompt per command. A source with
  nothing behind it now reports the same dead end as no source at all,
  once something tries to spend it.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2


## wwnz-api/v0.1.0 (2026-09-03)

### Features

- expose each crate's own version
  One subject for seven scopes because it is one line in each, and the reason
  is the same everywhere: `env!` expands where it is written, so a consumer
  asking for `CARGO_PKG_VERSION` gets its own version back. A crate that wants
  to report what it was built against has to be told by the crate itself.

- Woolworths GraphQL client, Auth0 login and order detail
  The GraphQL documents, session model, Auth0 walker, cart, stores,
  categories and orders.

  Adds OrderDetails, which the existing CLI does not have -- the operation
  had never been captured. Read from a HAR recording and cut down hard from
  the site's own document, which also asks for the customer's name, email
  and phone and each payment's card suffix. None of that is needed to show
  an order, and asking for it would mean holding it; a test asserts the
  document does not.

  An in-progress order reports orderTotalInCents as 0 and the real number as
  estimatedTotalInCents, so OrderDetail::total prefers the estimate when the
  settled total is zero.

  "Not signed in" arrives as an AUTH_NOT_AUTHENTICATED extension on a *200*,
  which is why matching English in an error chain was never reliable; it is
  now read off the code. renew() is public because it is also what auth
  refresh runs -- a Woolworths session cookie is encrypted and cannot be
  renewed on its own, so a full login is the only renewal there is.

  Session and StoredSession print cookie names only, never their values.

### Fixes

- make Trace Send + Sync, and export Reauth
  A trace is held across the awaits of the login flow, and Client::renew runs
  that flow from inside a method whose future has to be Send -- so a bare
  `&dyn Fn` made renew() uncallable from an async trait impl.

  Reauth was public in a private module, so nothing outside could give a client
  the means to sign itself back in.

### Dependencies

- net-kit: 0.0.0 -> 0.1.0
