#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

TARGET_VID = "0x0b05"
TARGET_PID = "0x1ca9"
DEFAULT_BURST_GAP = 0.01


def run_tshark(pcap: Path, fields: list[str], display_filter: str | None = None) -> list[list[str]]:
    cmd = [
        "tshark",
        "-r",
        str(pcap),
        "-T",
        "fields",
        "-E",
        "header=n",
        "-E",
        "separator=\t",
        "-E",
        "quote=n",
        "-E",
        "occurrence=a",
    ]
    if display_filter:
        cmd.extend(["-Y", display_filter])
    for field in fields:
        cmd.extend(["-e", field])
    proc = subprocess.run(cmd, capture_output=True, text=True, check=True)
    rows: list[list[str]] = []
    for line in proc.stdout.splitlines():
        rows.append(line.split("\t"))
    return rows


def ensure_dir(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)


def hex_to_bytes(value: str) -> bytes:
    return bytes.fromhex(value) if value else b""


def parse_uint(value: str) -> int:
    if value.startswith("0x"):
        return int(value, 16)
    return int(value)


def parse_jpeg_dimensions(data: bytes) -> tuple[int, int] | None:
    i = 0
    while i + 9 < len(data):
        if data[i] != 0xFF:
            i += 1
            continue
        while i < len(data) and data[i] == 0xFF:
            i += 1
        if i >= len(data):
            break
        marker = data[i]
        i += 1
        if marker in {0xD8, 0xD9}:
            continue
        if i + 1 >= len(data):
            break
        seglen = int.from_bytes(data[i : i + 2], "big")
        if seglen < 2 or i + seglen > len(data):
            break
        if marker in {0xC0, 0xC1, 0xC2, 0xC3} and seglen >= 7:
            height = int.from_bytes(data[i + 3 : i + 5], "big")
            width = int.from_bytes(data[i + 5 : i + 7], "big")
            return width, height
        i += seglen
    return None


def extract_jpeg(payload: bytes) -> bytes:
    start = payload.find(b"\xff\xd8")
    if start < 0:
        raise ValueError("JPEG SOI marker not found in burst payload")
    end = payload.find(b"\xff\xd9", start)
    if end < 0:
        raise ValueError("JPEG EOI marker not found in burst payload")
    return payload[start : end + 2]


def infer_target_address(records: list[dict[str, object]]) -> int:
    candidates = Counter()
    for record in records:
        endpoint = record["endpoint"]
        if endpoint in {"0x01", "0x03", "0x82", "0x84"}:
            candidates[int(record["device_address"])] += 1
    if not candidates:
        raise ValueError("Unable to infer target device address from HID traffic")
    return candidates.most_common(1)[0][0]


def load_capture_records(pcap: Path) -> list[dict[str, object]]:
    rows = run_tshark(
        pcap,
        [
            "frame.number",
            "frame.time_epoch",
            "usb.device_address",
            "usb.endpoint_address",
            "usb.urb_type",
            "usbhid.data",
        ],
    )
    records: list[dict[str, object]] = []
    for row in rows:
        if len(row) < 6 or not row[0] or not row[2] or not row[3] or not row[4]:
            continue
        records.append(
            {
                "frame": int(row[0]),
                "time_epoch": float(row[1]) if row[1] else None,
                "device_address": int(row[2]),
                "endpoint": row[3],
                "urb_type": row[4].strip("'"),
                "hex": row[5],
            }
        )
    return records


def load_identity_frame(pcap: Path) -> dict[str, object]:
    rows = run_tshark(
        pcap,
        ["frame.number", "usb.idVendor", "usb.idProduct", "usb.device_address"],
        f"usb.idVendor=={TARGET_VID} && usb.idProduct=={TARGET_PID}",
    )
    for row in rows:
        if len(row) >= 4 and row[0]:
            device_address = int(row[3]) if row[3] else None
            return {
                "frame": int(row[0]),
                "vendor_id": row[1] or TARGET_VID,
                "product_id": row[2] or TARGET_PID,
                "device_address": device_address,
            }
    return {"frame": None, "vendor_id": TARGET_VID, "product_id": TARGET_PID, "device_address": None}


