pub const DEFAULT_VENDOR_ID: u16 = 0x0b05;
pub const DEFAULT_PRODUCT_ID: u16 = 0x1ca9;
pub const DEFAULT_INIT_INTERFACE: i32 = 0;
pub const DEFAULT_BULK_INTERFACE: i32 = 1;
pub const DEFAULT_REFRESH_INTERVAL_MS: u64 = 0;
pub const DEFAULT_ACK_TIMEOUT_MS: i32 = 2000;
pub const DEFAULT_RETRY_DELAY_MS: u64 = 1000;
pub const DEFAULT_RELOAD_CHECK_INTERVAL_MS: u64 = 500;

pub const INIT_PACKET_LEN: usize = 440;
pub const HID_PACKET_LEN: usize = 1024;
pub const HID_PAYLOAD_LEN: usize = HID_PACKET_LEN - 4;
pub const ACK_PACKET_LEN: usize = 16;
pub const MAX_SYNTHETIC_CHUNKS: usize = 39;
pub const EXPECTED_JPEG_WIDTH: u16 = 320;
pub const EXPECTED_JPEG_HEIGHT: u16 = 320;

pub const ACK_SIGNATURE: [u8; ACK_PACKET_LEN] = [
    0x08, 0x81, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

pub const INIT_PACKET: [u8; INIT_PACKET_LEN] = {
    let mut packet = [0u8; INIT_PACKET_LEN];
    packet[0] = 0x12;
    packet[1] = 0x01;
    packet[2] = 0x00;
    packet[3] = 0x80;
    packet[4] = 0x64;
    packet
};
