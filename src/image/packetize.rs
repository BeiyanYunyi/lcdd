use anyhow::{Result, ensure};

use crate::protocol::{HID_PACKET_LEN, HID_PAYLOAD_LEN, MAX_SYNTHETIC_CHUNKS};

pub(crate) fn packetize_jpeg(bytes: &[u8]) -> Result<Vec<[u8; HID_PACKET_LEN]>> {
    ensure!(!bytes.is_empty(), "JPEG payload is empty");
    let chunk_count = bytes.len().div_ceil(HID_PAYLOAD_LEN);
    ensure!(
        chunk_count <= MAX_SYNTHETIC_CHUNKS,
        "JPEG payload requires {chunk_count} chunks; v1 supports at most {MAX_SYNTHETIC_CHUNKS}"
    );

    let mut packets = Vec::with_capacity(chunk_count);
    for (index, chunk) in bytes.chunks(HID_PAYLOAD_LEN).enumerate() {
        let mut packet = [0u8; HID_PACKET_LEN];
        if index == 0 {
            packet[..4].copy_from_slice(&[0x08, chunk_count as u8, 0x00, 0x80]);
        } else {
            packet[..4].copy_from_slice(&[0x08, index as u8, 0x00, 0x00]);
        }
        packet[4..4 + chunk.len()].copy_from_slice(chunk);
        packets.push(packet);
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::packetize_jpeg;
    use crate::protocol::{HID_PAYLOAD_LEN, MAX_SYNTHETIC_CHUNKS};

    #[test]
    fn packetizer_produces_expected_headers_for_20_21_22_chunks() {
        for chunks in [20usize, 21, 22] {
            let payload_len = (chunks - 1) * HID_PAYLOAD_LEN + 17;
            let payload = vec![0x5a; payload_len];
            let packets = packetize_jpeg(&payload).unwrap();
            assert_eq!(packets.len(), chunks);
            assert_eq!(packets[0][..4], [0x08, chunks as u8, 0x00, 0x80]);
            for index in 1..chunks {
                assert_eq!(packets[index][..4], [0x08, index as u8, 0x00, 0x00]);
            }
        }
    }

    #[test]
    fn packetizer_rejects_payloads_larger_than_v1_limit() {
        let payload = vec![0x7f; MAX_SYNTHETIC_CHUNKS * HID_PAYLOAD_LEN + 1];
        assert!(packetize_jpeg(&payload).is_err());
    }
}
