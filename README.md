# lcdd

Reverse-engineering and runtime tooling for the LCD screen on an ASUS liquid cooler.

This repository started as a USB capture analysis project. The goal was to understand how the vendor software talks to the cooler LCD, then replace that heavyweight Windows/Wine workflow with small native tools that do the same job on Linux.

At this point the repo can:

- inspect and decode the captured USB traffic from `aura.pcapng`
- reconstruct JPEG payloads from the capture
- replay captured sessions back to the cooler for validation
- run a native Rust service that keeps a `320x320` image on the LCD with an optional live dashboard overlay

If you want protocol details, read [docs/protocol.md](docs/protocol.md). If you want the Rust service design notes, read [docs/rust-service.md](docs/rust-service.md).

## What Is In This Repo

- [tools/lcdd_pcap.py](tools/lcdd_pcap.py): parses `aura.pcapng`, extracts upload bursts, and reconstructs JPEG payloads.
- [tools/lcdd_hid.py](tools/lcdd_hid.py): talks to the live HID device and replays captured traffic for testing.
- [src/main.rs](src/main.rs): Rust long-running service for showing a background image with an optional generated dashboard overlay on the LCD.
- [docs/protocol.md](docs/protocol.md): reverse-engineered protocol notes from the USB capture.
- [docs/rust-service.md](docs/rust-service.md): service behavior, config contract, and implementation notes.
- [aura.pcapng](aura.pcapng): the original USB capture used for reverse-engineering.

## Current State

The reverse-engineered target device is the ASUS LCD cooler at `VID:PID = 0x0b05:0x1ca9`.

The current working model is:

- interface `0` carries a `440`-byte init packet
- interface `1` carries `1024`-byte image packets and returns a `16`-byte ack
- the image payload is a baseline JPEG at `320x320`
- the Rust service packetizes JPEGs natively and keeps the image alive by continuous re-upload

This is good enough for practical use, but not every field in the protocol is fully understood yet.

## Development Environment

The easiest way to work in this repo is through the Nix dev shell defined in [flake.nix](flake.nix).

```bash
nix develop
```

From there you can run Rust and Python commands directly.

Useful checks:

```bash
cargo test
uv run tools/lcdd_hid.py list-devices
```

If you do not want an interactive shell, this also works:

```bash
nix develop -c cargo test
```

## Home Manager Module

This flake exports a Home Manager module at `homeManagerModules.default` and `homeManagerModules.lcdd`.

Import it from your Home Manager setup:

```nix
{
  inputs.lcdd.url = "github:beiyanyunyi/lcdd";

  outputs = { home-manager, lcdd, ... }: {
    homeConfigurations.me = home-manager.lib.homeManagerConfiguration {
      pkgs = import home-manager.inputs.nixpkgs {
        system = "x86_64-linux";
      };
      modules = [
        lcdd.homeManagerModules.default
        {
          programs.lcdd = {
            enable = true;
            config = {
              source.path = "/home/me/Pictures/lcd-background.png";

              dashboard.slots = [
                {
                  title = "CPU";
                  subtitle = "usage";
                  metric = "cpu_usage_percent";
                }
              ];
            };
          };
        }
      ];
    };
  };
}
```

If you already manage the runtime config file yourself, point the module at that file instead:

```nix
{
  programs.lcdd = {
    enable = true;
    configFile = "/home/me/.config/lcdd/config.toml";
  };
}
```

`programs.lcdd.config` and `programs.lcdd.configFile` are mutually exclusive. The module runs `lcdd` as a user service via `systemd --user`.

### Device Permissions

Home Manager cannot grant access to the cooler device on its own. You still need a system-level udev rule or equivalent permission setup for the known ASUS cooler `VID:PID = 0x0b05:0x1ca9`.

On NixOS, one way to do that is:

```nix
{
  services.udev.extraRules = ''
    SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0b05", ATTRS{idProduct}=="1ca9", TAG+="uaccess"
  '';
}
```

Without that step, the Home Manager user service may start but fail to open the HID device.

## Rust LCD Service

The Rust program is the intended runtime path for LCD output.

It accepts `bmp`, `ico`, `png`, `jpg/jpeg`, and `webp` inputs, then converts them into an internal `320x320` JPEG before upload.

`source.path` is always the background image path. When one or more `dashboard.slots` are configured, the service renders an optional live overlay on top using up to 4 fixed slot positions.

### JPEG Compatibility

Not every `320x320` baseline JPEG that opens on a desktop decoder works on the cooler.

Observed examples:

- `src/assets/test.jpg` loads successfully
- `out/jpegs/bursts/burst_0001/image.jpg` loads successfully
- `out/xi_small.jpg` loads successfully
- `out/xi_small_failed.jpg` does not load successfully

Two FFmpeg commands produced different results from the same source image:

Failing output:

```bash
ffmpeg -y -i out/xi.jpg -q:v 12 out/xi_small_failed.jpg
```

Working output:

```bash
ffmpeg -y -i out/xi.jpg -frames:v 1 -c:v mjpeg -pix_fmt yuvj420p -huffman default -q:v 2 out/xi_small.jpg
```

