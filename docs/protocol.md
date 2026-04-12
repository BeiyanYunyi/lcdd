# ASUS Aura LCD USB Protocol Notes

This document captures only the behaviors confirmed from `aura.pcapng`.
Where fields are still unclear, they are left as raw observations instead of over-interpreting them.

## Device identity

- Capture file: `aura.pcapng`
- Target device address during capture: `6`
- `VID:PID = 0x0b05:0x1ca9`
- First frame in the capture with this identity: frame `28`
- Configuration descriptor of interest: frame `32`

## USB configuration

Frame `32` shows two HID interfaces.

### Interface 0

- Class: HID
- OUT endpoint `0x01`, interrupt, `wMaxPacketSize = 440`
- IN endpoint `0x82`, interrupt, `wMaxPacketSize = 440`

### Interface 1

- Class: HID
- OUT endpoint `0x03`, interrupt, `wMaxPacketSize = 1024`
- IN endpoint `0x84`, interrupt, `wMaxPacketSize = 16`

No HID class control requests were observed for the target device. Control traffic in the capture only shows standard descriptor reads.

## Observed traffic roles

### Session / init packet on `0x01`

- First observed at frame `73`
- Transfer length: `440` bytes
- First bytes: `12 01 00 80 64 00 00 00`
- Remainder of the packet is mostly zero in the capture

Raw prefix from frame `73`:

```text
1201008064000000...
```

This looks like a session or mode switch message before image upload begins.

### Image upload channel on `0x03`

- Packet size: `1024` bytes per HID write
- First image packet seen at frame `75`
- First packet of first burst starts with `08 14 00 80`
- First packet of later bursts commonly starts with `08 15 00 80`
- Continuation packets then use headers like `08 01 00 00`, `08 02 00 00`, `08 03 00 00`, and so on

Immediately after the 4-byte transport header, the payload contains a baseline JPEG stream.

Example from frame `75`:

```text
08 14 00 80 ff d8 ff e0 ...
```

The JPEG headers in the capture include:

```text
ff c0 00 11 08 01 40 01 40
```

That corresponds to an image size of `320 x 320`.

### Observed JPEG compatibility constraints

The upload payload is a baseline JPEG, but "baseline JPEG at `320x320`" is not by itself a sufficient compatibility rule.

Observed files:

- `src/assets/test.jpg` loads successfully
- `out/jpegs/bursts/burst_0001/image.jpg` loads successfully
- `out/xi_small.jpg` loads successfully
- `out/xi_small_failed.jpg` does not load successfully

Two FFmpeg outputs from the same source behaved differently:

Failing:

```bash
ffmpeg -y -i out/xi.jpg -q:v 12 out/xi_small_failed.jpg
```

Working:

```bash
ffmpeg -y -i out/xi.jpg -frames:v 1 -c:v mjpeg -pix_fmt yuvj420p -huffman default -q:v 2 out/xi_small.jpg
```

`ffprobe` shows:

- `out/xi_small.jpg`: `mjpeg (Baseline)`, `pix_fmt=yuvj420p`
- `out/xi_small_failed.jpg`: `mjpeg (Baseline)`, `pix_fmt=yuvj444p`
- `src/assets/test.jpg`: also reports `pix_fmt=yuvj444p`, but still works

So `pix_fmt` alone does not explain the failure.

Marker-level inspection shows additional structural differences:

- `out/xi_small_failed.jpg` uses a shorter non-default Huffman block: `ff c4 00 92 ...`
- `out/xi_small_failed.jpg` SOF0 component descriptors are:

```text
01 12 00 02 12 00 03 12 00
```

Known-good examples use more conventional SOF0 layouts:

- `src/assets/test.jpg`:

```text
01 11 00 02 11 01 03 11 01
```

- capture-derived `out/jpegs/bursts/burst_0001/image.jpg`:

```text
01 22 00 02 11 01 03 11 01
```

- `out/xi_small.jpg`:

```text
01 22 00 02 11 00 03 11 00
```

The current evidence suggests the cooler is sensitive to JPEG marker layout, Huffman-table layout, and component sampling descriptors, not just dimensions or generic baseline-JPEG validity.

For practical image preparation, prefer the explicit working FFmpeg command above until the firmware rule is better understood.

### Status / ack channel on `0x84`

- Packet size: `16` bytes
- Observed as IN completions from the device
- Example frame: `115`
- Repeated constant payload in the capture:

```text
08 81 00 00 00 00 00 00 00 00 00 00 00 00 00 00
```

These acks appear between or after bursts, not after every chunk.

## Burst structure

The upload traffic on endpoint `0x03` is naturally grouped into bursts.
Using a simple time-gap heuristic, a new burst starts when the gap between consecutive `0x03` submit packets exceeds about `0.01` seconds.

Early bursts observed:

- Burst 1: frames `75-113`, 20 chunks, first header `08140080`
- Burst 2: frames `117-157`, 21 chunks, first header `08150080`
- Burst 3: frames `161-289`, 63 chunks, first header `08150080`

Later bursts in the capture become larger, including examples with roughly `84`, `126`, and `176` chunks.

## Important frame references

- Frame `28`: first explicit `VID:PID` appearance for the target device
- Frame `32`: configuration descriptor
- Frame `73`: first 440-byte init/session packet on `0x01`
- Frame `75`: first 1024-byte image/upload chunk on `0x03`
- Frame `115`: first observed 16-byte status/ack packet on `0x84`
- Frame `117`: first chunk of the second upload burst

## Working hypothesis

- Endpoint `0x01` carries a 440-byte initialization or session command.
- Endpoint `0x03` carries image data in 1024-byte HID reports.
- The first 4 bytes of each `0x03` report are transport framing.
- JPEG data begins at offset `4` in the first chunk of a burst, and continuation chunks keep the same 4-byte framing convention.
- Endpoint `0x84` returns a fixed 16-byte completion/status message after a burst or phase boundary.

## Reverse-engineering guidance

- Prefer exact replay of captured packets before attempting synthetic packet generation.
- Keep unknown header semantics raw in manifests and tools.
- Validate device identity and expected packet lengths before any live write.
- Require observed ack bytes to match the captured constant before continuing aggressive replay.
