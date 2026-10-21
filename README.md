# IEC 104 Workbench

A cross-platform desktop test client for IEC 60870-5-104 stations. It connects to a controlled station (outstation), shows every frame on the wire, keeps a live table of the reported values, and sends any command, system request, parameter or monitor-direction message you build, or raw ASDU octets typed as hex.

One self-contained binary for Windows, macOS and Linux: no runtime, no OpenSSL, no GTK. The protocol is [zenith-iec104](https://github.com/Project-Stateful-Zenith/zenith-iec104); the GUI is [egui](https://github.com/emilk/egui).

> [!NOTE]
> This repository is a clone of an internal production repository. It may
> lag behind the internal one in releases and patches, though we try to
> patch issues here that we catch internally.

## Features

**Connection**
- Host, port, common address, originator address
- Address sizes: cause of transmission 1 or 2 octets, common address 1–2, object address 1–3
- Link parameters k, w and t0–t3, checked against the standard's ranges
- STARTDT on connect (or manual STARTDT / STOPDT), automatic redial every t0
- Mutual TLS (1.2 / 1.3) with PEM certificate, key and CA, optional server-name check
- Connection state and free room in the k window

**Telegram list**
- Every APDU sent and received (I, S and U frames) with time, direction, type, cause, CA and a one-line decode
- Filters by CA, object address, type identifications, causes, direction and category (S/U frames, monitor, process commands, system commands, parameters, file transfer)
- Detail pane and any number of detail windows (double-click): APCI with N(S)/N(R), data unit identifier, every information object decoded, octets in hex
- Copy as hex, as a C array, or the decoded summary
- Record to a file and open it again later; points are rebuilt from the recording

**Points**
- Latest value, quality (IV NT SB BL OV), time tag, cause and update count of every information object, keyed by CA and object address
- Export to CSV

**Send**
- Process commands: single, double, regulating step, set point normalized / scaled / float, bitstring; select or execute, qualifier, with or without CP56 time tag
- System commands: interrogation (station or group), counter interrogation with freeze mode, read, clock synchronisation, test with time tag, reset process
- Parameters: P_ME_NA/NB/NC with KPA, P_AC_NA
- Monitor direction: single, double, step position, bitstring, normalized, scaled, float, integrated totals, each with quality flags and optional time tag
- Any cause of transmission, P/N and test bit
- Hex: raw ASDU octets with a live decode, sent as one I-frame

**Profiles**: save and load all settings as TOML; the last settings are remembered.

## Download

Prebuilt binaries are attached to each [release](https://github.com/Project-Stateful-Zenith/iec104-workbench/releases).

On Linux the binary loads the system's OpenGL and X11 or Wayland libraries at run time; any desktop has them.

## Build

```sh
cargo build --release
./target/release/iec104-workbench
```

Rust 1.95 or later.

## Recording format

One telegram per line: Unix time in milliseconds, `TX` or `RX`, the APDU in hex.

```text
1792569580988 TX 68 04 07 00 00 00
1792569580989 RX 68 04 0B 00 00 00
```

## Not yet

- File transfer (F_* types are decoded and can be sent as hex, but there is no upload/download workflow)
- Scripting and a headless mode
- Redundancy groups (several connections to one station)
- Station simulator mode
- Hand-made S- and U-frames: the link layer stays with the library, so sequence numbers and TESTFR are always valid

## License

Apache-2.0
