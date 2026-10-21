# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]


### Fixed

- TLS checks that the station's certificate names the host dialed, as the
  library does by default; it was off unless ticked (#1).

## [0.5.0] - 2026-10-21

### Added

- Upload: the Files tab offers a file to the station in the control
  direction and answers its requests for it, section by section.
- The simulator serves files: add them under *Served files* in the Station
  tab with an address and a name. A call directory lists them and a client
  can download them. Profiles keep the list.

### Changed

- Built on zenith-iec104 0.35.

## [0.4.0] - 2026-10-21

### Added

- Files tab: call a station's directory and list its files (address, name,
  length, kind, time), and download a file from the list or by address and
  name. Each section's and the file's checksum is checked; a bad section is
  asked for again. Built on zenith-iec104 0.34's `ft::receive`.
- An opt-in test downloads a file from lib60870's file server.

## [0.3.0] - 2026-10-21

### Added

- Select + execute: one click sends the select, waits for the station's
  confirmation and sends the execute. A negative confirmation, a cause 44
  to 47 mirror or no answer within t1 stops it, with the reason in the
  status bar.
- The qualifier of command (QU) is chosen by name: no additional
  definition, short pulse, long pulse or persistent output; other values up
  to 31 are typed in.
- Simulator: *Require select* makes the station accept commands only by
  select and execute.

### Changed

- Causes 44 to 47 show as `unk type`, `unk cause`, `unk CA` and `unk IOA`
  in the telegram list.

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

[Unreleased]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/releases/tag/v0.1.0
