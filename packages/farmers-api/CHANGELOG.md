# Changelog

## farmers-api/v0.1.0 (2026-09-18)

### Features

- add the Farmers NZ storefront api client

### Fixes

- name the password rather than fetch it
  Building a client read the stored password every time, for a renewal
  that almost never runs -- a keychain prompt per command. A source with
  nothing behind it now reports the same dead end as no source at all,
  once something tries to spend it.

### Dependencies

- net-kit: 0.1.1 -> 0.1.2