def load_configuration(pcap: Path) -> dict[str, object]:
    rows = run_tshark(
        pcap,
        [
            "frame.number",
            "usb.bInterfaceNumber",
            "usb.bEndpointAddress",
            "usb.wMaxPacketSize",
        ],
        "frame.number==32",
    )
    if not rows:
        return {"frame": None, "interfaces": []}
    row = rows[0]
    interfaces = []
    if len(row) >= 4:
        interface_numbers = [int(item) for item in row[1].split(",") if item]
        endpoints = [item for item in row[2].split(",") if item]
        sizes = [int(item) for item in row[3].split(",") if item]
        endpoint_entries = [{"address": ep, "max_packet_size": size} for ep, size in zip(endpoints, sizes)]
        endpoint_groups = [endpoint_entries[index : index + 2] for index in range(0, len(endpoint_entries), 2)]
        for interface_number, group in zip(interface_numbers, endpoint_groups):
            interfaces.append({"interface_number": interface_number, "class": "HID", "endpoints": group})
    return {"frame": int(row[0]), "interfaces": interfaces}


def analyze_pcap(pcap: Path, burst_gap: float = DEFAULT_BURST_GAP) -> dict[str, object]:
    identity = load_identity_frame(pcap)
    config = load_configuration(pcap)
    records = load_capture_records(pcap)
    device_address = identity["device_address"] or infer_target_address(records)
    filtered = [record for record in records if record["device_address"] == device_address]

    init_packet = None
    ack_packets: list[dict[str, object]] = []
    chunk_records: list[dict[str, object]] = []

    for record in filtered:
        endpoint = record["endpoint"]
        urb_type = record["urb_type"]
        data_hex = str(record["hex"])
        data = hex_to_bytes(data_hex)
        if endpoint == "0x01" and urb_type == "S" and data:
            if init_packet is None:
                init_packet = {
                    "frame": record["frame"],
                    "endpoint": endpoint,
                    "length": len(data),
                    "hex": data_hex,
                    "header_hex": data[:8].hex(),
                }
        elif endpoint == "0x84" and urb_type == "C" and data:
            ack_packets.append(
                {
                    "frame": record["frame"],
                    "endpoint": endpoint,
                    "length": len(data),
                    "hex": data_hex,
                }
            )
        elif endpoint == "0x03" and urb_type == "S" and data:
            chunk_records.append(
                {
                    "frame": record["frame"],
                    "time_epoch": record["time_epoch"],
                    "endpoint": endpoint,
                    "length": len(data),
                    "hex": data_hex,
                    "header_hex": data[:4].hex(),
                }
            )

    if init_packet is None:
        raise ValueError("No init packet was found on endpoint 0x01")
    if not chunk_records:
        raise ValueError("No upload packets were found on endpoint 0x03")

    bursts: list[dict[str, object]] = []
    current: list[dict[str, object]] = []
    previous_time: float | None = None
    previous_burst_end: float | None = None
    for chunk in chunk_records:
        current_time = float(chunk["time_epoch"])
        if previous_time is not None and current_time - previous_time > burst_gap and current:
            bursts.append(build_burst(current, previous_burst_end))
            previous_burst_end = float(current[-1]["time_epoch"])
            current = []
        current.append(chunk)
        previous_time = current_time
    if current:
        bursts.append(build_burst(current, previous_burst_end))

    manifest: dict[str, object] = {
        "pcap_path": str(pcap.resolve()),
        "identity": {
            "vendor_id": identity["vendor_id"],
            "product_id": identity["product_id"],
            "first_identity_frame": identity["frame"],
            "device_address": device_address,
        },
        "configuration": config,
        "burst_gap_seconds": burst_gap,
        "init_packet": init_packet,
        "ack_signature_hex": ack_packets[0]["hex"] if ack_packets else None,
        "ack_packets": ack_packets,
        "bursts": bursts,
        "write_frames": build_write_frames(init_packet, bursts),
    }
    return manifest


