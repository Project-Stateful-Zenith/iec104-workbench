# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Simulator points carry the NT, SB, BL and OV quality flags besides IV,
  set in the Station table's Quality column and kept in profiles (#24).
- The simulator log shows `<peer> disconnected` when a client goes away
  (#28).
- Simulator points can be sent cyclically (cause 1): set a period in
  seconds in the Station table's Cycle column; it is kept in profiles and
  sends the point's current value, edits included (#35).
- TLS settings: a certificate revocation list (optionally checking the
  peer's certificate only, or accepting certificates no list covers), an
  allow-list of peer certificates, the minimum and maximum TLS version and
  the handshake timeout. Profiles without them keep today's behaviour.

### Fixed

- The simulator's point table reads bitstrings by the send forms' rule, so
  `0XBEEF` is accepted there too (#9).
- The simulator runs each TLS handshake on its own: a client that connected
  and stalled its handshake held off every other client for 10 s (#8).
- The simulator waits 100 ms after a failed accept instead of retrying at
  once, which spun a core on a lasting error (#7).
- A file transfer and a pending select + execute stop when the connection
  drops; the Files tab stayed busy until Cancel (#6).
- Recordings keep their address sizes (`# sizes` lines), and every
  telegram keeps the sizes it was read with: a recording opened under other
  settings, or a telegram shown after the settings changed, was decoded with
  the wrong sizes. Older recordings load as before (#5).
- The telegram list keeps at most 1 000 000 telegrams and drops the oldest
  half when full; it grew without limit, about 2.6 GB a day at 100 frames a
  second (#4).
- Normalized set points, parameters and monitor values outside −1..1 (or
  not a number) are refused; they were clamped silently, so `5` went out as
  0.99997 and `nan` as 0 (#3).
- Opening a profile and the settings window are locked while connected or
  while the simulator runs: new address sizes under a running link could
  send a command to another object (#2).
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