`ffprobe` and marker inspection show that the difference is not just image size or "baseline JPEG" status. The failing file uses a different JPEG marker layout, including a shorter non-default Huffman block and different SOF0 component descriptors. `pix_fmt` alone is also not enough to explain success or failure, because `src/assets/test.jpg` works while reporting `yuvj444p`.

The runtime now performs its own JPEG conversion with the `image` crate, but the FFmpeg recipe above remains the reference compatibility target when comparing behavior or debugging device-visible differences.

### Config Discovery

If you do not pass `--config`, the service looks for a config file in the current working directory in this order:

1. `config.toml`
2. `config.ron`
3. `config.corn`

You can also point to a config explicitly:

```bash
cargo run -- --config ./config.toml
```

### Example Config

```toml
basedir = "cwd" # "cwd", "config_dir", "config_dir_real", or an absolute directory like "/srv/lcdd"

[device]
vendor_id = 0x0b05
product_id = 0x1ca9
interface_init = 0
interface_bulk = 1
# serial = "A247392SS000000"

[logging]
level = "info"
color = true

[source]
path = "./image.jpg"
rotate_degrees = 0

[dashboard]
render_interval_ms = 1000
time_format = "24h"
temperature_unit = "celsius"
# font_family = "Noto Sans"
# font_path = "/path/to/NotoSansCJK-Regular.ttc"
# debug_output_path = "./out/dashboard-debug.png"

[[dashboard.slots]]
title = "CPU"
subtitle = "usage"
metric = "cpu_usage_percent"

[refresh]
interval_ms = 0
ack_timeout_ms = 2000
retry_delay_ms = 1000
reload_check_interval_ms = 500

[protocol]
init_on_connect = false # false is fine for my current cooler
```

### Running The Service

Inside the dev shell:

```bash
cargo run -- --config ./config.toml
```

Dashboard font resolution order:

- use `dashboard.font_path` when set
- otherwise resolve `dashboard.font_family` from system fonts when set
- otherwise try a best-effort sans-serif fallback list from system fonts

Relative path resolution:

- if `basedir` is omitted, relative paths are resolved from the `lcdd` process working directory
- `basedir = "cwd"` keeps that process-working-directory behavior explicitly
- `basedir = "config_dir"` resolves relative paths from the directory containing the loaded config file
- `basedir = "config_dir_real"` resolves relative paths from the real config file location after following symlinks
- any other `basedir` value must be an absolute directory path and becomes the base for relative paths
- this applies to `source.path`, `dashboard.font_path`, and `dashboard.debug_output_path`

The dashboard renderer supports UTF-8 labels for Latin and common CJK text. It does not perform complex-script shaping.
If `dashboard.debug_output_path` is set, the service also writes the final composited frame to disk before JPEG upload so you can inspect what the LCD is actually being sent.

Behavior summary:

- sends the captured init packet on connect
- packetizes the JPEG natively into `1024`-byte HID reports
- decodes common image formats, optionally rotates them, and re-encodes to an internal JPEG
- can render a live dashboard overlay with up to 4 fixed, top-aligned slots over a background image
- renders dashboard text through `font-kit`, with UTF-8 support for Latin and common CJK labels
- collects built-in metrics for aggregate CPU usage, CPU temperature, memory usage, and local time
- verifies the device ack after each upload
- keeps re-uploading the image so the LCD does not clear itself
- watches the file and reloads it when it changes
- watches the config file and live-applies valid updates
- live-applies logging level and color changes from config reloads
- retries automatically if the cooler disconnects or re-enumerates

## Python Prototype Tools

The Python scripts are still useful for reverse-engineering and validation, but they are no longer the main runtime path.

### Inspect The Capture

```bash
uv run tools/lcdd_pcap.py inspect-pcap aura.pcapng
```

### Replay A Captured Session

Dry run:

```bash
uv run tools/lcdd_hid.py replay-session ./out/session/manifest.json
```

Live write:

```bash
uv run tools/lcdd_hid.py replay-session ./out/session/manifest.json --write --pace-scale 3.0
```

### List Matching HID Devices

```bash
uv run tools/lcdd_hid.py list-devices
```

## Typical Workflow

1. Use the Python tooling to inspect the capture, validate assumptions, and compare behavior against the original vendor traffic.
2. Use the Rust service when you want an actual native Linux solution for keeping a background image or a simple live overlay on the LCD.
3. Revisit the protocol docs when the hardware behaves differently than expected.

## Limitations

- The project is currently Linux-oriented.
- The Rust service supports a background image with a simple built-in dashboard overlay, not arbitrary animation generation yet.
- Dashboard text rendering depends on a usable system font unless `dashboard.font_path` points to a specific font file.
- Images must already be preprocessed to `320x320` JPEG.
- Some protocol semantics are still inferred from capture data rather than fully proven.
- Device behavior on shutdown, disconnect, or idle periods may still depend on hardware quirks.

## Further Reading

- [docs/protocol.md](docs/protocol.md)
- [docs/rust-service.md](docs/rust-service.md)
