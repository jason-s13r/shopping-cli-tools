# Changelog

## farmers-nz-cli/v0.1.0 (2026-09-18)

### Features

- add farmers, the Farmers NZ CLI

### Fixes

- stop reading the password on every command
  The client was built with the password already fetched, so every command
  paid a keychain prompt for a renewal that almost never happens. It now
  carries where the password is. auth status and auth refresh still ask
  outright, because that is what they report on.

### Dependencies

- build-kit: 0.3.0 -> 0.3.0
- net-kit: 0.1.1 -> 0.1.2
- farmers-api: 0.0.0 -> 0.1.0
