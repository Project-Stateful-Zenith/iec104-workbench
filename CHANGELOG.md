# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- About → Check for updates: asks GitHub for the latest release, only when
  pressed, and links to its page when it is newer than the running version.

## [1.0.0] - 2026-10-23

### Added

- `examples/walkthrough.rhai`, a scripted test that goes beyond the smoke
  test, and a README section on where the workbench stores files.
- An About window, opened from the menu bar, with the version, the license,
  a link to the repository and which Send tab to use.
- Point scaling: a point can be multiplied, divided, offset and rounded for
  display, in a Scaled column next to the raw value, in the chart and in the
  CSV export; scales are kept in the settings.
- The Send forms warn when a float value is not finite: `inf`, `NaN`, or a
  number beyond ±3.4028235e38, which went out as infinity without a word.

### Changed

- Releases attach the bare executable for each platform instead of a `.zip`
  or `.tar.gz` holding it with the README and license, plus a `SHA256SUMS`
  file with every binary's SHA-256.
- New simulator points take commands: the Commands column starts ticked.
  Unticked, the station refuses a command to the point as unknown IOA
  (cause 47), which looked like a fault.

### Fixed

- Read (C_RD_NA_1) is sent with cause 5 (request) instead of 6, from the
  Send view and from the script `read()`; stations ignored it before.
- Select + execute with a CP56 time tag sends the execute with the time it
  goes out, not the time of the select; a station that refuses old commands
  could refuse it.
- Interrogation, counter interrogation, clock synchronisation, test and
  reset process are sent even when the IOA field, which their form does not
  show, holds no valid number.
- The simulator log keeps at most 10 000 lines, dropping the older half when
  full, and draws only the lines in view; it grew without limit and slowed
  the window on long runs.
- Disconnect drops a select waiting for its confirmation and a running file
  transfer; before, the status line later said the select timed out.

## [0.6.0] - 2026-10-21

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
  the handshake timeout. Profiles without them keep the previous behaviour
  (#27).
- Scripted tests without the GUI: `iec104-workbench run <script.rhai>`
  with `--profile`, `--log`, `--log-level` and `--record`, exit code 0 on
  pass and 1 on failure. Scripts are Rhai and connect, send commands or raw
  ASDUs, wait for replies and check point values; see the README and
  `examples/smoke.rhai` (#21).
- The connection bar shows V(S), V(R) and the number of sent I-frames not
  yet acknowledged, followed from the traced frames (#22).
- The simulator receives uploaded files: with Accept uploads on, a file a
  client uploads in the control direction is saved as
  `upload-<ioa>-<name>.bin` in the folder set in the Station view (the
  system temp folder by default) and logged. Both are kept in profiles
  (#38).
- Common address and address sizes: the bar's field is labelled "ASDU
  address (CA)" and the size fields explain themselves on hover; Settings
  has an Address sizes preset (IEC 104 standard 2/2/3, CA 1 octet 2/1/3,
  compact 1/1/2). Find interrogates the broadcast common address and shows
  the CA the station answers with; the status bar hints once per CA when
  received ASDUs carry another CA than the configured one, with a "use CA
  n" button (#51).
- Redundancy groups (IEC 60870-5-104 clause 10): further `host:port`s to
  the same station in the settings make the connections one group, one of
  them started. The bar shows every member's state and STARTDT on a chosen
  member is a manual switchover; switchovers show in the status bar, data
  a failed started connection left unconfirmed is sent again on the next
  started one, and the telegram list has a Conn column (#32).
- Telegrams → Export as text… and Export as JSON… write the telegrams the
  list shows: as text with each telegram's details, or as JSON with time,
  direction, octets, type, cause, CA, object addresses and summary (#34).
- Points tab: tick a point's Chart box to draw its value over time; the
  last 10 000 numeric values per point are kept (#37).

### Changed

- Built on zenith-iec104 0.36.

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

[Unreleased]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.6.0...v1.0.0
[0.6.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Project-Stateful-Zenith/iec104-workbench/releases/tag/v0.1.0