def build_burst(chunks: list[dict[str, object]], previous_burst_end: float | None) -> dict[str, object]:
    start_time = float(chunks[0]["time_epoch"])
    end_time = float(chunks[-1]["time_epoch"])
    for index, chunk in enumerate(chunks, start=1):
        prev_time = start_time if index == 1 else float(chunks[index - 2]["time_epoch"])
        chunk["chunk_index"] = index
        chunk["gap_from_previous_chunk_seconds"] = 0.0 if index == 1 else round(float(chunk["time_epoch"]) - prev_time, 9)
    return {
        "start_frame": chunks[0]["frame"],
        "end_frame": chunks[-1]["frame"],
        "chunk_count": len(chunks),
        "duration_seconds": round(end_time - start_time, 9),
        "gap_from_previous_burst_seconds": None if previous_burst_end is None else round(start_time - previous_burst_end, 9),
        "first_header_hex": chunks[0]["header_hex"],
        "chunks": chunks,
    }


def build_write_frames(init_packet: dict[str, object], bursts: list[dict[str, object]]) -> list[dict[str, object]]:
    frames = [
        {
            "frame": init_packet["frame"],
            "endpoint": init_packet["endpoint"],
            "length": init_packet["length"],
            "path": init_packet.get("path"),
        }
    ]
    for burst_index, burst in enumerate(bursts, start=1):
        for chunk in burst["chunks"]:
            frames.append(
                {
                    "frame": chunk["frame"],
                    "endpoint": chunk["endpoint"],
                    "length": chunk["length"],
                    "burst_index": burst_index,
                    "chunk_index": chunk["chunk_index"],
                    "path": chunk.get("path"),
                }
            )
    return frames


def materialize_session(manifest: dict[str, object], output_dir: Path, include_chunks: bool = True, include_jpegs: bool = True) -> dict[str, object]:
    ensure_dir(output_dir)
    init_bytes = hex_to_bytes(str(manifest["init_packet"]["hex"]))
    init_path = output_dir / "init_ep01_frame_{:06d}.bin".format(int(manifest["init_packet"]["frame"]))
    init_path.write_bytes(init_bytes)
    manifest["init_packet"]["path"] = str(init_path.resolve())

    ack_dir = output_dir / "acks"
    ensure_dir(ack_dir)
    for ack in manifest["ack_packets"]:
        ack_path = ack_dir / "ack_frame_{:06d}.bin".format(int(ack["frame"]))
        ack_path.write_bytes(hex_to_bytes(str(ack["hex"])))
        ack["path"] = str(ack_path.resolve())

    bursts_dir = output_dir / "bursts"
    ensure_dir(bursts_dir)
    for burst_index, burst in enumerate(manifest["bursts"], start=1):
        burst_dir = bursts_dir / f"burst_{burst_index:04d}"
        ensure_dir(burst_dir)
        payload = bytearray()
        chunk_paths: list[str] = []
        for chunk in burst["chunks"]:
            chunk_bytes = hex_to_bytes(str(chunk["hex"]))
            if include_chunks:
                chunk_path = burst_dir / "chunk_{:04d}_frame_{:06d}.bin".format(int(chunk["chunk_index"]), int(chunk["frame"]))
                chunk_path.write_bytes(chunk_bytes)
                chunk["path"] = str(chunk_path.resolve())
                chunk_paths.append(str(chunk_path.resolve()))
            payload.extend(chunk_bytes[4:])
        burst["chunk_paths"] = chunk_paths
        if include_jpegs:
            jpeg_bytes = extract_jpeg(bytes(payload))
            jpeg_path = burst_dir / "image.jpg"
            jpeg_path.write_bytes(jpeg_bytes)
            burst["jpeg_path"] = str(jpeg_path.resolve())
            burst["jpeg_size_bytes"] = len(jpeg_bytes)
            dims = parse_jpeg_dimensions(jpeg_bytes)
            if dims:
                burst["jpeg_dimensions"] = {"width": dims[0], "height": dims[1]}

    manifest["write_frames"] = build_write_frames(manifest["init_packet"], manifest["bursts"])
    manifest_path = output_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=False) + "\n")
    manifest["manifest_path"] = str(manifest_path.resolve())
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=False) + "\n")
    return manifest


