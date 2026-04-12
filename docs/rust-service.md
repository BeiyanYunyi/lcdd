# Rust LCD Service Plan

This document is the implementation contract for the Rust service that keeps a background image on the ASUS liquid-cooler LCD, with an optional generated dashboard overlay.

## Goal

Build a long-running Rust service that:

- loads a source image from disk
- optionally renders a live dashboard overlay on top of that background
- packetizes it natively in Rust
- uploads it to the cooler LCD through the reverse-engineered HID protocol
- keeps the image visible by continuously re-uploading it
- auto-reloads the file when it changes
- auto-reconnects if the cooler disconnects or re-enumerates

The Python tools remain prototype and reverse-engineering utilities only:

- `tools/lcdd_pcap.py`
- `tools/lcdd_hid.py`

## Confirmed Protocol Facts

These are the only protocol details the Rust service should treat as hard requirements.

### Device identity

- `VID:PID = 0x0b05:0x1ca9`
- HID interface `0`: init channel
- HID interface `1`: bulk upload plus ack channel

### Init packet

The cooler accepts a single `440`-byte init/session packet before uploads.

- captured prefix: `12 01 00 80 64 00 00 00`
- full packet length: `440`
- current working assumption: the rest of the packet is zero padding

### Bulk image upload

- HID write size: `1024` bytes
- first `4` bytes are transport framing
- remaining `1020` bytes carry JPEG payload or zero padding
- JPEG data is baseline JPEG data already preprocessed to `320x320`

### Ack packet

After an upload burst, the cooler returns a constant `16`-byte ack:

```text
08 81 00 00 00 00 00 00 00 00 00 00 00 00 00 00
```

The Rust service should verify this ack on every completed upload.

## Native Packetization Rules

The Rust service should not depend on the Python-generated manifest at runtime.
It should synthesize upload packets from JPEG bytes directly.

### Framing model

A single image upload is represented as one packet sequence.

- each packet is exactly `1024` bytes
- each packet reserves `4` bytes for header, so payload capacity is `1020` bytes
- `chunk_count = ceil(jpeg_len / 1020)`

### First packet header

The first packet header is:

```text
08 <chunk_count> 00 80
```

Observed examples:

- `20` chunks -> `08 14 00 80`
- `21` chunks -> `08 15 00 80`
- `22` chunks -> `08 16 00 80`

### Continuation packet headers

Continuation packet headers count upward from `1`:

```text
08 01 00 00
08 02 00 00
...
08 <chunk_count - 1> 00 00
```

Observed examples:

- `21` chunks end with `08 14 00 00`
- `22` chunks end with `08 15 00 00`

### Initial v1 limit

For v1, support synthetic uploads up to `22` chunks.
If a JPEG exceeds this, fail with a clear validation error instead of inventing behavior that is not confirmed yet.

## Service Behavior

### Startup

- discover config from the current working directory, unless `--config <path>` is provided
- supported config formats: `toml`, `ron`, `corn`
- initialize logging with `fern`
- load and validate the source JPEG, including a conservative compatibility check for the cooler decoder
- keep retrying cooler discovery if the device is not present
- log retry events with `warn!`

### Steady state

- open the two HID interfaces for the cooler
- send the init packet once per connection
- upload the current JPEG
- wait for and verify the `0x84` ack signature
- continue re-uploading forever
- if `refresh.interval_ms = 0`, re-upload continuously with no extra inter-cycle sleep
- if the source file changes, reload and validate it, then use the new image on future uploads
- if the dashboard overlay is enabled, rerender metrics on `dashboard.render_interval_ms` while reusing the latest prepared frame between renders

### Failure handling

If any of these operations fail:

- discovery
- open
- write
- ack read
- ack mismatch

then the service should:

- log a warning
- drop the current session
- sleep for `refresh.retry_delay_ms`
- retry discovery and open
- resend init on the new connection
- restore the latest valid image

### Shutdown

On `SIGINT` or `SIGTERM`:

- stop scheduling new uploads
- finish the in-flight upload or ack wait when practical
- close handles cleanly
- exit without a panic

## Config Contract

The service is config-first.

### Config discovery order

If `--config` is not passed, search the current working directory in this order:

1. `config.toml`
2. `config.ron`
3. `config.corn`

### Config schema

```toml
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
init_on_connect = true
```

### Semantics

- `device.serial` is optional and is used to disambiguate multiple matching coolers
- `source.path` is always the background image path
- the runtime normalizes the source image to `320x320` before upload
- `refresh.interval_ms = 0` means continuous looping
- `protocol.init_on_connect = true` is the default and expected mode
- `dashboard.slots` accepts `0..=4` configured slots
- supported built-in slot metrics are `cpu_usage_percent`, `cpu_temperature`, `memory_used_percent`, and `time`
- dashboard layout provides 4 fixed stacked slot positions with title and subtitle on the left and data on the right
- if fewer than 4 slots are configured, they render in the top-most positions
- if no slots are configured, the runtime keeps background-image-only behavior

## Internal Structure

The Rust code should be organized around these responsibilities.

### Config loader

- parse CLI for `--config`
- load the selected config via the `config` crate
- deserialize into typed Rust structs
- provide defaults for all optional service tuning values

### Image source abstraction

Define a small source trait now so future image generators can reuse the sender pipeline.

V1 ships one implementation:

- watched file source

Responsibilities:

- load file bytes
- validate JPEG format
- validate `320x320`
- detect file changes
- rebuild packetized payload on successful reload
- keep the last valid image if a reload fails validation

### Packetizer

Responsibilities:

- split JPEG bytes into `1020`-byte payload chunks
- generate correct `4`-byte headers
- pad packets to `1024` bytes
- reject payloads that need more than `22` chunks

### Device session

Responsibilities:

- discover matching HID interfaces with `hidapi`
- pair interface `0` and interface `1` for the same target cooler
- send init packet on connect
- upload packets in order
- drain stale input before a new upload if necessary
- read and verify ack
- surface disconnect/reconnect conditions cleanly

## Crate Choices

Planned dependencies:

- `anyhow` for error handling
- `serde` for config deserialization
- `config` with `toml`, `ron`, and `corn` support
- `hidapi` for HID discovery and device I/O
- `jpeg-decoder` for JPEG validation
- `log` and `fern` for runtime logging
- `ctrlc` for shutdown handling

## Acceptance Criteria

The Rust service is considered complete when:

- it builds with `cargo build`
- it accepts config via default discovery or `--config`
- it validates and packetizes a `320x320` JPEG natively
- it keeps the still image visible on the LCD by continuous re-upload
- it auto-reloads when the JPEG file changes
- it auto-reconnects after unplug/replug or device re-enumeration
- it exits cleanly on `Ctrl+C`

## Manual Validation

Recommended checks after implementation:

1. start the service with a valid JPEG and verify the image appears
2. leave it running and verify the image stays visible
3. replace the JPEG on disk and verify the LCD updates automatically
4. unplug and replug the cooler USB and verify automatic recovery
5. stop with `Ctrl+C` and verify clean exit behavior
