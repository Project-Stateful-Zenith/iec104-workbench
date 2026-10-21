<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/banner-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset=".github/assets/banner-light.svg">
    <img alt="Zenith IEC104 Workbench" src=".github/assets/banner-dark.svg" width="100%">
  </picture>
</p>

# iec104-workbench

A desktop test client for IEC 60870-5-104, the protocol SCADA control centres
use to talk to substations and other remote stations over TCP. It connects to
a controlled station (outstation), shows every frame on the wire, keeps a live
table of the values the station reports, and sends whatever you build.

| View | Use |
| --- | --- |
| Telegrams | every APDU sent and received, filtered, decoded layer by layer |
| Points | the latest value, quality and time tag of every information object |
| Send | process, system, parameter and monitor-direction messages, or raw ASDU octets |
| Files | the station's directory, file downloads and uploads |
| Station | simulator mode: the workbench is the controlled station, with points you edit and report |

> [!NOTE]
> This repository is a clone of an internal production repository. It may
> lag behind the internal one in releases and patches, though we try to
> patch issues here that we catch internally.

## How it is built

The protocol is [zenith-iec104](https://github.com/Project-Stateful-Zenith/zenith-iec104):
its master driver on Tokio carries the link layer (sequence numbers, the k
and w window, the t0 to t3 timers, TESTFR), and its ASDU codec builds and
decodes every message. The GUI is [egui](https://github.com/emilk/egui).

The result is one self-contained binary for Windows, macOS and Linux: no
runtime, no OpenSSL, no GTK. On Linux it loads the system's OpenGL and X11 or
Wayland libraries at run time, which any desktop has.

What it covers:

- **Connection**: host, port, common address and originator address; cause
  of transmission 1 or 2 octets, common address 1 or 2, object address 1 to
  3; k, w and t0 to t3, checked against the standard's ranges; STARTDT on
  connect or manual STARTDT/STOPDT, and a redial every t0; mutual TLS 1.2
  and 1.3 with PEM certificate, key and CA.
- **Telegrams**: time, direction, type, cause, CA and a one-line decode of
  every I, S and U frame; filters by CA, object address, type, cause,
  direction and category; a detail pane and any number of detail windows
  with the APCI, the data unit identifier and every information object;
  copy as hex, as a C array or as the decoded line; record to a file and
  open it again.
- **Points**: value, quality (IV NT SB BL OV), time tag, cause and update
  count per CA and object address; export to CSV.
- **Send**: single, double and regulating step commands, set points
  (normalized, scaled, float) and bitstrings, select or execute or both in
  one click, QU by name, with or without CP56 time tag; interrogation, counter interrogation, read, clock
  synchronisation, test and reset process; parameters P_ME_NA/NB/NC and
  P_AC_NA; monitor-direction values with quality flags; any cause, P/N and
  test bit; raw ASDU octets in hex with a live decode.
- **Files**: call the station's directory, list its files and download
  one, from the list or by address and name, with every section's and the
  file's checksum checked; upload a file to the station in the control
  direction.
- **Simulator**: switch the mode to Simulator and the workbench listens as
  a controlled station, over TCP or mutual TLS. It answers interrogation,
  counter interrogation, read, clock synchronisation and test commands from
  its points, sends a value you edit as a spontaneous event, takes commands
  on the points you mark (directly or by select and execute), and can echo
  a command's result as return information. It serves files too: a call
  directory lists them and a client can download them; and with Accept
  uploads on it receives files a client uploads and saves them to a folder. Every frame goes to the telegram list.
- **Profiles**: all settings, simulator points included, saved and loaded
  as TOML; the last ones are remembered.

## Screenshot

<p align="center">
  <img alt="The telegram list after a station interrogation" src=".github/assets/screenshot.png" width="100%">
</p>

## Download

Prebuilt binaries for Windows, macOS (Apple silicon and Intel) and Linux are
attached to each
[release](https://github.com/Project-Stateful-Zenith/iec104-workbench/releases).
Unpack and run; there is nothing to install.

A recording (Telegrams → Save recording) holds one telegram per line: Unix
time in milliseconds, `TX` or `RX`, the APDU in hex.

```text
1792569580988 TX 68 04 07 00 00 00
1792569580989 RX 68 04 0B 00 00 00
```

## Build

```sh
cargo build --release
cargo test
./target/release/iec104-workbench
```

Rust 1.88 or newer. The tests include an end-to-end run against
zenith-iec104's own server; no hardware is needed.

Changes are listed in [CHANGELOG.md](CHANGELOG.md).

Not yet in this version: scripting and redundancy groups.

## Disclaimer

> [!WARNING]
> This project is an independent implementation, built on zenith-iec104,
> which is written from the published IEC 60870-5-104 standards. No text of
> the standards and no code of other implementations is contained in this
> repository.
>
> This project is not affiliated with, endorsed by or supported by the IEC.
> TLS follows the IEC 62351-3 profile as secondary sources describe it and
> claims no conformance to IEC 62351.
>
> The software is provided "as is", without warranty of any kind. Use it at
> your own risk, especially when sending commands to equipment.

## License

Apache-2.0, see [LICENSE](LICENSE).
Copyright 2026 Project-Stateful-Zenith Contributors.
