#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///

from __future__ import annotations

import argparse
import json
import os
import select
import sys
import time
from pathlib import Path

TARGET_VID = 0x0B05
TARGET_PID = 0x1CA9
EXPECTED_INIT_LENGTH = 440
EXPECTED_BULK_LENGTH = 1024
EXPECTED_ACK_LENGTH = 16


def read_text(path: Path) -> str | None:
    try:
        return path.read_text().strip()
    except OSError:
        return None


def find_ancestor_with(path: Path, name: str) -> Path | None:
    current = path.resolve()
    for candidate in [current, *current.parents]:
        target = candidate / name
        if target.exists():
            return candidate
    return None


def load_hid_id(device_dir: Path) -> tuple[int | None, int | None]:
    uevent_path = device_dir / "uevent"
    text = read_text(uevent_path)
    if text:
        for line in text.splitlines():
            if line.startswith("HID_ID="):
                _, payload = line.split("=", 1)
                parts = payload.split(":")
                if len(parts) == 3:
                    return int(parts[1], 16), int(parts[2], 16)
    usb_node = find_ancestor_with(device_dir, "idVendor")
    if usb_node is None:
        return None, None
    vid_text = read_text(usb_node / "idVendor")
    pid_text = read_text(usb_node / "idProduct")
    if vid_text and pid_text:
        return int(vid_text, 16), int(pid_text, 16)
    return None, None


def load_interface_number(device_dir: Path) -> int | None:
    iface_node = find_ancestor_with(device_dir, "bInterfaceNumber")
    if iface_node is None:
        return None
    value = read_text(iface_node / "bInterfaceNumber")
    if not value:
        return None
    try:
        return int(value, 16)
    except ValueError:
        try:
            return int(value)
        except ValueError:
            return None


def list_hidraw_devices() -> list[dict[str, object]]:
    devices = []
    for hidraw_dir in sorted(Path("/sys/class/hidraw").glob("hidraw*")):
        devnode = Path("/dev") / hidraw_dir.name
        device_dir = (hidraw_dir / "device").resolve()
        vid, pid = load_hid_id(device_dir)
        interface_number = load_interface_number(device_dir)
        role = None
        if vid == TARGET_VID and pid == TARGET_PID:
            if interface_number == 0:
                role = "init-440"
            elif interface_number == 1:
                role = "bulk-1024-ack-16"
            else:
                role = "target-unknown-interface"
        devices.append(
            {
                "hidraw": hidraw_dir.name,
                "path": str(devnode),
                "sysfs_path": str(device_dir),
                "vendor_id": None if vid is None else f"0x{vid:04x}",
                "product_id": None if pid is None else f"0x{pid:04x}",
                "interface_number": interface_number,
                "role": role,
            }
        )
    return devices


def filter_target_devices() -> list[dict[str, object]]:
    return [
        device
        for device in list_hidraw_devices()
        if device["vendor_id"] == f"0x{TARGET_VID:04x}" and device["product_id"] == f"0x{TARGET_PID:04x}"
    ]


def select_nodes(devices: list[dict[str, object]], init_node: str | None, bulk_node: str | None) -> tuple[str, str]:
    if init_node and bulk_node:
        return init_node, bulk_node

    init_candidate = None
    bulk_candidate = None
    for device in devices:
        if device["interface_number"] == 0 and init_candidate is None:
            init_candidate = str(device["path"])
        elif device["interface_number"] == 1 and bulk_candidate is None:
            bulk_candidate = str(device["path"])

    selected_init = init_node or init_candidate
    selected_bulk = bulk_node or bulk_candidate
    if not selected_init or not selected_bulk:
        raise ValueError("Unable to auto-select both init and bulk hidraw nodes")
    return selected_init, selected_bulk


def load_manifest(path: Path) -> dict[str, object]:
    return json.loads(path.read_text())


def read_packet(path: str) -> bytes:
    return Path(path).read_bytes()


def require_lengths(manifest: dict[str, object]) -> None:
    init_length = int(manifest["init_packet"]["length"])
    if init_length != EXPECTED_INIT_LENGTH:
        raise ValueError(f"Unexpected init packet length: {init_length}")
    for burst in manifest["bursts"]:
        for chunk in burst["chunks"]:
            length = int(chunk["length"])
            if length != EXPECTED_BULK_LENGTH:
                raise ValueError(f"Unexpected bulk chunk length in frame {chunk['frame']}: {length}")


