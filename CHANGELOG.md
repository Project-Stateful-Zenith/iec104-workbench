# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] - 2026-10-21

### Added

- Simulator mode: the workbench acts as the controlled station, over TCP or
  mutual TLS. Its points are edited in a Station table (address, type,
  value, IV, interrogation group, time tag, commands). It answers
  interrogation, counter interrogation, read, clock synchronisation and test
  commands from them, sends an edited value as a spontaneous event, logs the
  commands it receives, and can echo a command's result as return
  information (cause 11). Every frame goes to the telegram list. Profiles
  save the points.
- End-to-end tests of counter interrogation and of the client against the
  simulator.

### Changed

- A release builds every platform even when one of them fails.

## [0.1.0] - 2026-10-21

The first release.

### Added

- Connection settings: address sizes, k, w and t0 to t3, STARTDT on connect
  or by hand, mutual TLS.
- Telegram list with filters, a layered detail view, copy formats, and
  recordings to save and open.
- Points table with CSV export.
- Send forms for process, system, parameter and monitor-direction messages,
  and raw ASDU octets in hex.
- TOML profiles; the last settings are remembered.
- Prebuilt binaries for Windows, macOS and Linux.

[Unreleased]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/releases/tag/v0.1.0
