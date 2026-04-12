# Repository Guidelines

## Project Structure & Module Organization
`src/main.rs` contains the Rust LCD service and is the main runtime path. `tools/` holds Python utilities for capture analysis and HID replay, especially `tools/aura_pcap.py` and `tools/aura_hid.py`, for reverse-engineering. `docs/` stores protocol and service notes. `src/assets/` contains packaged runtime assets, while `out/` is generated analysis output and should be treated as disposable. Top-level config examples such as `aura-lcd.toml` live at the repository root.

## Build, Test, and Development Commands
Use the Nix shell first so `pkg-config`, `hidapi`, and related system libraries are available:

```bash
nix develop
```

Key commands:

```bash
cargo run -- --config ./aura-lcd.toml   # run the Rust LCD service
cargo test                              # run Rust tests/check compilation
uv run tools/aura_pcap.py inspect-pcap aura.pcapng
uv run tools/aura_hid.py list-devices
```

If you are not in the dev shell, `cargo test` may fail while linking `hidapi`.

## Coding Style & Naming Conventions
Follow Rust defaults: 4-space indentation, `snake_case` for functions and fields, `CamelCase` for types, and small typed structs for config/state. Keep Python scripts similarly simple with 4-space indentation, `snake_case`, and standard-library-first implementations. Prefer clear filenames tied to device behavior, protocol roles, or artifacts, for example `aura_hid.py` or `protocol.md`.

## Testing Guidelines
There is no dedicated `tests/` directory yet. For Rust changes, run `cargo test` and validate any device-facing behavior against the documented protocol in `docs/protocol.md`. For Python tooling, run the relevant `uv run` command against `aura.pcapng` and inspect generated files in `out/`. Add focused tests next to new Rust logic when practical, especially for packetization, JPEG validation, and config parsing. When checking image compatibility, do not assume any `320x320` JPEG is acceptable: compare working and failing files with `ffprobe` plus marker-level inspection, and preserve the documented safe FFmpeg recipe in `README.md`.

## Commit & Pull Request Guidelines
Prefer the `conventional-gitmoji commit style`. Keep each commit scoped to one concern. Pull requests should explain the hardware or protocol behavior being changed, list the commands run for verification, and include logs or screenshots when output is device-visible.

## Configuration & Safety Notes
Do not commit device-specific serial numbers, local paths, or captured data beyond the checked-in sample artifacts. Treat writes to the HID device as potentially disruptive and prefer dry-run or listing commands before live replay. For LCD images, prefer the known-safe MJPEG generation path documented in `README.md` instead of assuming desktop-valid JPEGs will render on device.