def command_inspect(args: argparse.Namespace) -> int:
    manifest = analyze_pcap(args.pcap, burst_gap=args.burst_gap)
    summary = {
        "pcap_path": manifest["pcap_path"],
        "identity": manifest["identity"],
        "configuration": manifest["configuration"],
        "init_packet": {
            "frame": manifest["init_packet"]["frame"],
            "length": manifest["init_packet"]["length"],
            "header_hex": manifest["init_packet"]["header_hex"],
        },
        "ack_signature_hex": manifest["ack_signature_hex"],
        "ack_count": len(manifest["ack_packets"]),
        "burst_count": len(manifest["bursts"]),
        "bursts": [
            {
                "index": index,
                "start_frame": burst["start_frame"],
                "end_frame": burst["end_frame"],
                "chunk_count": burst["chunk_count"],
                "duration_seconds": burst["duration_seconds"],
                "first_header_hex": burst["first_header_hex"],
            }
            for index, burst in enumerate(manifest["bursts"], start=1)
        ],
    }
    json.dump(summary, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def command_extract_session(args: argparse.Namespace) -> int:
    manifest = analyze_pcap(args.pcap, burst_gap=args.burst_gap)
    materialize_session(manifest, args.output_dir, include_chunks=True, include_jpegs=True)
    print(manifest["manifest_path"])
    return 0


def command_reconstruct_jpegs(args: argparse.Namespace) -> int:
    manifest = analyze_pcap(args.pcap, burst_gap=args.burst_gap)
    materialize_session(manifest, args.output_dir, include_chunks=False, include_jpegs=True)
    results = [
        {
            "index": index,
            "jpeg_path": burst.get("jpeg_path"),
            "jpeg_dimensions": burst.get("jpeg_dimensions"),
            "jpeg_size_bytes": burst.get("jpeg_size_bytes"),
        }
        for index, burst in enumerate(manifest["bursts"], start=1)
    ]
    json.dump(results, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Analyze ASUS Aura LCD USB captures")
    subparsers = parser.add_subparsers(dest="command", required=True)

    inspect = subparsers.add_parser("inspect-pcap", help="Print a capture summary as JSON")
    inspect.add_argument("pcap", type=Path)
    inspect.add_argument("--burst-gap", type=float, default=DEFAULT_BURST_GAP)
    inspect.set_defaults(func=command_inspect)

    extract = subparsers.add_parser("extract-session", help="Write a manifest, chunk dumps, and JPEGs")
    extract.add_argument("pcap", type=Path)
    extract.add_argument("--output-dir", type=Path, default=Path("out/session"))
    extract.add_argument("--burst-gap", type=float, default=DEFAULT_BURST_GAP)
    extract.set_defaults(func=command_extract_session)

    reconstruct = subparsers.add_parser("reconstruct-jpegs", help="Rebuild JPEGs from upload bursts")
    reconstruct.add_argument("pcap", type=Path)
    reconstruct.add_argument("--output-dir", type=Path, default=Path("out/jpegs"))
    reconstruct.add_argument("--burst-gap", type=float, default=DEFAULT_BURST_GAP)
    reconstruct.set_defaults(func=command_reconstruct_jpegs)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
