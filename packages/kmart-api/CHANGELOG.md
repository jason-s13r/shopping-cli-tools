# Changelog

## kmart-api/v0.2.1 (2026-09-18)

### Fixes

- name the password rather than fetch it
  Building a client read the stored password every time, for a renewal
  that almost never runs -- a keychain prompt per command. A source with
  nothing behind it now reports the same dead end as no source at all,
  once something tries to spend it.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2


## kmart-api/v0.2.0 (2026-09-11)

### Features

- tell a token not yet fetched from one that ran out
  `Tokens::pending` is the state `from_refresh` starts in, and the one
  every browser login and every pasted token passes through. `lapsed` is
  true of it as well -- that is what makes the next call fetch an access
  token -- but the two mean opposite things to a person, and reporting a
  sign-in that worked a second ago as expired is wrong.

### Dependencies

- net-kit: 0.1.0 -> 0.1.1


## kmart-api/v0.1.0 (2026-09-05)

### Features

- let a login carry browser-earned admission
  The password submit is guarded by Akamai, which binds admission to the
  client that ran its sensor. `auth::login` now takes an admission cookie
  set and seeds it onto the auth host before the flow, so a `_abck` earned
  elsewhere can be tried across the TLS boundary. The seed reads itself
  back out of the jar and reports whether the cookie will actually send
  and whether it reads validated -- a scope bug or a stale cookie tests
  nothing, and a 403 must not be misread as a verdict.

- add a client for Kmart Australia and New Zealand
  Four surfaces behind one client: a Constructor.io catalogue that needs no
  credentials, a commercetools-backed GraphQL gateway that needs Akamai
  cookies, a second half of that gateway that needs an account token too,
  and Auth0. Telling those apart is most of the crate -- a bot challenge, a
  lapsed session and a missing one all arrive as failures and need opposite
  advice.

  Both countries share one backend, one Auth0 tenant and one store-id
  namespace, so `Country` is a parameter rather than a second crate.

  Akamai guards exactly the step of the login that would produce a session,
  so `auth::login` is correct against Auth0 and cannot be used; the way in
  is `auth::refresh`, from a token a browser obtained.

  The Constructor index key and the Auth0 client id are read from the storefront
  home page rather than only compiled in, so a rotation heals itself; `vendor` is
  the one module they live in, and the values that shipped are the fallback.

  The two countries turn out to be separate Auth0 applications despite the shared
  tenant -- read off the Australian storefront rather than assumed to match -- so
  a session records which one minted it and renews under that.
