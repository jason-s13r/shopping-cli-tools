# Changelog

## bgnz-api/v0.1.1 (2026-09-27)

### Dependencies

- net-kit: 0.1.2 -> 0.2.0


## bgnz-api/v0.1.0 (2026-09-11)

### Features

- add client for the Briscoe Group storefronts
  One Magento 2 backend serves Briscoes and Rebel Sport behind the PWA
  Studio frontend, differing only by a `store` header — so one crate with
  a `Banner` parameter rather than two. Their identities stay separate:
  each fascia has its own Gigya site and API key, so a banner's
  credentials are filed apart and signing in to one does not sign in to
  the other.

  Three surfaces: Klevu for unauthenticated search and browse, GraphQL
  for catalogue reads plus the guarded cart/wishlist/orders/loyalty, and
  a small REST service for click-and-collect stock. Signing in is built
  around Gigya's one hard gate — `accounts.login` demands a reCAPTCHA
  token before anything else — while token minting after that, every two
  hours, is unguarded.

  Vendor shapes arrive loosely typed (booleans as integers, prices as
  strings, opening hours as JSON-in-JSON), so `wire` keeps those quirks
  in one place: a renamed field costs a column, not a command.