def drain_nonblocking(fd: int) -> None:
    while True:
        ready, _, _ = select.select([fd], [], [], 0)
        if not ready:
            return
        try:
            os.read(fd, 4096)
        except BlockingIOError:
            return


def write_exact(fd: int, payload: bytes) -> None:
    view = memoryview(payload)
    total = 0
    while total < len(payload):
        written = os.write(fd, view[total:])
        if written <= 0:
            raise OSError("Short or failed hidraw write")
        total += written


def read_ack(fd: int, timeout: float) -> bytes:
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("Timed out waiting for device ack")
        ready, _, _ = select.select([fd], [], [], remaining)
        if not ready:
            continue
        data = os.read(fd, 4096)
        if not data:
            continue
        return data


def preview_replay(manifest: dict[str, object], init_path: str, bulk_path: str) -> dict[str, object]:
    return {
        "init_node": init_path,
        "bulk_node": bulk_path,
        "init_frame": manifest["init_packet"]["frame"],
        "burst_count": len(manifest["bursts"]),
        "bulk_chunk_count": sum(int(burst["chunk_count"]) for burst in manifest["bursts"]),
        "expected_ack_hex": manifest.get("ack_signature_hex"),
    }


def maybe_sleep(seconds: float | None, scale: float) -> None:
    if seconds is None or seconds <= 0 or scale <= 0:
        return
    time.sleep(seconds * scale)


def replay_once(
    manifest: dict[str, object],
    init_fd: int,
    bulk_fd: int,
    pace_mode: str,
    pace_scale: float,
    ack_timeout: float,
) -> list[str]:
    require_lengths(manifest)
    expected_ack = bytes.fromhex(manifest["ack_signature_hex"]) if manifest.get("ack_signature_hex") else None
    observed_acks = []
    init_payload = read_packet(manifest["init_packet"]["path"])
    if len(init_payload) != EXPECTED_INIT_LENGTH:
        raise ValueError("Init packet file length does not match manifest")

    drain_nonblocking(bulk_fd)
    write_exact(init_fd, init_payload)
    for burst in manifest["bursts"]:
        if pace_mode in {"burst", "full"}:
            maybe_sleep(burst.get("gap_from_previous_burst_seconds"), pace_scale)
        for chunk in burst["chunks"]:
            if pace_mode == "full":
                maybe_sleep(chunk.get("gap_from_previous_chunk_seconds"), pace_scale)
            payload = read_packet(chunk["path"])
            if len(payload) != EXPECTED_BULK_LENGTH:
                raise ValueError(f"Chunk file length mismatch for frame {chunk['frame']}")
            write_exact(bulk_fd, payload)
        ack = read_ack(bulk_fd, ack_timeout)
        observed_acks.append(ack.hex())
        if expected_ack is not None:
            if len(ack) != EXPECTED_ACK_LENGTH:
                raise ValueError(f"Unexpected ack length {len(ack)}; expected {EXPECTED_ACK_LENGTH}")
            if ack != expected_ack:
                raise ValueError(
                    f"Ack mismatch after burst starting at frame {burst['start_frame']}: "
                    f"expected {expected_ack.hex()}, got {ack.hex()}"
                )
    return observed_acks


def replay_session(
    manifest: dict[str, object],
    init_path: str,
    bulk_path: str,
    pace_mode: str,
    pace_scale: float,
    ack_timeout: float,
    cycles: int,
    loop_delay: float,
) -> dict[str, object]:
    if cycles < 0:
        raise ValueError("cycles must be >= 0")

    init_fd = os.open(init_path, os.O_RDWR)
    bulk_fd = os.open(bulk_path, os.O_RDWR | os.O_NONBLOCK)
    completed_cycles = 0
    observed_acks: list[str] = []
    try:
        while cycles == 0 or completed_cycles < cycles:
            observed_acks.extend(replay_once(manifest, init_fd, bulk_fd, pace_mode, pace_scale, ack_timeout))
            completed_cycles += 1
            if (cycles == 0 or completed_cycles < cycles) and loop_delay > 0:
                time.sleep(loop_delay)
        return {"status": "ok", "completed_cycles": completed_cycles, "observed_ack_hex": observed_acks}
    finally:
        os.close(bulk_fd)
        os.close(init_fd)


def send_captured_frame(manifest: dict[str, object], frame: int, init_path: str, bulk_path: str) -> dict[str, object]:
    require_lengths(manifest)
    for entry in manifest["write_frames"]:
        if int(entry["frame"]) != frame:
            continue
        payload = read_packet(entry["path"])
        if entry["endpoint"] == "0x01":
            target = init_path
        elif entry["endpoint"] == "0x03":
            target = bulk_path
        else:
            raise ValueError(f"Unsupported endpoint in manifest frame entry: {entry['endpoint']}")
        fd = os.open(target, os.O_RDWR)
        try:
            write_exact(fd, payload)
        finally:
            os.close(fd)
        return {"status": "ok", "frame": frame, "endpoint": entry["endpoint"], "target": target}
    raise ValueError(f"Frame {frame} not found in manifest write_frames")


def command_list_devices(_: argparse.Namespace) -> int:
    json.dump(list_hidraw_devices(), sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def command_replay_session(args: argparse.Namespace) -> int:
    manifest = load_manifest(args.manifest)
    devices = filter_target_devices()
    init_path, bulk_path = select_nodes(devices, args.init_node, args.bulk_node)
    preview = {
        **preview_replay(manifest, init_path, bulk_path),
        "pace_mode": args.pace_mode,
        "pace_scale": args.pace_scale,
        "cycles": args.cycles,
        "loop_delay": args.loop_delay,
    }
    if not args.write:
        json.dump({"status": "dry-run", **preview}, sys.stdout, indent=2)
        sys.stdout.write("\n")
        return 0
    result = replay_session(
        manifest,
        init_path,
        bulk_path,
        pace_mode=args.pace_mode,
        pace_scale=args.pace_scale,
        ack_timeout=args.ack_timeout,
        cycles=args.cycles,
        loop_delay=args.loop_delay,
    )
    json.dump({**preview, **result}, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def command_send_captured_frame(args: argparse.Namespace) -> int:
    manifest = load_manifest(args.manifest)
    devices = filter_target_devices()
    init_path, bulk_path = select_nodes(devices, args.init_node, args.bulk_node)
    if not args.write:
        json.dump(
            {
                "status": "dry-run",
                "frame": args.frame,
                "init_node": init_path,
                "bulk_node": bulk_path,
            },
            sys.stdout,
            indent=2,
        )
        sys.stdout.write("\n")
        return 0
    result = send_captured_frame(manifest, args.frame, init_path, bulk_path)
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Interact with the lcdd target hidraw interfaces")
    subparsers = parser.add_subparsers(dest="command", required=True)

    list_parser = subparsers.add_parser("list-devices", help="List hidraw nodes and identify the target device")
    list_parser.set_defaults(func=command_list_devices)

    replay = subparsers.add_parser("replay-session", help="Replay the captured init packet and upload bursts")
    replay.add_argument("manifest", type=Path)
    replay.add_argument("--init-node", type=str)
    replay.add_argument("--bulk-node", type=str)
    replay.add_argument(
        "--pace-mode",
        choices=["burst", "full", "none"],
        default="burst",
        help="Apply captured timing by burst gaps, all gaps, or not at all",
    )
    replay.add_argument(
        "--pace-scale",
        type=float,
        default=1.0,
        help="Multiply captured sleep durations by this factor",
    )
    replay.add_argument(
        "--cycles",
        type=int,
        default=1,
        help="Number of times to replay the full animation; use 0 to loop forever",
    )
    replay.add_argument(
        "--loop-delay",
        type=float,
        default=0.0,
        help="Extra delay in seconds between completed animation cycles",
    )
    replay.add_argument("--ack-timeout", type=float, default=2.0)
    replay.add_argument("--write", action="store_true", help="Actually write packets to the hidraw nodes")
    replay.set_defaults(func=command_replay_session)

    send_frame = subparsers.add_parser("send-captured-frame", help="Write one captured frame from the manifest")
    send_frame.add_argument("manifest", type=Path)
    send_frame.add_argument("frame", type=int)
    send_frame.add_argument("--init-node", type=str)
    send_frame.add_argument("--bulk-node", type=str)
    send_frame.add_argument("--write", action="store_true", help="Actually write the frame to the selected hidraw node")
    send_frame.set_defaults(func=command_send_captured_frame)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
